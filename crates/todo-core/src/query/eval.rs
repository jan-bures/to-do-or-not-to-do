//! Query evaluation over the in-memory state.

use super::ast::{ActorSpec, DateExpr, DateField, DueState, Expr, Predicate, Query, SortKey};
use super::fold::fold;
use crate::actor::Actor;
use crate::ids::TaskId;
use crate::model::{Priority, Task};
use crate::state::State;
use crate::time::{Date, Timestamp, add_days, utc_date};
use serde::{Deserialize, Serialize};
use std::cell::OnceCell;
use std::cmp::Ordering;
use std::collections::HashMap;

/// What "now" and "today" mean for a query run.
///
/// The core has no clock or time-zone database, so the caller supplies both. `today` is the owner's local
/// date (My Day rolls over at local midnight); `utc_offset_seconds` is the owner's current UTC offset and
/// is used to turn timestamps (created, updated, completed) into local dates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvalContext {
    /// The current time.
    pub now: Timestamp,
    /// The owner's local date.
    pub today: Date,
    /// The owner's UTC offset in seconds (0 for UTC).
    pub utc_offset_seconds: i32,
}

impl EvalContext {
    /// A context in UTC: `today` is the UTC date of `now`.
    #[must_use]
    pub fn utc(now: Timestamp) -> Self {
        Self {
            now,
            today: utc_date(now),
            utc_offset_seconds: 0,
        }
    }

    /// A context with an explicit local date and UTC offset.
    #[must_use]
    pub fn new(now: Timestamp, today: Date, utc_offset_seconds: i32) -> Self {
        Self {
            now,
            today,
            utc_offset_seconds,
        }
    }

    /// The owner's local date of `ts`.
    #[must_use]
    pub fn local_date(&self, ts: Timestamp) -> Date {
        let shifted = Timestamp::from_second(ts.as_second() + i64::from(self.utc_offset_seconds));
        utc_date(shifted.unwrap_or(ts))
    }

    /// The calendar date `d` stands for (`None` if it overflows the calendar).
    #[must_use]
    pub fn resolve(&self, d: DateExpr) -> Option<Date> {
        match d {
            DateExpr::Today => Some(self.today),
            DateExpr::Tomorrow => add_days(self.today, 1),
            DateExpr::Yesterday => add_days(self.today, -1),
            DateExpr::Offset(n) => add_days(self.today, i64::from(n)),
            DateExpr::Absolute(d) => Some(d),
        }
    }
}

fn actor_matches(spec: &ActorSpec, actor: &Actor) -> bool {
    match (spec, actor) {
        (ActorSpec::Owner, Actor::Owner { .. })
        | (ActorSpec::AnyAgent, Actor::Agent { .. })
        | (ActorSpec::Importer, Actor::Importer)
        | (ActorSpec::EmailIngest, Actor::EmailIngest)
        | (ActorSpec::System, Actor::System) => true,
        (ActorSpec::Agent(a), Actor::Agent { agent_id }) => a == agent_id,
        _ => false,
    }
}

fn name_matches(name: &str, id: &str, wanted: &str) -> bool {
    id == wanted || fold(name) == fold(wanted)
}

/// Folded titles of the live steps of each task, built once per [`Query::run`] so that free-text terms
/// do not scan every step for every task.
type StepIndex = HashMap<TaskId, Vec<String>>;

fn step_index(state: &State, stop: Option<Stop<'_>>) -> Option<StepIndex> {
    let mut idx = StepIndex::new();
    for (i, s) in state.steps.values().enumerate() {
        if i % CHECK_EVERY == 0 && stop.is_some_and(|f| f()) {
            return None;
        }
        if s.deleted.is_none() {
            idx.entry(s.task).or_default().push(fold(&s.title));
        }
    }
    Some(idx)
}

/// A cancellation check: true when the caller wants evaluation to stop.
pub type Stop<'a> = &'a dyn Fn() -> bool;

/// Steps scanned between two calls of a [`Stop`] check.
const CHECK_EVERY: usize = 256;

/// What one evaluation run shares across tasks.
struct Env<'a> {
    state: &'a State,
    ctx: &'a EvalContext,
    steps: Option<&'a StepIndex>,
    stop: Option<Stop<'a>>,
}

