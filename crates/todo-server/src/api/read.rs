//! Read endpoints, served from the in-memory state (and the event log for the activity feed).
//!
//! Every read runs on the blocking pool through bounded admission ([`blocking`] with [`Work::Read`]),
//! against an immutable snapshot of the state, so a slow read never occupies an async worker or holds
//! up the writer. Scans check the request's [`Cancel`] every [`TICK`] items, so work stops soon after
//! the request deadline even when nothing matches.
//!
//! # Pagination
//!
//! Collection reads are paginated with a keyset cursor. Each endpoint orders its items by a sort key
//! (an [`OrderKey`] that ends with the item ID, so keys are unique). A page is selected in one pass:
//! items whose key is after the cursor's key, keeping only the smallest `limit + 1` in a bounded heap,
//! so work is linear in the items scanned and memory is bounded by the page size; nothing is sorted
//! in full.
//!
//! `next_cursor` is opaque: base64url of `{"b": binding, "k": key}`. The key is the last item's sort
//! key, so the next page starts at that *position* even if the item was since deleted or moved. The
//! binding is a hash of the route, its filters (path ID, `include_deleted`, `done`, the normalized
//! query) and, for smart-list queries, the evaluation date and UTC offset. A cursor used with a
//! different route, filter or day is rejected with 400 `invalid_cursor`: restart from the first page.
//!
//! Pages are not a snapshot. Items added or moved behind the cursor are not seen; an item whose sort
//! key changes between pages can be skipped or appear twice. Clients that need an exact copy sync
//! through the event log instead.
//!
//! Pages stop at `limit` items (at most [`MAX_PAGE`]) or [`MAX_PAGE_BYTES`] of JSON *including* the
//! envelope, whichever comes first. Items are measured while they are serialized, against the bytes
//! still left. If the first item alone is too large, the response is 500 `item_too_large` with the
//! `next_cursor` that skips it.
//!
//! # Access
//!
//! Agents only see what their grants let them read ([`visibility`]): other items are left out of
//! pages, and a list or task they cannot read is 404, exactly like one that does not exist.

use super::dto::{
    ActivityItem, ActivityPage, ActivityParams, CollectionPage, DEFAULT_PAGE, IncludeDeleted,
    ListPage, ListTasksParams, MAX_PAGE, PageParams, ReadParams, SmartList, TagPage, TaskDetail,
    TaskPage, TaskQueryParams,
};
use super::error::{ApiError, ErrorBody};
use super::{ApiPath, ApiQuery, RequestCancel, Work, blocking, visibility};
use crate::app::AppState;
use crate::auth::Authenticated;
use crate::cancel::Cancel;
use crate::summary;
use axum::Json;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::io;
use todo_core::Actor;
use todo_core::ids::ListId;
use todo_core::model::{Collection, List, Tag, Task};
use todo_core::query::{BUILTIN_LISTS, EvalContext, Expr, OrderKey, Query, fold};
use todo_core::state::State as CoreState;

/// Most bytes of JSON in one page of a collection read, envelope included.
pub const MAX_PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Bytes of a page reserved for the envelope (`next_cursor`, braces). A cursor holds at most a folded
/// title (500 characters) plus positions and IDs, well under this.
const PAGE_ENVELOPE: usize = 16 * 1024;
/// Longest accepted cursor, in bytes.
const MAX_CURSOR_BYTES: usize = 8 * 1024;
/// Items scanned between two checks of the request's cancellation.
pub const TICK: usize = 256;
/// Most items of each kind (steps, links, attachments, reminders, tags) in a task detail; more sets
/// `truncated`.
pub const MAX_DETAIL_CHILDREN: usize = 200;
/// Longest accepted smart-list query text, in bytes.
pub const MAX_QUERY_BYTES: usize = 1024;
/// Most AST nodes (operators, predicates and sort clauses) in a smart-list query.
pub const MAX_QUERY_NODES: usize = 64;
/// Most predicates (terms) in a smart-list query.
pub const MAX_QUERY_TERMS: usize = 32;

fn live_or(state: &CoreState, include_deleted: bool, id: todo_core::ids::EntityId) -> bool {
    include_deleted || state.is_live(id)
}

/// Runs `f` against the current state snapshot on the blocking pool.
async fn read<T, F>(app: &AppState, cancel: Cancel, f: F) -> Result<T, ApiError>
where
    F: FnOnce(&CoreState, &Cancel) -> Result<T, ApiError> + Send + 'static,
    T: Send + 'static,
{
    let engine = app.engine.clone();
    blocking(app, Work::Read, move || f(&engine.state(), &cancel)).await?
}

