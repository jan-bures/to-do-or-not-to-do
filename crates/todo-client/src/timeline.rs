//! Calendar-date timeline projections, independent of pixel size and platform.

use std::collections::BTreeSet;

use todo_core::State;
use todo_core::ids::{ListId, TaskId};
use todo_core::model::Task;
use todo_core::query::{EvalContext, QueryStatus};
use todo_core::time::{Date, days_between};

use crate::room_header::{RoomKey, SmartList, list_live, room_tasks, task_live};

/// Shape of a scheduled bar. Unscheduled tasks have no bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Range,
    Milestone,
    OpenEnded,
}

/// A task's inclusive date interval and how to draw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub first: Date,
    pub last: Date,
    pub kind: SpanKind,
}

impl Span {
    #[must_use]
    pub fn of(task: &Task) -> Option<Self> {
        let (first, last, kind) = match (task.start_date, task.due) {
            (Some(start), Some(due)) => (start, due, SpanKind::Range),
            (None, Some(due)) => (due, due, SpanKind::Milestone),
            (Some(start), None) => (start, start, SpanKind::OpenEnded),
            (None, None) => return None,
        };
        Some(Self { first, last, kind })
    }

    /// Clip to an inclusive date window and return `(offset_days, visible_days)`.
    #[must_use]
    pub fn in_window(self, from: Date, through: Date) -> Option<(i64, i64)> {
        let first = self.first.max(from);
        let last = self.last.min(through);
        (first <= last).then(|| (days_between(from, first), days_between(first, last) + 1))
    }
}

/// A scheduled task, in room order. The list ID can supply a tint in a cross-list room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lane {
    pub task: TaskId,
    pub list: ListId,
    pub stage: QueryStatus,
    pub span: Span,
}

/// Separate scheduled lanes and Unscheduled tray, each in input order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    pub lanes: Vec<Lane>,
    pub unscheduled: Vec<TaskId>,
}

/// Project the caller's visible tasks, keeping their order within each output group.
pub fn project<'a>(tasks: impl IntoIterator<Item = &'a Task>) -> Timeline {
    let mut timeline = Timeline::default();
    for task in tasks {
        if let Some(span) = Span::of(task) {
            timeline.lanes.push(Lane {
                task: task.id,
                list: task.list,
                stage: QueryStatus::of(task),
                span,
            });
        } else {
            timeline.unscheduled.push(task.id);
        }
    }
    timeline
}

/// Timeline is available for an enabled list, an eligible collection, or Planned. Planned uses
/// every dated open task (start-only tasks included); the legacy Planned list stays unchanged.
pub fn room(state: &State, room: RoomKey<'_>, ctx: &EvalContext) -> Option<Timeline> {
    match room {
        RoomKey::List(id) if list_live(state, id) && state.lists[&id].features.timeline => {}
        RoomKey::Collection(id)
            if state.lists.values().any(|l| {
                l.collection == Some(id) && l.features.timeline && list_live(state, l.id)
            }) => {}
        RoomKey::Smart(SmartList::Planned) => {
            let mut tasks: Vec<_> = state
                .tasks
                .values()
                .filter(|t| {
                    task_live(state, t)
                        && !t.is_done()
                        && (t.start_date.is_some() || t.due.is_some())
                })
                .collect();
            tasks.sort_unstable_by_key(|t| (t.start_date.or(t.due), t.id));
            return Some(project(tasks));
        }
        _ => return None,
    }
    let tasks = room_tasks(state, room, &BTreeSet::new(), ctx);
    Some(project(tasks.open.into_iter().chain(tasks.done)))
}