/// A task's folded text, computed at most once per task and run.
struct FoldedText {
    title: String,
    notes: String,
    /// Folded step titles; `None` when they come from the run's [`StepIndex`].
    steps: Option<Vec<String>>,
}

impl FoldedText {
    fn new(env: &Env<'_>, task: &Task) -> Self {
        let steps = match env.steps {
            Some(_) => None,
            None => Some(
                env.state
                    .steps
                    .values()
                    .filter(|s| s.task == task.id && s.deleted.is_none())
                    .map(|s| fold(&s.title))
                    .collect(),
            ),
        };
        Self {
            title: fold(&task.title),
            notes: fold(&task.notes),
            steps,
        }
    }

    fn contains(&self, env: &Env<'_>, task: TaskId, needle: &str) -> bool {
        let steps = match (&self.steps, env.steps) {
            (Some(own), _) => own.as_slice(),
            (None, Some(idx)) => idx.get(&task).map_or(&[][..], Vec::as_slice),
            (None, None) => &[],
        };
        if self.title.contains(needle) || self.notes.contains(needle) {
            return true;
        }
        // One task can have many steps: check for cancellation while scanning them. A stopped scan
        // reports no match; the caller sees the cancellation and discards the result.
        for (i, s) in steps.iter().enumerate() {
            if i % CHECK_EVERY == 0 && env.stop.is_some_and(|f| f()) {
                return false;
            }
            if s.contains(needle) {
                return true;
            }
        }
        false
    }
}

impl Predicate {
    /// Tests the predicate against a task.
    #[must_use]
    pub fn matches(&self, state: &State, task: &Task, ctx: &EvalContext) -> bool {
        let env = Env {
            state,
            ctx,
            steps: None,
            stop: None,
        };
        self.eval(&env, task, &OnceCell::new())
    }

    fn eval(&self, env: &Env<'_>, task: &Task, text: &OnceCell<FoldedText>) -> bool {
        let (state, ctx) = (env.state, env.ctx);
        match self {
            Self::List(v) => state
                .lists
                .get(&task.list)
                .is_some_and(|l| name_matches(&l.name, &l.id.to_string(), v)),
            Self::Collection(v) => state
                .lists
                .get(&task.list)
                .and_then(|l| l.collection)
                .and_then(|c| state.collections.get(&c))
                .is_some_and(|c| name_matches(&c.name, &c.id.to_string(), v)),
            Self::DueState(s) => match s {
                DueState::Overdue => !task.is_done() && task.due.is_some_and(|d| d < ctx.today),
                DueState::None => task.due.is_none(),
                DueState::Any => task.due.is_some(),
            },
            Self::Date { field, op, value } => {
                let lhs = match field {
                    DateField::Due => task.due,
                    DateField::Created => Some(ctx.local_date(task.created.at)),
                    DateField::Updated => Some(ctx.local_date(task.updated.at)),
                    DateField::Completed => task.completed_at.map(|t| ctx.local_date(t)),
                };
                match (lhs, ctx.resolve(*value)) {
                    (Some(l), Some(r)) => op.test(l.cmp(&r)),
                    _ => false,
                }
            }
            Self::MyDay => task.my_day == Some(ctx.today),
            Self::Priority { op, level } => op.test(task.priority.cmp(level)),
            Self::Tag(v) => task.tags.iter().any(|t| {
                state.tags.get(t).is_some_and(|g| {
                    g.deleted.is_none() && name_matches(&g.name, &g.id.to_string(), v)
                })
            }),
            Self::Done => task.is_done(),
            Self::CreatedBy(s) => actor_matches(s, &task.created.by),
            Self::UpdatedBy(s) => actor_matches(s, &task.updated.by),
            Self::Text(w) => {
                text.get_or_init(|| FoldedText::new(env, task))
                    .contains(env, task.id, &fold(w))
            }
        }
    }
}

impl Expr {
    /// Tests the expression against a task.
    #[must_use]
    pub fn matches(&self, state: &State, task: &Task, ctx: &EvalContext) -> bool {
        let env = Env {
            state,
            ctx,
            steps: None,
            stop: None,
        };
        self.eval(&env, task, &OnceCell::new())
    }

