//! Workflow edits shared by all clients.

use todo_core::{
    Command, State,
    command::TaskPatch,
    ids::{ListId, TaskId},
    model::{ListFeatures, ListView, WorkflowStatus},
    time::Date,
};

use super::organize::{is_actionable, list_live};

/// Set a list's capabilities. Turning off the current default view falls back to List on the server.
pub fn set_list_features(state: &State, id: ListId, features: ListFeatures) -> Option<Command> {
    let list = state.lists.get(&id)?;
    (list_live(state, id) && list.features != features).then_some(Command::UpdateList {
        id,
        name: None,
        color: None,
        features: Some(features),
    })
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

/// Picking a start after the due date moves due to the same day in one undoable patch.
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
    let due = start
        .zip(task.due)
        .filter(|(start, due)| start > due)
        .map(|(start, _)| Some(start));
    Some(Command::UpdateTask {
        id,
        patch: TaskPatch {
            due,
            start_date: Some(start),
            ..TaskPatch::default()
        },
    })
}
