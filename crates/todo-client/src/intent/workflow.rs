//! Workflow edits shared by all clients.

use todo_core::{
    Command, State,
    command::{ListFeaturesPatch, TaskPatch},
    ids::{ListId, TaskId},
    model::{ListView, Task, WorkflowStatus},
    time::{Date, add_days},
};

use super::organize::{is_actionable, list_live};
use crate::{room_header::RoomKey, timeline};

/// The two dates as they were when a scheduling gesture started. A change to either date on
/// another device cancels the gesture rather than overwriting the new schedule with stale input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dates {
    pub start: Option<Date>,
    pub due: Option<Date>,
}

impl Dates {
    pub fn of(task: &Task) -> Self {
        Self {
            start: task.start_date,
            due: task.due,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schedule {
    Move(i64),
    StartEdge(i64),
    DueEdge(i64),
    Duration(i64),
    Place(Date),
    PickStart(Option<Date>),
    PickDue(Option<Date>),
}

impl Schedule {
    /// Inclusive dates for both the live preview and the command. Resizing clamps at the other
    /// edge. A milestone can acquire a start; a start-only bar can acquire a due date.
    pub fn preview(self, mut dates: Dates) -> Option<Dates> {
        let old = dates;
        match self {
            Self::Move(by) => {
                if let Some(start) = dates.start {
                    dates.start = Some(add_days(start, by)?);
                }
                if let Some(due) = dates.due {
                    dates.due = Some(add_days(due, by)?);
                }
            }
            Self::StartEdge(by) => {
                let start = dates.start.or_else(|| dates.due.filter(|_| by < 0))?;
                let next = add_days(start, by)?;
                dates.start = Some(dates.due.map_or(next, |due| next.min(due)));
            }
            Self::DueEdge(by) => {
                let due = dates.due.or_else(|| dates.start.filter(|_| by > 0))?;
                let next = add_days(due, by)?;
                dates.due = Some(dates.start.map_or(next, |start| next.max(start)));
            }
            Self::Duration(by) => {
                let due = dates.due.or(dates.start)?;
                let start = dates.start.unwrap_or(due);
                dates.start = Some(start);
                dates.due = Some(add_days(due, by)?.max(start));
            }
            Self::Place(day) if dates == Dates::default() => dates.due = Some(day),
            Self::Place(_) => return None,
            Self::PickStart(start) => {
                dates.start = start;
                if let (Some(start), Some(due)) = (start, dates.due) {
                    dates.due = Some(start.max(due));
                }
            }
            Self::PickDue(due) => {
                dates.due = due;
                if let (Some(start), Some(due)) = (dates.start, due) {
                    dates.start = Some(start.min(due));
                }
            }
        }
        (dates != old).then_some(dates)
    }
}

/// One undoable write, only if the task is still in the same timeline with the same dates.
pub fn schedule(
    state: &State,
    room: RoomKey<'_>,
    id: TaskId,
    before: Dates,
    edit: Schedule,
) -> Option<Command> {
    let task = state.tasks.get(&id)?;
    if Dates::of(task) != before || !timeline::includes(state, room, task) {
        return None;
    }
    let after = edit.preview(before)?;
    Some(Command::UpdateTask {
        id,
        patch: TaskPatch {
            start_date: (after.start != before.start).then_some(after.start),
            due: (after.due != before.due).then_some(after.due),
            recurrence: (before.due.is_some() && after.due.is_none() && task.recurrence.is_some())
                .then_some(None),
            ..TaskPatch::default()
        },
    })
}

/// Patch the settings being edited, leaving other devices' changes alone.
pub fn set_list_features(
    state: &State,
    id: ListId,
    features: ListFeaturesPatch,
) -> Option<Command> {
    let list = state.lists.get(&id)?;
    (list_live(state, id) && list.features != features.merge(list.features)).then_some(
        Command::UpdateList {
            id,
            name: None,
            color: None,
            features: Some(features),
        },
    )
}

/// Change just the stored open stage, not the task's completion state.
pub fn set_status(state: &State, id: TaskId, status: WorkflowStatus) -> Option<Command> {
    let task = state.tasks.get(&id)?;
    if !is_actionable(state, task)
        || !state
            .lists
            .get(&task.list)?
            .features
            .allows(ListView::Board)
        || task.status == status
    {
        return None;
    }
    Some(Command::UpdateTask {
        id,
        patch: TaskPatch {
            status: Some(status),
            ..TaskPatch::default()
        },
    })
}

/// Pick a start date. The server reconciles it with the current due date in the same event.
pub fn set_start(state: &State, id: TaskId, start: Option<Date>) -> Option<Command> {
    let task = state.tasks.get(&id)?;
    if !is_actionable(state, task)
        || !state
            .lists
            .get(&task.list)?
            .features
            .allows(ListView::Timeline)
        || task.start_date == start
    {
        return None;
    }
    Some(Command::UpdateTask {
        id,
        patch: TaskPatch {
            start_date: Some(start),
            ..TaskPatch::default()
        },
    })
}