fn page_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

/// What a cursor is bound to: a hash of the route and everything that selects or orders its items.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding(String);

impl Binding {
    fn new(parts: &[&str]) -> Self {
        let mut h = Sha256::new();
        for p in parts {
            h.update(p.as_bytes());
            h.update([0x1f]);
        }
        Self(hex::encode(&h.finalize()[..8]))
    }
}

#[derive(Serialize, Deserialize)]
struct CursorData {
    b: String,
    k: OrderKey,
}

fn encode_cursor(binding: &Binding, key: &OrderKey) -> String {
    let data = CursorData {
        b: binding.0.clone(),
        k: key.clone(),
    };
    // Serializing strings and integers cannot fail.
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&data).unwrap_or_default())
}

/// Decodes a cursor and checks its binding, before any work is admitted.
fn decode_cursor(cursor: Option<&str>, binding: &Binding) -> Result<Option<OrderKey>, ApiError> {
    let Some(c) = cursor else { return Ok(None) };
    if c.len() > MAX_CURSOR_BYTES {
        return Err(ApiError::invalid_cursor());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(c)
        .map_err(|_| ApiError::invalid_cursor())?;
    let data: CursorData =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::invalid_cursor())?;
    if data.b != binding.0 {
        return Err(ApiError::invalid_cursor());
    }
    Ok(Some(data.k))
}

struct Entry<'a, T> {
    key: OrderKey,
    item: &'a T,
}

impl<T> PartialEq for Entry<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl<T> Eq for Entry<'_, T> {}
impl<T> PartialOrd for Entry<'_, T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Entry<'_, T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

