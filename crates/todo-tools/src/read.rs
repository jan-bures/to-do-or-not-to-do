//! Pure read helpers shared by REST/MCP and the scoped local replica.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{cmp::Ordering, collections::BinaryHeap};
use todo_core::model::{List, ListFeatures, Step};
use todo_core::query::{Expr, OrderKey, ParseError, Query};
use todo_core::state::State;

/// Default page size of collection reads.
pub const DEFAULT_PAGE: usize = 100;
/// Largest page size of collection reads.
pub const MAX_PAGE: usize = 500;
/// Most children in each section of a task detail.
pub const MAX_DETAIL_CHILDREN: usize = 200;

/// Longest accepted cursor, in bytes.
const MAX_CURSOR_BYTES: usize = 8 * 1024;
/// Items scanned between cancellation checks.
pub const TICK: usize = 256;
/// Query length and complexity caps.
pub const MAX_QUERY_BYTES: usize = 1024;
pub const MAX_QUERY_NODES: usize = 64;
pub const MAX_QUERY_TERMS: usize = 32;

#[must_use]
pub fn page_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the cursor is malformed or belongs to a different query; restart from the first page")]
pub struct InvalidCursor;

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("read canceled")]
pub struct Cancelled;

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("the query has too many {limit} ({value}; at most {max})")]
    TooComplex {
        limit: &'static str,
        value: usize,
        max: usize,
    },
}

/// What a cursor is bound to: a hash of the route and everything that selects or orders its items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding(String);

impl Binding {
    #[must_use]
    pub fn new(parts: &[&str]) -> Self {
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

#[must_use]
pub fn encode_cursor(binding: &Binding, key: &OrderKey) -> String {
    let data = CursorData {
        b: binding.0.clone(),
        k: key.clone(),
    };
    // Serializing strings and integers cannot fail.
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&data).unwrap_or_default())
}

/// Decodes a cursor and checks its binding, before any work is admitted.
pub fn decode_cursor(
    cursor: Option<&str>,
    binding: &Binding,
) -> Result<Option<OrderKey>, InvalidCursor> {
    let Some(c) = cursor else { return Ok(None) };
    if c.len() > MAX_CURSOR_BYTES {
        return Err(InvalidCursor);
    }
    let bytes = URL_SAFE_NO_PAD.decode(c).map_err(|_| InvalidCursor)?;
    let data: CursorData = serde_json::from_slice(&bytes).map_err(|_| InvalidCursor)?;
    if data.b != binding.0 {
        return Err(InvalidCursor);
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
/// order. One pass with a bounded max-heap; checks `stop` every [`TICK`] items.
pub fn select<'a, T>(
    items: impl Iterator<Item = &'a T>,
    keep: impl Fn(&T) -> bool,
    key: impl Fn(&T) -> OrderKey,
    after: Option<&OrderKey>,
    n: usize,
    stop: impl Fn() -> bool,
) -> Result<Vec<(OrderKey, &'a T)>, Cancelled>
where
    T: 'a,
{
    select_keyed(items, |item| keep(item).then(|| key(item)), after, n, stop)
}

pub(crate) fn select_keyed<'a, T>(
    items: impl Iterator<Item = &'a T>,
    key: impl Fn(&T) -> Option<OrderKey>,
    after: Option<&OrderKey>,
    n: usize,
    stop: impl Fn() -> bool,
) -> Result<Vec<(OrderKey, &'a T)>, Cancelled> {
    let mut heap: BinaryHeap<Entry<'a, T>> = BinaryHeap::with_capacity(n.saturating_add(1));
    for (i, item) in items.enumerate() {
        if i % TICK == 0 && stop() {
            return Err(Cancelled);
        }
        let Some(k) = key(item) else {
            continue;
        };
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
    if stop() {
        return Err(Cancelled);
    }
    Ok(heap
        .into_sorted_vec()
        .into_iter()
        .map(|e| (e.key, e.item))
        .collect())
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

/// Parses a query and enforces the complexity caps.
pub fn parse_query(text: &str) -> Result<Query, QueryError> {
    if text.len() > MAX_QUERY_BYTES {
        return Err(QueryError::TooComplex {
            limit: "bytes",
            value: text.len(),
            max: MAX_QUERY_BYTES,
        });
    }
    let query = Query::parse(text)?;
    let (nodes, terms) = complexity(&query);
    if terms > MAX_QUERY_TERMS {
        return Err(QueryError::TooComplex {
            limit: "terms",
            value: terms,
            max: MAX_QUERY_TERMS,
        });
    }
    if nodes > MAX_QUERY_NODES {
        return Err(QueryError::TooComplex {
            limit: "nodes",
            value: nodes,
            max: MAX_QUERY_NODES,
        });
    }
    Ok(query)
}

/// Lists without a collection first, then by collection position, then list position.
#[must_use]
pub fn list_key(state: &State, l: &List) -> OrderKey {
    let mut k = OrderKey::new();
    match l.collection.and_then(|c| state.collections.get(&c)) {
        None => k.push_int(0, false).push_str("", false).push_str("", false),
        Some(c) => k
            .push_int(1, false)
            .push_str(c.position.to_string(), false)
            .push_str(c.id.to_string(), false),
    };
    k.push_str(l.position.to_string(), false)
        .push_str(l.id.to_string(), false);
    k
}

/// Steps in checklist order.
#[must_use]
pub fn step_key(step: &Step) -> OrderKey {
    let mut key = OrderKey::new();
    key.push_str(step.position.to_string(), false)
        .push_str(step.id.to_string(), false);
    key
}

/// MCP presents the effective status and explicit defaults, rather than the stored open stage.
pub fn project_task(task: &mut Value) {
    if let Some(task) = task.as_object_mut() {
        if task.get("completed_at").is_some_and(|at| !at.is_null()) {
            task.insert("status".into(), json!("done"));
        } else {
            task.entry("status").or_insert(json!("todo"));
        }
        task.entry("start_date").or_insert(Value::Null);
    }
}

/// MCP includes explicit list defaults that are elided on the REST wire.
pub fn project_list(list: &mut Value) {
    if let Some(list) = list.as_object_mut() {
        list.entry("features")
            .or_insert(json!(ListFeatures::default()));
    }
}
