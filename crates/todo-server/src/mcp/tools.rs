//! The MCP tools.
//!
//! Each tool is a thin adapter over a REST handler: it builds the same request the REST client would
//! send and calls the handler function directly, with the caller and the request's [`Cancel`] that the
//! HTTP middleware attached. So admission permits, the write quota, the access checks in `todo-core`,
//! the read visibility filters, the query caps and pagination are the REST ones, by construction.
//!
//! | Tool | Command or endpoint | Level needed (agents) |
//! |---|---|---|
//! | `list_collections` | `GET /collections` | read |
//! | `list_lists` | `GET /lists` | read |
//! | `get_task` | `GET /tasks/{id}` | read |
//! | `query_tasks` | `GET /tasks?query=` | read |
//! | `create_task` | `create_task` | capture |
//! | `add_note` | `add_note` | capture |
//! | `update_task` | `update_task` | plan |
//! | `complete_task` | `complete_task` / `uncomplete_task` | plan |
//! | `move_task` | `move_task` | plan, on both lists |
//! | `add_step` | `create_step` | plan |
//! | `update_step` | `update_step`, `complete_step` / `uncomplete_step` | plan |
//! | `set_my_day` | `set_my_day` | plan |
//! | `merge_tasks` | `merge_tasks` | plan, on both tasks |
//! | `archive_task` | `delete` (a tombstone; undo restores it) | plan |
//! | `get_activity` | `GET /activity` | read (events filtered) |
//! | `undo_event` | `POST /events/{seq}/undo` | the level of the change; own events only |
//!
//! # Results and errors
//!
//! A successful tool returns the REST response body as structured content (and the same JSON as
//! text). A write returns `command_id`, `events` (`[{id, seq}]`), `summary` (the activity-feed line),
//! `replayed`, and the IDs of anything it created. A failure is a tool error (`isError: true`) whose
//! structured content is the REST error envelope, `{"error": {"code", "message", "details"}}`, with the
//! same codes (`forbidden`, `not_found`, `undo_conflict`, `quota_exceeded`, …) plus `retry_after` in
//! seconds where REST sends `Retry-After`.
//!
//! # Idempotency
//!
//! Every write takes an optional `command_id` (a UUID); without one, a fresh one is generated per call.
//! The IDs of entities a write creates are derived from the `command_id`, so retrying with the same
//! `command_id` and arguments sends exactly the same command and returns the stored result
//! (`replayed: true`) instead of writing twice. A tool that sends two commands (`update_step` with both
//! a title and a completion change) derives one command ID per part.

use crate::api::commands::{self, REPLAYED_HEADER};
use crate::api::dto::{
    ActivityParams, CommandRequest, IncludeDeleted, ReadParams, TaskQueryParams,
};
use crate::api::error::ApiError;
use crate::api::read::{self, MAX_PAGE_BYTES};
use crate::api::{ApiJson, ApiPath, ApiQuery, RequestCancel, Work, blocking, visibility};
use crate::app::AppState;
use crate::auth::Authenticated;
use crate::cancel::Cancel;
use crate::engine::CommandResult;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::Response;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::future::Future;
use todo_core::command::Command;
use todo_core::ids::{ListId, TaskId};
use todo_core::query::fold;
use uuid::Uuid;

/// What the server tells agents when they connect (the full guide is `docs/agents/`).
pub const INSTRUCTIONS: &str = "\
A personal to-do list. You act with the levels the owner granted your token, per list: read, capture \
(create tasks, add notes) or plan (change, complete, move, merge and archive tasks, steps, My Day). \
Lists you cannot read do not exist for you. Create tasks in the Inbox unless told otherwise. Before \
creating a task, query for an existing one (query_tasks) to avoid duplicates. Pass the same command_id \
when retrying a write, so it is not applied twice. Every write is attributed to you, shows in the \
owner's activity feed and can be undone.";

// ---- arguments --------------------------------------------------------------------------------------

/// Paging for collections and lists.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageArgs {
    /// Also return deleted (archived) items. Default false.
    #[serde(default)]
    pub include_deleted: bool,
    /// Page size, 1–500 (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// `next_cursor` from the previous page; omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// One task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetTaskArgs {
    /// The task's ID.
    pub task_id: Uuid,
    /// Also return it (and its steps) when deleted. Default false.
    #[serde(default)]
    pub include_deleted: bool,
}

