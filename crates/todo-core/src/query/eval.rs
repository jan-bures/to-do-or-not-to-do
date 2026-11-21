//! Query evaluation over the in-memory state.

use super::ast::{ActorSpec, DateExpr, DateField, DueState, Expr, Predicate, Query, SortKey};
use super::fold::fold;
use crate::actor::Actor;
use crate::model::Task;
use crate::state::State;
use crate::time::{Date, Timestamp, add_days, utc_date};
use std::cmp::Ordering;

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

    fn local_date(&self, ts: Timestamp) -> Date {
        let shifted = Timestamp::from_second(ts.as_second() + i64::from(self.utc_offset_seconds));
        utc_date(shifted.unwrap_or(ts))
    }

    fn resolve(&self, d: DateExpr) -> Option<Date> {
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

impl Predicate {
    /// Tests the predicate against a task.
    #[must_use]
    pub fn matches(&self, state: &State, task: &Task, ctx: &EvalContext) -> bool {
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
                let needle = fold(w);
                fold(&task.title).contains(&needle)
                    || fold(&task.notes).contains(&needle)
                    || state.steps.values().any(|s| {
                        s.task == task.id && s.deleted.is_none() && fold(&s.title).contains(&needle)
                    })
            }
        }
    }
}

impl Expr {
    /// Tests the expression against a task.
    #[must_use]
    pub fn matches(&self, state: &State, task: &Task, ctx: &EvalContext) -> bool {
        match self {
            Self::True => true,
            Self::And(v) => v.iter().all(|e| e.matches(state, task, ctx)),
            Self::Or(v) => v.iter().any(|e| e.matches(state, task, ctx)),
            Self::Not(e) => !e.matches(state, task, ctx),
            Self::Pred(p) => p.matches(state, task, ctx),
        }
    }
}

fn manual_order(state: &State, a: &Task, b: &Task) -> Ordering {
    let key = |t: &Task| {
        state.lists.get(&t.list).map(|l| {
            (
                l.collection
                    .and_then(|c| state.collections.get(&c))
                    .map(|c| (c.position.clone(), c.id)),
                l.position.clone(),
                l.id,
            )
        })
    };
    key(a)
        .cmp(&key(b))
        .then_with(|| a.position.cmp(&b.position))
        .then_with(|| a.id.cmp(&b.id))
}

fn by_key(key: SortKey, a: &Task, b: &Task) -> Ordering {
    // `None` sorts after `Some` for optional fields.
    fn opt<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
        match (a, b) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
    match key {
        SortKey::Due => opt(a.due, b.due),
        SortKey::Priority => a.priority.cmp(&b.priority),
        SortKey::Created => a.created.at.cmp(&b.created.at),
        SortKey::Updated => a.updated.at.cmp(&b.updated.at),
        SortKey::Completed => opt(a.completed_at, b.completed_at),
        SortKey::Title => fold(&a.title).cmp(&fold(&b.title)),
        SortKey::Manual => Ordering::Equal,
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
    #[must_use]
    pub fn run<'a>(&self, state: &'a State, ctx: &EvalContext) -> Vec<&'a Task> {
        let mut out: Vec<&Task> = state
            .tasks
            .values()
            .filter(|t| state.is_live(t.id.into()) && self.matches(state, t, ctx))
            .collect();
        out.sort_by(|a, b| {
            for s in &self.sort {
                let o = by_key(s.key, a, b);
                let o = if s.descending { o.reverse() } else { o };
                if o != Ordering::Equal {
                    return o;
                }
            }
            manual_order(state, a, b)
        });
        out
    }
}
