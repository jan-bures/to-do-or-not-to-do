//! Workflow edits shared by all clients.

use todo_core::{
    Command, State,
    command::{ListFeaturesPatch, TaskPatch},
    ids::{ListId, TaskId},
    model::{ListView, WorkflowStatus},
    time::Date,
};

use super::organize::{is_actionable, list_live};

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