/// A smart-list query.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryArgs {
    /// Query in the smart-list language, for example `list:inbox and not done`, `due<=today sort:due`,
    /// `priority>=high tag:work`, `my_day`, or free words matched against titles, notes and steps
    /// (`milk`). Empty matches every live task. At most 1024 bytes, 64 nodes and 32 terms.
    #[serde(default)]
    pub query: Option<String>,
    /// Page size, 1–500 (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// `next_cursor` from the previous page of the same query; omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Task priority.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PriorityArg {
    /// No priority.
    None,
    /// Low.
    Low,
    /// Normal.
    Normal,
    /// High (shows in "Important").
    High,
}

/// Where to put an item among its siblings.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PositionArg {
    /// Before all others.
    First,
    /// After all others (the default).
    #[default]
    Last,
}

impl PositionArg {
    fn placement(self) -> Value {
        match self {
            Self::First => json!({ "at": "first" }),
            Self::Last => json!({ "at": "last" }),
        }
    }
}

/// A new task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTaskArgs {
    /// Title (required, one line).
    pub title: String,
    /// The list to create it in. Omit to use the Inbox (the list named "Inbox" that you can see).
    #[serde(default)]
    pub list_id: Option<Uuid>,
    /// Notes (plain text or Markdown).
    #[serde(default)]
    pub notes: Option<String>,
    /// Due date, `YYYY-MM-DD`.
    #[serde(default)]
    pub due: Option<String>,
    /// Priority. Default none.
    #[serde(default)]
    pub priority: Option<PriorityArg>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Text to append to a task's notes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddNoteArgs {
    /// The task.
    pub task_id: Uuid,
    /// Text to append (on a new paragraph).
    pub text: String,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Changes to a task. Omitted fields are left alone.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// New title.
    #[serde(default)]
    pub title: Option<String>,
    /// New notes, replacing the old ones (use `add_note` to append instead).
    #[serde(default)]
    pub notes: Option<String>,
    /// New due date `YYYY-MM-DD`, or `null` to clear it.
    #[serde(default, deserialize_with = "double_option")]
    pub due: Option<Option<String>>,
    /// New priority.
    #[serde(default)]
    pub priority: Option<PriorityArg>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Complete or reopen a task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompleteTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// `true` (default) completes it; a repeating task then gets its next occurrence. `false` reopens it.
    #[serde(default = "yes")]
    pub completed: bool,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

fn yes() -> bool {
    true
}

/// Move a task to another list.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// The destination list.
    pub list_id: Uuid,
    /// Where in the destination list. Default last.
    #[serde(default)]
    pub position: PositionArg,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// A new checklist step.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddStepArgs {
    /// The task.
    pub task_id: Uuid,
    /// The step's title.
    pub title: String,
    /// Where among the task's steps. Default last.
    #[serde(default)]
    pub position: PositionArg,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Changes to a step. Give `title`, `completed`, or both.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateStepArgs {
    /// The step.
    pub step_id: Uuid,
    /// New title.
    #[serde(default)]
    pub title: Option<String>,
    /// `true` ticks the step off, `false` reopens it.
    #[serde(default)]
    pub completed: Option<bool>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Put a task in My Day, or take it out.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetMyDayArgs {
    /// The task.
    pub task_id: Uuid,
    /// `true` (default) adds it to My Day, `false` removes it.
    #[serde(default = "yes")]
    pub in_my_day: bool,
    /// The day, `YYYY-MM-DD`. Default: today in the owner's time zone. Ignored when removing.
    #[serde(default)]
    pub date: Option<String>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Merge a duplicate into another task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeTasksArgs {
    /// The duplicate. Its steps, links, attachments, tags and notes move to the target, and it is
    /// deleted.
    pub source_task_id: Uuid,
    /// The task that is kept.
    pub target_task_id: Uuid,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// One task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Paging for the activity feed.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivityArgs {
    /// Only events with `seq` below this (the previous page's `next_before`). Default: the newest.
    #[serde(default)]
    pub before: Option<u64>,
    /// Page size, 1–200 (default 50).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Undo one change.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoArgs {
    /// `seq` of any event of the change (from a write result or `get_activity`). Undoing an undo
    /// redoes the change.
    pub seq: u64,
    /// Undo even though later changes touched the same fields (only after an `undo_conflict` you have
    /// looked at). Default false.
    #[serde(default)]
    pub force: bool,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact undo.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

// ---- plumbing ---------------------------------------------------------------------------------------

/// The caller and the request's cancellation, attached by the HTTP middleware.
fn caller(ctx: &RequestContext<RoleServer>) -> Result<(Authenticated, Cancel), ApiError> {
    let parts = ctx
        .extensions
        .get::<Parts>()
        .ok_or_else(ApiError::unauthorized)?;
    let auth = parts
        .extensions
        .get::<Authenticated>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    let cancel = parts
        .extensions
        .get::<Cancel>()
        .cloned()
        .unwrap_or_default();
    Ok((auth, cancel))
}

/// The REST error envelope, plus `retry_after`.
fn error_body(e: &ApiError) -> Value {
    let mut err = json!({ "code": e.code, "message": e.message });
    if let Some(d) = &e.details {
        err["details"] = d.clone();
    }
    if let Some(s) = e.retry_after {
        err["retry_after"] = json!(s);
    }
    json!({ "error": err })
}

fn tool_result(tool: &'static str, r: Result<Value, ApiError>) -> CallToolResult {
    match r {
        Ok(v) => {
            tracing::debug!(tool, "mcp tool call succeeded");
            let mut res = CallToolResult::success(vec![ContentBlock::text(v.to_string())]);
            res.structured_content = Some(v);
            res
        }
        Err(e) => {
            tracing::debug!(tool, code = e.code, "mcp tool call failed");
            let body = error_body(&e);
            let mut res = CallToolResult::error(vec![ContentBlock::text(body.to_string())]);
            res.structured_content = Some(body);
            res
        }
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_body", message)
}

fn to_value(v: &impl Serialize) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| {
        tracing::error!(error = %e, "cannot encode a tool result");
        ApiError::internal()
    })
}

async fn body_json(res: Response) -> Result<Value, ApiError> {
    // The handlers already cap their bodies at MAX_PAGE_BYTES.
    let bytes = axum::body::to_bytes(res.into_body(), MAX_PAGE_BYTES + 1024)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "cannot read a handler response");
            ApiError::internal()
        })?;
    serde_json::from_slice(&bytes).map_err(|e| {
        tracing::error!(error = %e, "a handler response is not JSON");
        ApiError::internal()
    })
}