    fn eval(&self, env: &Env<'_>, task: &Task, text: &OnceCell<FoldedText>) -> bool {
        match self {
            Self::True => true,
            Self::And(v) => v.iter().all(|e| e.eval(env, task, text)),
            Self::Or(v) => v.iter().any(|e| e.eval(env, task, text)),
            Self::Not(e) => !e.eval(env, task, text),
            Self::Pred(p) => p.eval(env, task, text),
        }
    }

    fn has_text(&self) -> bool {
        match self {
            Self::True => false,
            Self::And(v) | Self::Or(v) => v.iter().any(Self::has_text),
            Self::Not(e) => e.has_text(),
            Self::Pred(p) => matches!(p, Predicate::Text(_)),
        }
    }
}

/// One component of an [`OrderKey`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyValue {
    /// An integer.
    Int(i64),
    /// A string, compared byte-wise.
    Str(String),
}

/// A key component and whether it sorts descending.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyPart(pub KeyValue, pub bool);

impl Ord for KeyPart {
    fn cmp(&self, other: &Self) -> Ordering {
        let o = self.0.cmp(&other.0);
        if self.1 || other.1 { o.reverse() } else { o }
    }
}

impl PartialOrd for KeyPart {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A comparable, serialisable sort key. For tasks, [`Query::order_key`] produces keys whose order *is*
/// the order of [`Query::run`] (keys end with the task ID, so they are unique). Callers can build keys
/// for their own orders with the `push_*` methods. Only compare keys built the same way.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrderKey(pub Vec<KeyPart>);

impl OrderKey {
    /// An empty key.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an integer.
    pub fn push_int(&mut self, v: i64, descending: bool) -> &mut Self {
        self.0.push(KeyPart(KeyValue::Int(v), descending));
        self
    }

    /// Appends a string.
    pub fn push_str(&mut self, v: impl Into<String>, descending: bool) -> &mut Self {
        self.0.push(KeyPart(KeyValue::Str(v.into()), descending));
        self
    }

    /// Appends an optional value that sorts after every present value (before them when descending).
    fn push_opt(&mut self, v: Option<KeyValue>, descending: bool) {
        self.push_int(i64::from(v.is_none()), descending);
        self.0
            .push(KeyPart(v.unwrap_or(KeyValue::Int(0)), descending));
    }

    fn push_ts(&mut self, ts: Timestamp, descending: bool) {
        self.push_int(ts.as_second(), descending);
        self.push_int(i64::from(ts.subsec_nanosecond()), descending);
    }

    fn push_opt_ts(&mut self, ts: Option<Timestamp>, descending: bool) {
        self.push_int(i64::from(ts.is_none()), descending);
        self.push_ts(ts.unwrap_or(Timestamp::UNIX_EPOCH), descending);
    }
}

fn date_value(d: Date) -> KeyValue {
    KeyValue::Int(i64::from(d.year()) * 10_000 + i64::from(d.month()) * 100 + i64::from(d.day()))
}

fn priority_rank(p: Priority) -> i64 {
    match p {
        Priority::None => 0,
        Priority::Low => 1,
        Priority::Normal => 2,
        Priority::High => 3,
    }
}

/// A query prepared against one state and context. Tests tasks one at a time, with the same step index
/// as [`Query::run`], for callers that scan with their own time budget or selection.
pub struct Matcher<'a> {
    query: &'a Query,
    state: &'a State,
    ctx: &'a EvalContext,
    index: Option<StepIndex>,
    stop: Option<Stop<'a>>,
}

impl std::fmt::Debug for Matcher<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Matcher")
            .field("query", self.query)
            .field("indexed", &self.index.is_some())
            .finish_non_exhaustive()
    }
}

impl Matcher<'_> {
    /// True if the task is live and matches the filter. With a [`Stop`] check (see
    /// [`Query::matcher_until`]) the answer is meaningless once the check returned true.
    #[must_use]
    pub fn matches(&self, task: &Task) -> bool {
        let env = Env {
            state: self.state,
            ctx: self.ctx,
            steps: self.index.as_ref(),
            stop: self.stop,
        };
        self.state.is_live(task.id.into()) && self.query.expr.eval(&env, task, &OnceCell::new())
    }
}

