//! Calendar-date timeline projections, independent of pixel size and platform.

use std::collections::BTreeSet;

use todo_core::State;
use todo_core::ids::{ListId, TaskId};
use todo_core::model::Task;
use todo_core::query::{EvalContext, QueryStatus};
use todo_core::time::{Date, Weekday, add_days, days_between};

/// Calendar scale, shared across clients; pixel widths belong to each viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zoom {
    Week,
    #[default]
    TwoWeeks,
    Month,
    Quarter,
}

impl Zoom {
    pub const ALL: [Self; 4] = [Self::Week, Self::TwoWeeks, Self::Month, Self::Quarter];

    pub const fn days(self) -> i64 {
        match self {
            Self::Week => 7,
            Self::TwoWeeks => 14,
            Self::Month => 35,
            Self::Quarter => 84,
        }
    }

    pub const fn column_days(self) -> i64 {
        match self {
            Self::Quarter => 7,
            _ => 1,
        }
    }

    /// Leave a little past context at Today. Week columns always start on Monday.
    pub fn around_today(self, today: Date) -> Date {
        let before = match self {
            Self::Week => 1,
            Self::TwoWeeks => 2,
            Self::Month | Self::Quarter => Weekday::of(today).offset() as i64 + 7,
        };
        add_days(today, -before).unwrap_or(Date::MIN)
    }

    pub fn column_start(self, day: Date) -> Date {
        if self == Self::Quarter {
            add_days(day, -(Weekday::of(day).offset() as i64)).unwrap_or(Date::MIN)
        } else {
            day
        }
    }
}

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
        Self::from_dates(task.start_date, task.due)
    }

    #[must_use]
    pub fn from_dates(start: Option<Date>, due: Option<Date>) -> Option<Self> {
        let (first, last, kind) = match (start, due) {
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

/// Timeline is available for an enabled list, an eligible collection, or Planned.
pub fn available(state: &State, room: RoomKey<'_>) -> bool {
    match room {
        RoomKey::List(id) => list_live(state, id) && state.lists[&id].features.timeline,
        RoomKey::Collection(id) => state
            .lists
            .values()
            .any(|l| l.collection == Some(id) && l.features.timeline && list_live(state, l.id)),
        RoomKey::Smart(SmartList::Planned) => true,
        _ => false,
    }
}

/// Whether a task can be scheduled here, regardless of the client's temporary filter or zoom.
pub fn includes(state: &State, room: RoomKey<'_>, task: &Task) -> bool {
    task_live(state, task)
        && available(state, room)
        && match room {
            RoomKey::List(id) => task.list == id,
            RoomKey::Collection(id) => state.lists[&task.list].collection == Some(id),
            RoomKey::Smart(SmartList::Planned) => !task.is_done() && Span::of(task).is_some(),
            _ => false,
        }
}

/// Planned uses every dated open task (start-only included); the legacy list stays unchanged.
pub fn room(state: &State, room: RoomKey<'_>, ctx: &EvalContext) -> Option<Timeline> {
    if !available(state, room) {
        return None;
    }
    if room == RoomKey::Smart(SmartList::Planned) {
        let mut tasks: Vec<_> = state
            .tasks
            .values()
            .filter(|t| includes(state, room, t))
            .collect();
        tasks.sort_unstable_by_key(|t| (t.start_date.or(t.due), t.id));
        return Some(project(tasks));
    }
    let tasks = room_tasks(state, room, &BTreeSet::new(), ctx);
    Some(project(tasks.open.into_iter().chain(tasks.done)))
}