/// An ID derived from a command ID, so a retry recreates the same entity.
#[must_use]
pub fn derived_id(command_id: Uuid, what: &str) -> Uuid {
    let mut h = Sha256::new();
    h.update(b"todo-mcp\x1f");
    h.update(command_id.as_bytes());
    h.update([0x1f]);
    h.update(what.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    Uuid::new_v8(b)
}

/// A write's result: the stored command result, whether it was replayed, and `extra` fields.
async fn outcome(res: Response, extra: Value) -> Result<Value, ApiError> {
    let replayed = res.headers().contains_key(REPLAYED_HEADER);
    let v = body_json(res).await?;
    let r: CommandResult = serde_json::from_value(v).map_err(|e| {
        tracing::error!(error = %e, "unexpected command result");
        ApiError::internal()
    })?;
    let mut out = json!({
        "command_id": r.command_id,
        "events": r.events,
        "summary": r.summary,
        "replayed": replayed,
    });
    if let (Value::Object(o), Value::Object(e)) = (&mut out, extra) {
        o.extend(e);
    }
    Ok(out)
}

/// Runs one command through `POST /api/v1/commands`.
async fn command(
    app: &AppState,
    auth: Authenticated,
    cancel: Cancel,
    command_id: Uuid,
    command: Value,
    extra: Value,
) -> Result<Value, ApiError> {
    let command: Command =
        serde_json::from_value(command).map_err(|e| invalid(format!("invalid command: {e}")))?;
    let res = commands::post_command(
        auth,
        State(app.clone()),
        RequestCancel(cancel),
        ApiJson(CommandRequest {
            command_id,
            command,
        }),
    )
    .await?;
    outcome(res, extra).await
}

/// The list named "Inbox" (case- and diacritic-insensitively) that the caller can read: a top-level one
/// first, then by position.
async fn inbox(app: &AppState, auth: &Authenticated) -> Result<ListId, ApiError> {
    let engine = app.engine.clone();
    let actor = auth.actor.clone();
    blocking(app, Work::Read, move || {
        let s = engine.state();
        s.lists
            .values()
            .filter(|l| {
                s.is_live(l.id.into())
                    && fold(&l.name) == "inbox"
                    && visibility::list_visible(&s, &actor, l.id)
            })
            .min_by_key(|l| (l.collection.is_some(), l.position.to_string(), l.id))
            .map(|l| l.id)
    })
    .await?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "inbox_not_found",
            "no list named Inbox is visible to you; pass list_id (see list_lists)",
        )
    })
}