/// The `n` smallest items (by `key`) among those `keep` accepts whose key is after `after`, in key
/// order. One pass with a bounded max-heap; checks `cancel` every [`TICK`] items.
fn select<'a, T>(
    items: impl Iterator<Item = &'a T>,
    keep: impl Fn(&T) -> bool,
    key: impl Fn(&T) -> OrderKey,
    after: Option<&OrderKey>,
    n: usize,
    cancel: &Cancel,
) -> Result<Vec<(OrderKey, &'a T)>, ApiError>
where
    T: 'a,
{
    let mut heap: BinaryHeap<Entry<'a, T>> = BinaryHeap::with_capacity(n.saturating_add(1));
    for (i, item) in items.enumerate() {
        if i % TICK == 0 && cancel.is_cancelled() {
            return Err(ApiError::timeout());
        }
        if !keep(item) {
            continue;
        }
        let k = key(item);
        if after.is_some_and(|a| k <= *a) {
            continue;
        }
        if heap.len() < n {
            heap.push(Entry { key: k, item });
        } else if heap.peek().is_some_and(|top| k < top.key) {
            heap.pop();
            heap.push(Entry { key: k, item });
        }
    }
    // Matchers may give up early once cancelled (see `Query::matcher_until`); never return their
    // partial answers.
    if cancel.is_cancelled() {
        return Err(ApiError::timeout());
    }
    Ok(heap
        .into_sorted_vec()
        .into_iter()
        .map(|e| (e.key, e.item))
        .collect())
}

/// Counts bytes and fails once more than `cap` were written, which stops serialization early.
struct Meter {
    n: usize,
    cap: usize,
}

impl io::Write for Meter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.n = self.n.saturating_add(buf.len());
        if self.n > self.cap {
            Err(io::Error::other("over budget"))
        } else {
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The JSON size of `v` if it is at most `cap` bytes.
fn json_size_within(v: &impl Serialize, cap: usize) -> Option<usize> {
    let mut m = Meter { n: 0, cap };
    serde_json::to_writer(&mut m, v).ok().map(|()| m.n)
}

/// Builds a page from `selected` (in key order, up to `limit + 1` items) within the byte budget.
fn page<T: Clone + Serialize>(
    selected: &[(OrderKey, &T)],
    limit: usize,
    binding: &Binding,
) -> Result<(Vec<T>, Option<String>), ApiError> {
    let budget = MAX_PAGE_BYTES - PAGE_ENVELOPE;
    let mut bytes = 0usize;
    let mut items = Vec::new();
    let mut last = None;
    let mut cut = false;
    for (k, item) in selected.iter().take(limit) {
        // One byte for the separating comma.
        match json_size_within(*item, budget.saturating_sub(bytes).saturating_sub(1)) {
            Some(n) => {
                bytes += n + 1;
                items.push((*item).clone());
                last = Some(k);
            }
            None if items.is_empty() => {
                return Err(ApiError::item_too_large(
                    "next_cursor",
                    json!(encode_cursor(binding, k)),
                ));
            }
            None => {
                cut = true;
                break;
            }
        }
    }
    let more = cut || selected.len() > limit;
    let next = more
        .then(|| last.map(|k| encode_cursor(binding, k)))
        .flatten();
    Ok((items, next))
}

fn deleted_flag(include_deleted: bool) -> &'static str {
    if include_deleted { "deleted" } else { "live" }
}

fn collection_key(c: &Collection) -> OrderKey {
    let mut k = OrderKey::new();
    k.push_str(c.position.to_string(), false)
        .push_str(c.id.to_string(), false);
    k
}

/// Collections, in display order.
#[utoipa::path(
    get, path = "/api/v1/collections", tag = "read", params(ReadParams),
    responses(
        (status = 200, body = CollectionPage),
        (status = 400, description = "Bad cursor, or a cursor of another query (`invalid_cursor`)", body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 503, description = "Busy; see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn collections(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiQuery(q): ApiQuery<ReadParams>,
) -> Result<Json<CollectionPage>, ApiError> {
    let binding = Binding::new(&["collections", deleted_flag(q.include_deleted)]);
    let after = decode_cursor(q.cursor.as_deref(), &binding)?;
    let limit = page_limit(q.limit);
    read(&app, cancel, move |s, cancel| {
        let visible = visibility::visible_collections(s, &auth.actor);
        let sel = select(
            s.collections.values(),
            |c| {
                live_or(s, q.include_deleted, c.id.into())
                    && visibility::collection_in(visible.as_ref(), c)
            },
            collection_key,
            after.as_ref(),
            limit + 1,
            cancel,
        )?;
        let (items, next_cursor) = page(&sel, limit, &binding)?;
        Ok(Json(CollectionPage { items, next_cursor }))
    })
    .await
}

/// Lists, grouped by collection and in display order.
#[utoipa::path(
    get, path = "/api/v1/lists", tag = "read", params(ReadParams),
    responses(
        (status = 200, body = ListPage),
        (status = 400, description = "Bad cursor, or a cursor of another query (`invalid_cursor`)", body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 503, description = "Busy; see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn lists(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiQuery(q): ApiQuery<ReadParams>,
) -> Result<Json<ListPage>, ApiError> {
    let binding = Binding::new(&["lists", deleted_flag(q.include_deleted)]);
    let after = decode_cursor(q.cursor.as_deref(), &binding)?;
    let limit = page_limit(q.limit);
    read(&app, cancel, move |s, cancel| {
        // Lists without a collection first, then by collection position, then list position.
        let key = |l: &List| {
            let mut k = OrderKey::new();
            match l.collection.and_then(|c| s.collections.get(&c)) {
                None => k.push_int(0, false).push_str("", false).push_str("", false),
                Some(c) => k
                    .push_int(1, false)
                    .push_str(c.position.to_string(), false)
                    .push_str(c.id.to_string(), false),
            };
            k.push_str(l.position.to_string(), false)
                .push_str(l.id.to_string(), false);
            k
        };
        let sel = select(
            s.lists.values(),
            |l| {
                live_or(s, q.include_deleted, l.id.into())
                    && visibility::list_entity_visible(s, &auth.actor, l)
            },
            key,
            after.as_ref(),
            limit + 1,
            cancel,
        )?;
        let (items, next_cursor) = page(&sel, limit, &binding)?;
        Ok(Json(ListPage { items, next_cursor }))
    })
    .await
}

fn task_key(t: &Task) -> OrderKey {
    let mut k = OrderKey::new();
    k.push_str(t.position.to_string(), false)
        .push_str(t.id.to_string(), false);
    k
}

/// Tasks of one list, in manual order.
#[utoipa::path(
    get, path = "/api/v1/lists/{id}/tasks", tag = "read",
    params(("id" = String, Path, format = Uuid, description = "List ID"), ListTasksParams),
    responses(
        (status = 200, body = TaskPage),
        (status = 400, description = "Bad cursor, or a cursor of another query (`invalid_cursor`)", body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 503, description = "Busy; see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn list_tasks(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiPath(id): ApiPath<ListId>,
    ApiQuery(q): ApiQuery<ListTasksParams>,
) -> Result<Json<TaskPage>, ApiError> {
    let done = match q.done {
        None => "any",
        Some(true) => "done",
        Some(false) => "open",
    };
    let binding = Binding::new(&[
        "list_tasks",
        &id.to_string(),
        deleted_flag(q.include_deleted),
        done,
    ]);
    let after = decode_cursor(q.cursor.as_deref(), &binding)?;
    let limit = page_limit(q.limit);
    read(&app, cancel, move |s, cancel| {
        if !s.lists.contains_key(&id) || !visibility::list_visible(s, &auth.actor, id) {
            return Err(ApiError::not_found(format!("list {id} not found")));
        }
        let sel = select(
            s.tasks.values(),
            |t| {
                t.list == id
                    && live_or(s, q.include_deleted, t.id.into())
                    && q.done.is_none_or(|d| d == t.is_done())
            },
            task_key,
            after.as_ref(),
            limit + 1,
            cancel,
        )?;
        let (items, next_cursor) = page(&sel, limit, &binding)?;
        Ok(Json(TaskPage { items, next_cursor }))
    })
    .await
}

/// Size of a parsed query: (nodes, terms).
fn complexity(q: &Query) -> (usize, usize) {
    fn walk(e: &Expr, nodes: &mut usize, terms: &mut usize) {
        *nodes += 1;
        match e {
            Expr::True => {}
            Expr::And(v) | Expr::Or(v) => v.iter().for_each(|x| walk(x, nodes, terms)),
            Expr::Not(x) => walk(x, nodes, terms),
            Expr::Pred(_) => *terms += 1,
        }
    }
    let (mut nodes, mut terms) = (q.sort.len(), 0);
    walk(&q.expr, &mut nodes, &mut terms);
    (nodes, terms)
}

fn too_complex(reason: &str, value: usize, max: usize) -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "query_too_complex",
        format!("the query has too many {reason} ({value}; at most {max})"),
    )
    .with_details(json!({ "limit": reason, "value": value, "max": max }))
}

/// Parses a query and enforces the complexity caps.
fn parse_query(text: &str) -> Result<Query, ApiError> {
    if text.len() > MAX_QUERY_BYTES {
        return Err(too_complex("bytes", text.len(), MAX_QUERY_BYTES));
    }
    let query = Query::parse(text).map_err(|e| ApiError::invalid_query(&e))?;
    let (nodes, terms) = complexity(&query);
    if terms > MAX_QUERY_TERMS {
        return Err(too_complex("terms", terms, MAX_QUERY_TERMS));
    }
    if nodes > MAX_QUERY_NODES {
        return Err(too_complex("nodes", nodes, MAX_QUERY_NODES));
    }
    Ok(query)
}

async fn run_query(
    app: &AppState,
    actor: Actor,
    cancel: Cancel,
    route: &str,
    text: &str,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<TaskPage, ApiError> {
    let query = parse_query(text)?;
    let ctx: EvalContext = app.eval_context();
    // "today" changes what matches, so a cursor is only good for the day (and offset) it was made on.
    let binding = Binding::new(&[
        route,
        &query.to_string(),
        &ctx.today.to_string(),
        &ctx.utc_offset_seconds().to_string(),
    ]);
    let after = decode_cursor(cursor, &binding)?;
    let limit = page_limit(limit);
    read(app, cancel, move |s, cancel| {
        // Building the step index and scanning a task's steps stop once the request is cancelled.
        let stop = || cancel.is_cancelled();
        let m = query
            .matcher_until(s, &ctx, &stop)
            .ok_or_else(ApiError::timeout)?;
        let sel = select(
            s.tasks.values(),
            |t| m.matches(t) && visibility::task_visible(s, &actor, t),
            |t| query.order_key(s, t),
            after.as_ref(),
            limit + 1,
            cancel,
        )?;
        let (items, next_cursor) = page(&sel, limit, &binding)?;
        Ok(TaskPage { items, next_cursor })
    })
    .await
}

/// Tasks matching a smart-list query.
#[utoipa::path(
    get, path = "/api/v1/tasks", tag = "read", params(TaskQueryParams),
    responses(
        (status = 200, body = TaskPage),
        (status = 400, description = "Bad cursor or a cursor of another query or day (`invalid_cursor`), or the query is too complex (`query_too_complex`)", body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 422, description = "The query does not parse", body = ErrorBody),
        (status = 503, description = "Busy; see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn query_tasks(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiQuery(q): ApiQuery<TaskQueryParams>,
) -> Result<Json<TaskPage>, ApiError> {
    run_query(
        &app,
        auth.actor,
        cancel,
        "tasks",
        q.query.as_deref().unwrap_or_default(),
        q.cursor.as_deref(),
        q.limit,
    )
    .await
    .map(Json)
}

/// At most [`MAX_DETAIL_CHILDREN`] of `items` (those `keep` accepts) in `key` order, and whether
/// there were more.
fn children<'a, T: Clone + 'a>(
    items: impl Iterator<Item = &'a T>,
    keep: impl Fn(&T) -> bool,
    key: impl Fn(&T) -> OrderKey,
    cancel: &Cancel,
) -> Result<(Vec<T>, bool), ApiError> {
    let sel = select(items, keep, key, None, MAX_DETAIL_CHILDREN + 1, cancel)?;
    let more = sel.len() > MAX_DETAIL_CHILDREN;
    Ok((
        sel.into_iter()
            .take(MAX_DETAIL_CHILDREN)
            .map(|(_, x)| x.clone())
            .collect(),
        more,
    ))
}

fn id_key(id: impl ToString) -> OrderKey {
    let mut k = OrderKey::new();
    k.push_str(id.to_string(), false);
    k
}

/// Keeps the leading `items` whose JSON fits in `left` bytes (plus a comma each), taking what they use
/// from `left`. True if any were dropped.
fn fit_within<T: Serialize>(items: &mut Vec<T>, left: &mut usize) -> bool {
    let mut keep = 0;
    for x in items.iter() {
        match json_size_within(x, left.saturating_sub(1)) {
            Some(n) => {
                *left -= n + 1;
                keep += 1;
            }
            None => break,
        }
    }
    let cut = keep < items.len();
    items.truncate(keep);
    cut
}

/// The task detail as JSON of at most `budget` bytes. Each kind of child is capped at
/// [`MAX_DETAIL_CHILDREN`], then children are kept (steps, links, attachments, reminders, tags, in
/// that order) while they fit; `truncated` reports any cut.
fn detail_bytes(
    s: &CoreState,
    id: todo_core::ids::TaskId,
    include_deleted: bool,
    cancel: &Cancel,
    budget: usize,
) -> Result<Vec<u8>, ApiError> {
    let task = s
        .tasks
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found(format!("task {id} not found")))?;
    let keep = |deleted: bool| include_deleted || !deleted;
    let (mut steps, more_steps) = children(
        s.steps.values(),
        |x| x.task == id && keep(x.deleted.is_some()),
        |x| {
            let mut k = OrderKey::new();
            k.push_str(x.position.to_string(), false)
                .push_str(x.id.to_string(), false);
            k
        },
        cancel,
    )?;
    let (mut links, more_links) = children(
        s.links.values(),
        |x| x.task == id && keep(x.deleted.is_some()),
        |x| id_key(x.id),
        cancel,
    )?;
    let (mut attachments, more_attachments) = children(
        s.attachments.values(),
        |x| x.task == id && keep(x.deleted.is_some()),
        |x| id_key(x.id),
        cancel,
    )?;
    let (mut reminders, more_reminders) = children(
        s.reminders.values(),
        |x| x.task == id && keep(x.deleted.is_some()),
        |x| {
            let mut k = OrderKey::new();
            k.push_int(x.at.as_second(), false)
                .push_int(i64::from(x.at.subsec_nanosecond()), false)
                .push_str(x.id.to_string(), false);
            k
        },
        cancel,
    )?;
    // The task's own order of tags.
    let mut tags: Vec<Tag> = task
        .tags
        .iter()
        .filter_map(|t| s.tags.get(t))
        .filter(|t| keep(t.deleted.is_some()))
        .take(MAX_DETAIL_CHILDREN + 1)
        .cloned()
        .collect();
    let more_tags = tags.len() > MAX_DETAIL_CHILDREN;
    tags.truncate(MAX_DETAIL_CHILDREN);

    // The envelope (field names, brackets, `truncated`) is far below the reserve.
    let mut left = budget.saturating_sub(PAGE_ENVELOPE);
    let task_bytes =
        json_size_within(&task, left).ok_or_else(|| ApiError::item_too_large("task", json!(id)))?;
    left -= task_bytes;
    let cut = fit_within(&mut steps, &mut left)
        | fit_within(&mut links, &mut left)
        | fit_within(&mut attachments, &mut left)
        | fit_within(&mut reminders, &mut left)
        | fit_within(&mut tags, &mut left);
    let detail = TaskDetail {
        truncated: cut
            || more_steps
            || more_links
            || more_attachments
            || more_reminders
            || more_tags,
        task,
        steps,
        links,
        attachments,
        reminders,
        tags,
    };
    serde_json::to_vec(&detail).map_err(|e| {
        tracing::error!(error = %e, "cannot encode a task detail");
        ApiError::internal()
    })
}

/// One task with its steps, links, attachments, reminders and tags. Each kind is capped at
/// [`MAX_DETAIL_CHILDREN`], and the whole response at [`MAX_PAGE_BYTES`]; `truncated` says if anything
/// was cut. The response is serialized inside the admitted read.
#[utoipa::path(
    get, path = "/api/v1/tasks/{id}", tag = "read",
    params(("id" = String, Path, format = Uuid, description = "Task ID"), IncludeDeleted),
    responses(
        (status = 200, body = TaskDetail),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 503, description = "Busy; see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn task_detail(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiPath(id): ApiPath<todo_core::ids::TaskId>,
    ApiQuery(q): ApiQuery<IncludeDeleted>,
) -> Result<Response, ApiError> {
    let body = read(&app, cancel, move |s, cancel| {
        if s.tasks
            .get(&id)
            .is_some_and(|t| !visibility::task_visible(s, &auth.actor, t))
        {
            return Err(ApiError::not_found(format!("task {id} not found")));
        }
        detail_bytes(s, id, q.include_deleted, cancel, MAX_PAGE_BYTES)
    })
    .await?;
    Ok((
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        body,
    )
        .into_response())
}

fn tag_key(t: &Tag) -> OrderKey {
    let mut k = OrderKey::new();
    k.push_str(fold(&t.name), false)
        .push_str(t.id.to_string(), false);
    k
}

/// Tags, by name.
#[utoipa::path(
    get, path = "/api/v1/tags", tag = "read", params(ReadParams),
    responses(
        (status = 200, body = TagPage),
        (status = 400, description = "Bad cursor, or a cursor of another query (`invalid_cursor`)", body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 503, description = "Busy; see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn tags(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiQuery(q): ApiQuery<ReadParams>,
) -> Result<Json<TagPage>, ApiError> {
    let binding = Binding::new(&["tags", deleted_flag(q.include_deleted)]);
    let after = decode_cursor(q.cursor.as_deref(), &binding)?;
    let limit = page_limit(q.limit);
    read(&app, cancel, move |s, cancel| {
        let visible = visibility::visible_tags(s, &auth.actor);
        let sel = select(
            s.tags.values(),
            |t| {
                (q.include_deleted || t.deleted.is_none())
                    && visibility::tag_in(visible.as_ref(), t)
            },
            tag_key,
            after.as_ref(),
            limit + 1,
            cancel,
        )?;
        let (items, next_cursor) = page(&sel, limit, &binding)?;
        Ok(Json(TagPage { items, next_cursor }))
    })
    .await
}

/// The built-in smart lists (a fixed handful, so not paginated).
#[utoipa::path(
    get, path = "/api/v1/smart-lists", tag = "read",
    responses((status = 200, body = Vec<SmartList>), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn smart_lists(_auth: Authenticated) -> Json<Vec<SmartList>> {
    Json(
        BUILTIN_LISTS
            .iter()
            .map(|b| SmartList {
                id: b.id.to_owned(),
                name: b.name.to_owned(),
                query: b.query.to_owned(),
            })
            .collect(),
    )
}

/// Tasks of a built-in smart list.
#[utoipa::path(
    get, path = "/api/v1/smart-lists/{id}/tasks", tag = "read",
    params(("id" = String, Path, description = "Smart list ID, for example `my_day`"), PageParams),
    responses(
        (status = 200, body = TaskPage),
        (status = 400, description = "Bad cursor, or a cursor of another list or day (`invalid_cursor`)", body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 503, description = "Busy; see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn smart_list_tasks(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiPath(id): ApiPath<String>,
    ApiQuery(q): ApiQuery<PageParams>,
) -> Result<Json<TaskPage>, ApiError> {
    let list = BUILTIN_LISTS
        .iter()
        .find(|b| b.id == id)
        .ok_or_else(|| ApiError::not_found(format!("smart list `{id}` not found")))?;
    run_query(
        &app,
        auth.actor,
        cancel,
        &format!("smart-lists/{}", list.id),
        list.query,
        q.cursor.as_deref(),
        q.limit,
    )
    .await
    .map(Json)
}

/// Most activity entries per page.
pub const MAX_ACTIVITY_PAGE: usize = 200;
/// Bytes of an activity page reserved per entry for its summary and flags.
const ACTIVITY_ITEM_RESERVE: usize = 2 * 1024;

/// The activity feed: events newest first, with summaries and undo state.
#[utoipa::path(
    get, path = "/api/v1/activity", tag = "read", params(ActivityParams),
    responses(
        (status = 200, body = ActivityPage),
        (status = 401, body = ErrorBody),
        (status = 500, description = "One event is larger than the page limit (`item_too_large`, with `next_before` to skip it)", body = ErrorBody),
        (status = 503, description = "Busy; see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn activity(
    auth: Authenticated,
    State(app): State<AppState>,
    ApiQuery(q): ApiQuery<ActivityParams>,
) -> Result<Json<ActivityPage>, ApiError> {
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_ACTIVITY_PAGE);
    let engine = app.engine.clone();
    // Event rows are sized in the database before they are read, so an oversized page is never
    // loaded; summaries get a fixed reserve per entry.
    let budget = MAX_PAGE_BYTES - PAGE_ENVELOPE - limit * ACTIVITY_ITEM_RESERVE;
    blocking(&app, Work::Read, move || {
        let actor = q.agent.map(|agent_id| Actor::Agent { agent_id });
        let unrestricted = visibility::sees_all(&auth.actor);
        // For scoped readers keep the original scan/cursor and apply the filter only to visible
        // items. Otherwise an empty page's cursor could identify an unseen event's actor.
        let query_actor = actor.as_ref().filter(|_| unrestricted);
        let page = engine.events_before_capped(q.before, limit, budget, query_actor)?;
        if let Some(seq) = page.oversized {
            tracing::warn!(seq, "an event is larger than the page limit");
            return Err(ApiError::item_too_large("next_before", json!(seq)));
        }
        let s = engine.state();
        let gone = engine.whereabouts();
        let j = &s.journal;
        let more = page.truncated || page.events.len() == limit;
        // The oldest event read (visible or not) is where the next page continues.
        let scanned = page.events.last().and_then(|e| e.event.seq);
        let scopes = page.scopes;
        let items: Vec<ActivityItem> = page
            .events
            .into_iter()
            .filter(|e| {
                let scope = e.event.seq.and_then(|seq| scopes.get(&seq));
                visibility::event_visible(&s, &gone, &auth.actor, &e.event, scope)
                    && actor.as_ref().is_none_or(|actor| actor == &e.event.actor)
            })
            .map(|mut event| {
                if !unrestricted {
                    // See `visibility::log_event_for`.
                    event.command_len = None;
                }
                let root = j.root_of(event.event.id);
                let head = j.head_of(root);
                let undone = if event.event.undoes.is_none() {
                    j.is_undone(event.event.id)
                } else {
                    head != event.event.id
                };
                let summary = summary::event(&s, &event.event);
                if !q.workflow_fields {
                    visibility::legacy_workflow_projection(&mut event.event);
                }
                ActivityItem {
                    summary,
                    undone,
                    can_undo: head == event.event.id,
                    event,
                }
            })
            .collect();
        let next_before = more.then_some(scanned).flatten();
        Ok(Json(ActivityPage { items, next_before }))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Keys are the first few characters (real keys are short: positions, IDs, folded titles).
    #[allow(clippy::ptr_arg)] // `select` passes `&T` with `T = String`.
    fn key_of(s: &String) -> OrderKey {
        let mut k = OrderKey::new();
        k.push_str(s.chars().take(8).collect::<String>(), false);
        k
    }

    proptest! {
        #[test]
        fn top_k_selection_is_the_sorted_prefix_after_the_cursor(
            items in proptest::collection::btree_set("[a-z]{1,4}", 0..60),
            after in proptest::option::of("[a-z]{1,4}"),
            n in 1usize..10,
        ) {
            let items: Vec<String> = items.into_iter().rev().collect();
            let after_key = after.as_ref().map(key_of);
            let got: Vec<&String> = select(items.iter(), |_| true, key_of, after_key.as_ref(), n, &Cancel::new())
                .unwrap()
                .into_iter()
                .map(|(_, x)| x)
                .collect();
            let mut want: Vec<&String> = items
                .iter()
                .filter(|x| after.as_ref().is_none_or(|a| *x > a))
                .collect();
            want.sort();
            want.truncate(n);
            prop_assert_eq!(got, want);
        }
    }

    #[test]
    fn a_scan_stops_once_the_request_is_cancelled() {
        let items: Vec<String> = (0..10_000).map(|i| format!("{i:05}")).collect();
        let cancel = Cancel::new();
        assert!(cancel.cancel());
        let err = select(items.iter(), |_| true, key_of, None, 10, &cancel)
            .err()
            .unwrap();
        assert_eq!(err.code, "timeout");
    }

    #[test]
    fn pages_stay_within_the_byte_limit_including_the_envelope() {
        let big: Vec<String> = (0..6)
            .map(|i| format!("{i}{}", "x".repeat(1024 * 1024)))
            .collect();
        let sel: Vec<(OrderKey, &String)> = big.iter().map(|s| (key_of(s), s)).collect();
        let binding = Binding::new(&["test"]);
        let (items, next) = page(&sel, 10, &binding).unwrap();
        assert_eq!(items.len(), 3);
        let next = next.unwrap();
        let body = serde_json::to_vec(&json!({ "items": items, "next_cursor": next })).unwrap();
        assert!(body.len() <= MAX_PAGE_BYTES, "{}", body.len());
        // The cursor continues after the last item returned.
        let k = decode_cursor(Some(&next), &binding).unwrap().unwrap();
        assert_eq!(k, key_of(&big[2]));
        assert!(decode_cursor(Some(&next), &Binding::new(&["other"])).is_err());
    }

    #[test]
    fn a_single_item_over_the_limit_is_refused_with_a_cursor_past_it() {
        let huge = "x".repeat(MAX_PAGE_BYTES);
        let sel = vec![(key_of(&huge), &huge)];
        let binding = Binding::new(&["test"]);
        let err = page(&sel, 10, &binding).err().unwrap();
        assert_eq!(err.code, "item_too_large");
        let next = err.details.unwrap()["next_cursor"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            decode_cursor(Some(&next), &binding).unwrap().unwrap(),
            key_of(&huge)
        );
    }

    #[test]
    fn task_detail_is_cut_to_its_byte_budget_with_the_flag_set() {
        use crate::engine::{Engine, system_clock};
        use todo_core::Actor;
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(&dir.path().join("db.sqlite3"), system_clock()).unwrap();
        let owner = Actor::owner("pc").unwrap();
        let run = |cmd: serde_json::Value| {
            engine
                .execute(
                    &owner,
                    uuid::Uuid::new_v4(),
                    &serde_json::from_value(cmd).unwrap(),
                )
                .unwrap();
        };
        let (list, task) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        run(json!({ "cmd": "create_list", "id": list, "name": "L" }));
        run(json!({ "cmd": "create_task", "id": task, "list": list, "title": "t" }));
        for i in 0..50 {
            run(json!({
                "cmd": "create_step", "id": uuid::Uuid::new_v4(), "task": task,
                "title": format!("{i:02}{}", "s".repeat(400)),
            }));
        }
        let state = engine.state();
        let id = serde_json::from_value(json!(task)).unwrap();
        let full: serde_json::Value = serde_json::from_slice(
            &detail_bytes(&state, id, false, &Cancel::new(), MAX_PAGE_BYTES).unwrap(),
        )
        .unwrap();
        assert_eq!(full["truncated"], false);
        assert_eq!(full["steps"].as_array().unwrap().len(), 50);

        let budget = PAGE_ENVELOPE + 8 * 1024;
        let body = detail_bytes(&state, id, false, &Cancel::new(), budget).unwrap();
        assert!(body.len() <= budget, "{} > {budget}", body.len());
        let cut: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(cut["truncated"], true);
        let steps = cut["steps"].as_array().unwrap();
        assert!(!steps.is_empty() && steps.len() < 50, "{}", steps.len());
        // What is kept is the leading part, in order.
        assert_eq!(steps[..], full["steps"].as_array().unwrap()[..steps.len()]);
        // The task itself must fit, or the read is refused.
        let err = detail_bytes(&state, id, false, &Cancel::new(), PAGE_ENVELOPE + 10).unwrap_err();
        assert_eq!(err.code, "item_too_large");
    }

    #[test]
    fn a_scan_cancelled_after_its_last_check_still_fails() {
        // Fewer items than one tick: only the final check can see the cancellation.
        let items: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let cancel = Cancel::new();
        let err = select(
            items.iter(),
            |_| {
                let _ = cancel.cancel();
                true
            },
            key_of,
            None,
            5,
            &cancel,
        )
        .err()
        .unwrap();
        assert_eq!(err.code, "timeout");
    }
}