impl Query {
    /// True if the task matches the filter (ignores sorting).
    #[must_use]
    pub fn matches(&self, state: &State, task: &Task, ctx: &EvalContext) -> bool {
        self.expr.matches(state, task, ctx)
    }

    /// Runs the query: all live tasks that match, sorted. Deleted tasks and tasks under deleted lists
    /// or collections never match. Ties fall back to manual order.
    ///
    /// Step titles are indexed by task once per run, so free-text terms cost time linear in the size of
    /// the state rather than in tasks times steps.
    #[must_use]
    pub fn run<'a>(&self, state: &'a State, ctx: &EvalContext) -> Vec<&'a Task> {
        let m = self.matcher(state, ctx);
        let mut out: Vec<&Task> = state.tasks.values().filter(|t| m.matches(t)).collect();
        out.sort_by_cached_key(|t| self.order_key(state, t));
        out
    }

    /// Prepares the query for evaluating tasks one at a time (see [`Matcher`]).
    #[must_use]
    pub fn matcher<'a>(&'a self, state: &'a State, ctx: &'a EvalContext) -> Matcher<'a> {
        Matcher {
            query: self,
            state,
            ctx,
            index: self
                .expr
                .has_text()
                .then(|| step_index(state, None))
                .flatten(),
            stop: None,
        }
    }

    /// Like [`matcher`](Self::matcher), but calls `stop` periodically while building the step index
    /// and while scanning a task's steps. Returns `None` if `stop` returned true during the build.
    /// Once `stop` returns true, [`Matcher::matches`] answers are unreliable; the caller must check
    /// `stop` itself after its scan and discard the result.
    #[must_use]
    pub fn matcher_until<'a>(
        &'a self,
        state: &'a State,
        ctx: &'a EvalContext,
        stop: Stop<'a>,
    ) -> Option<Matcher<'a>> {
        let index = if self.expr.has_text() {
            Some(step_index(state, Some(stop))?)
        } else {
            None
        };
        Some(Matcher {
            query: self,
            state,
            ctx,
            index,
            stop: Some(stop),
        })
    }

    /// The total order [`run`](Self::run) sorts by: the sort clauses, then manual order (collection,
    /// list, position in list), then task ID.
    #[must_use]
    pub fn compare(&self, state: &State, a: &Task, b: &Task) -> Ordering {
        self.order_key(state, a).cmp(&self.order_key(state, b))
    }

    /// The task's place in [`compare`](Self::compare) order, as a value that can be stored (for keyset
    /// pagination). Optional fields sort after present ones; descending clauses reverse that.
    #[must_use]
    pub fn order_key(&self, state: &State, t: &Task) -> OrderKey {
        let mut k = OrderKey::new();
        let mut seen: Vec<SortKey> = Vec::with_capacity(self.sort.len());
        for s in &self.sort {
            // Only the first clause per key matters (and repeating it would only grow the key).
            if seen.contains(&s.key) {
                continue;
            }
            seen.push(s.key);
            let d = s.descending;
            match s.key {
                SortKey::Due => k.push_opt(t.due.map(date_value), d),
                SortKey::Priority => {
                    k.push_int(priority_rank(t.priority), d);
                }
                SortKey::Created => k.push_ts(t.created.at, d),
                SortKey::Updated => k.push_ts(t.updated.at, d),
                SortKey::Completed => k.push_opt_ts(t.completed_at, d),
                SortKey::Title => {
                    k.push_str(fold(&t.title), d);
                }
                SortKey::Manual => {}
            }
        }
        // Manual order: tasks of a missing list first, then lists without a collection, then by
        // collection position, list position and task position.
        match state.lists.get(&t.list) {
            None => {
                k.push_int(0, false);
            }
            Some(l) => {
                k.push_int(1, false);
                match l.collection.and_then(|c| state.collections.get(&c)) {
                    None => k.push_int(0, false).push_str("", false).push_str("", false),
                    Some(c) => k
                        .push_int(1, false)
                        .push_str(c.position.to_string(), false)
                        .push_str(c.id.to_string(), false),
                };
                k.push_str(l.position.to_string(), false)
                    .push_str(l.id.to_string(), false);
            }
        }
        k.push_str(t.position.to_string(), false)
            .push_str(t.id.to_string(), false);
        k
    }
}