// ---- the server ---------------------------------------------------------------------------------------

/// The MCP server handler: one per request (the transport is stateless), cloned from a prototype.
#[derive(Clone)]
pub struct TodoMcp {
    app: AppState,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for TodoMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TodoMcp").finish_non_exhaustive()
    }
}

impl TodoMcp {
    /// A handler over `app`.
    #[must_use]
    pub fn new(app: AppState) -> Self {
        Self {
            app,
            tool_router: Self::tool_router(),
        }
    }

    /// The names of all tools.
    #[must_use]
    pub fn tool_names() -> Vec<String> {
        let mut v: Vec<String> = Self::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        v.sort();
        v
    }

    /// Runs a tool body with the caller and turns the outcome into a tool result.
    async fn run<F, Fut>(
        &self,
        tool: &'static str,
        ctx: &RequestContext<RoleServer>,
        f: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        F: FnOnce(AppState, Authenticated, Cancel) -> Fut,
        Fut: Future<Output = Result<Value, ApiError>>,
    {
        let r = match caller(ctx) {
            Ok((auth, cancel)) => f(self.app.clone(), auth, cancel).await,
            Err(e) => Err(e),
        };
        Ok(tool_result(tool, r))
    }
}

#[tool_router]
impl TodoMcp {
    #[tool(
        description = "List the collections (groups of lists) you can see, in display order. Paginated: pass next_cursor back as cursor.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_collections(
        &self,
        Parameters(a): Parameters<PageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("list_collections", &ctx, |app, auth, cancel| async move {
            let page = read::collections(
                auth,
                State(app),
                RequestCancel(cancel),
                ApiQuery(ReadParams {
                    include_deleted: a.include_deleted,
                    limit: a.limit,
                    cursor: a.cursor,
                }),
            )
            .await?;
            to_value(&page.0)
        })
        .await
    }

    #[tool(
        description = "List the lists you can see (with their IDs and collections), in display order. Paginated: pass next_cursor back as cursor.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_lists(
        &self,
        Parameters(a): Parameters<PageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("list_lists", &ctx, |app, auth, cancel| async move {
            let page = read::lists(
                auth,
                State(app),
                RequestCancel(cancel),
                ApiQuery(ReadParams {
                    include_deleted: a.include_deleted,
                    limit: a.limit,
                    cursor: a.cursor,
                }),
            )
            .await?;
            to_value(&page.0)
        })
        .await
    }

    #[tool(
        description = "Get one task with its steps, links, attachments, reminders and tags.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_task(
        &self,
        Parameters(a): Parameters<GetTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("get_task", &ctx, |app, auth, cancel| async move {
            let res = read::task_detail(
                auth,
                State(app),
                RequestCancel(cancel),
                ApiPath(TaskId::from_uuid(a.task_id)),
                ApiQuery(IncludeDeleted {
                    include_deleted: a.include_deleted,
                }),
            )
            .await?;
            body_json(res).await
        })
        .await
    }

    #[tool(
        description = "Find tasks with a smart-list query (see the query argument), across the lists you can see. Use it before create_task to avoid duplicates. Paginated: pass next_cursor back as cursor with the same query.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn query_tasks(
        &self,
        Parameters(a): Parameters<QueryArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("query_tasks", &ctx, |app, auth, cancel| async move {
            let page = read::query_tasks(
                auth,
                State(app),
                RequestCancel(cancel),
                ApiQuery(TaskQueryParams {
                    query: a.query,
                    limit: a.limit,
                    cursor: a.cursor,
                }),
            )
            .await?;
            to_value(&page.0)
        })
        .await
    }

    #[tool(
        description = "Create a task (capture level). Goes to the Inbox unless list_id is given. Returns task_id and the event. Query first to avoid duplicates; retry with the same command_id.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_task(
        &self,
        Parameters(a): Parameters<CreateTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("create_task", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let list = match a.list_id {
                Some(l) => ListId::from_uuid(l),
                None => inbox(&app, &auth).await?,
            };
            let task = TaskId::from_uuid(derived_id(command_id, "task"));
            let mut cmd =
                json!({ "cmd": "create_task", "id": task, "list": list, "title": a.title });
            if let Some(n) = a.notes {
                cmd["notes"] = json!(n);
            }
            if let Some(d) = a.due {
                cmd["due"] = json!(d);
            }
            if let Some(p) = a.priority {
                cmd["priority"] = json!(p);
            }
            let extra = json!({ "task_id": task, "list_id": list });
            command(&app, auth, cancel, command_id, cmd, extra).await
        })
        .await
    }

    #[tool(
        description = "Append text to a task's notes (capture level).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn add_note(
        &self,
        Parameters(a): Parameters<AddNoteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("add_note", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let cmd = json!({ "cmd": "add_note", "task": a.task_id, "text": a.text });
            command(&app, auth, cancel, command_id, cmd, json!({})).await
        })
        .await
    }

    #[tool(
        description = "Change a task's title, notes, due date or priority (plan level). Omitted fields stay as they are; due: null clears the due date.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn update_task(
        &self,
        Parameters(a): Parameters<UpdateTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("update_task", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let mut patch = serde_json::Map::new();
            if let Some(t) = a.title {
                patch.insert("title".into(), json!(t));
            }
            if let Some(n) = a.notes {
                patch.insert("notes".into(), json!(n));
            }
            if let Some(d) = a.due {
                patch.insert("due".into(), json!(d));
            }
            if let Some(p) = a.priority {
                patch.insert("priority".into(), json!(p));
            }
            if patch.is_empty() {
                return Err(invalid(
                    "nothing to change: give title, notes, due or priority",
                ));
            }
            let cmd = json!({ "cmd": "update_task", "id": a.task_id, "patch": patch });
            command(&app, auth, cancel, command_id, cmd, json!({})).await
        })
        .await
    }

    #[tool(
        description = "Complete a task, or reopen it with completed: false (plan level). Completing a repeating task creates its next occurrence.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn complete_task(
        &self,
        Parameters(a): Parameters<CompleteTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("complete_task", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let cmd = if a.completed {
                "complete_task"
            } else {
                "uncomplete_task"
            };
            let cmd = json!({ "cmd": cmd, "id": a.task_id });
            command(&app, auth, cancel, command_id, cmd, json!({})).await
        })
        .await
    }

    #[tool(
        description = "Move a task to another list (plan level on both lists).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn move_task(
        &self,
        Parameters(a): Parameters<MoveTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("move_task", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let cmd = json!({
                "cmd": "move_task", "id": a.task_id, "list": a.list_id,
                "placement": a.position.placement(),
            });
            command(&app, auth, cancel, command_id, cmd, json!({})).await
        })
        .await
    }

    #[tool(
        description = "Add a checklist step to a task (plan level). Returns step_id.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn add_step(
        &self,
        Parameters(a): Parameters<AddStepArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("add_step", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let step = derived_id(command_id, "step");
            let cmd = json!({
                "cmd": "create_step", "id": step, "task": a.task_id, "title": a.title,
                "placement": a.position.placement(),
            });
            command(
                &app,
                auth,
                cancel,
                command_id,
                cmd,
                json!({ "step_id": step }),
            )
            .await
        })
        .await
    }

    #[tool(
        description = "Retitle a step and/or tick it off (completed: true) or reopen it (completed: false) (plan level).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn update_step(
        &self,
        Parameters(a): Parameters<UpdateStepArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("update_step", &ctx, |app, auth, cancel| async move {
            let base = a.command_id.unwrap_or_else(Uuid::new_v4);
            let mut parts = Vec::new();
            if let Some(t) = a.title {
                parts.push(json!({ "cmd": "update_step", "id": a.step_id, "title": t }));
            }
            if let Some(done) = a.completed {
                let cmd = if done {
                    "complete_step"
                } else {
                    "uncomplete_step"
                };
                parts.push(json!({ "cmd": cmd, "id": a.step_id }));
            }
            match parts.len() {
                0 => Err(invalid("nothing to change: give title, completed or both")),
                1 => {
                    let cmd = parts.remove(0);
                    command(&app, auth, cancel, base, cmd, json!({})).await
                }
                _ => {
                    // Two commands: one derived ID each, so a retry replays both.
                    let mut results = Vec::new();
                    for (i, cmd) in parts.into_iter().enumerate() {
                        let id = derived_id(base, &format!("part{i}"));
                        match command(&app, auth.clone(), cancel.clone(), id, cmd, json!({})).await
                        {
                            Ok(v) => results.push(v),
                            Err(mut e) if !results.is_empty() => {
                                let mut d = e.details.take().unwrap_or_else(|| json!({}));
                                if let Value::Object(m) = &mut d {
                                    m.insert("applied".into(), json!(results));
                                }
                                return Err(e.with_details(d));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    let events: Vec<Value> = results
                        .iter()
                        .flat_map(|r| r["events"].as_array().cloned().unwrap_or_default())
                        .collect();
                    let summary: Vec<&str> = results
                        .iter()
                        .filter_map(|r| r["summary"].as_str())
                        .collect();
                    Ok(json!({
                        "command_id": base,
                        "events": events,
                        "summary": summary.join("; "),
                        "replayed": results.iter().all(|r| r["replayed"] == true),
                        "parts": results,
                    }))
                }
            }
        })
        .await
    }

    #[tool(
        description = "Put a task in My Day (today by default) or take it out with in_my_day: false (plan level).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn set_my_day(
        &self,
        Parameters(a): Parameters<SetMyDayArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("set_my_day", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let date = if a.in_my_day {
                match a.date {
                    Some(d) => json!(d),
                    None => json!(app.eval_context().today),
                }
            } else {
                Value::Null
            };
            let cmd = json!({ "cmd": "set_my_day", "id": a.task_id, "date": date });
            command(&app, auth, cancel, command_id, cmd, json!({ "date": date })).await
        })
        .await
    }

    #[tool(
        description = "Merge a duplicate task (source) into the one to keep (target) (plan level on both). The source's steps, links, attachments, tags and notes move over and the source is deleted.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = false
        )
    )]
    async fn merge_tasks(
        &self,
        Parameters(a): Parameters<MergeTasksArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("merge_tasks", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let cmd = json!({
                "cmd": "merge_tasks", "source": a.source_task_id, "target": a.target_task_id,
            });
            command(&app, auth, cancel, command_id, cmd, json!({})).await
        })
        .await
    }

    #[tool(
        description = "Archive a task (plan level): it leaves its list and all queries but is kept, and undo_event brings it back.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = false
        )
    )]
    async fn archive_task(
        &self,
        Parameters(a): Parameters<ArchiveTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("archive_task", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let cmd = json!({ "cmd": "delete", "id": { "kind": "task", "id": a.task_id } });
            command(&app, auth, cancel, command_id, cmd, json!({})).await
        })
        .await
    }

    #[tool(
        description = "Recent changes, newest first, with one-line summaries, who made them and whether they can be undone. Only changes in lists you can see. Paginated: pass next_before back as before.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_activity(
        &self,
        Parameters(a): Parameters<ActivityArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("get_activity", &ctx, |app, auth, _cancel| async move {
            let page = read::activity(
                auth,
                State(app),
                ApiQuery(ActivityParams {
                    before: a.before,
                    limit: a.limit,
                }),
            )
            .await?;
            to_value(&page.0)
        })
        .await
    }

    #[tool(
        description = "Undo one of your own changes by the seq of any of its events (undoing an undo redoes it). Refused with undo_conflict if later changes touched the same fields; look at them before retrying with force: true.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = false
        )
    )]
    async fn undo_event(
        &self,
        Parameters(a): Parameters<UndoArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("undo_event", &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let body = serde_json::to_vec(&json!({ "command_id": command_id, "force": a.force }))
                .map_err(|_| ApiError::internal())?;
            let res = commands::post_undo(
                auth,
                State(app),
                RequestCancel(cancel),
                ApiPath(a.seq),
                Bytes::from(body),
            )
            .await?;
            outcome(res, json!({})).await
        })
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TodoMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("todo", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_are_stable_and_distinct() {
        let c = Uuid::new_v4();
        assert_eq!(derived_id(c, "task"), derived_id(c, "task"));
        assert_ne!(derived_id(c, "task"), derived_id(c, "step"));
        assert_ne!(derived_id(c, "task"), derived_id(Uuid::new_v4(), "task"));
    }

    #[test]
    fn errors_carry_the_rest_envelope() {
        let e = ApiError::quota_exceeded(7);
        let r = tool_result("t", Err(e));
        assert_eq!(r.is_error, Some(true));
        let body = r.structured_content.unwrap();
        assert_eq!(body["error"]["code"], "quota_exceeded");
        assert_eq!(body["error"]["retry_after"], 7);
    }

    #[test]
    fn every_tool_has_a_description_and_an_object_schema() {
        let names = TodoMcp::tool_names();
        assert_eq!(names.len(), 16, "{names:?}");
        for t in TodoMcp::tool_router().list_all() {
            assert!(
                t.description.as_deref().is_some_and(|d| d.len() > 20),
                "{}",
                t.name
            );
            assert_eq!(
                t.input_schema.get("type"),
                Some(&json!("object")),
                "{}",
                t.name
            );
        }
    }
}
