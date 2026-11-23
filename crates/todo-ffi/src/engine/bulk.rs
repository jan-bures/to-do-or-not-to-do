//! Room selection and the tracked bulk command, through the same engine as single-task actions.

use std::sync::{Arc, Mutex};

use super::{
    DestinationGroup, TodoEngine, Undone, destinations, invalid, list_id, locked,
    organize::OrganizeChange, task_id,
};
use crate::FfiError;
use todo_client::intent::organize::task_destination_groups;
use todo_client::selection::{self, BulkTaskAction, Selection, Tool};
use todo_core::ids::TaskId;
use todo_core::time::Date;

#[derive(Debug, Default, uniffi::Object)]
pub struct RoomSelection(Mutex<Selection>);

#[uniffi::export]
impl RoomSelection {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn toggle(&self, visible: Vec<String>, id: String) -> Vec<String> {
        let visible = ids(visible);
        let mut selection = locked(&self.0);
        if let Ok(id) = task_id(&id) {
            selection.toggle(&visible, id, None);
        }
        strings(selection.ordered(&visible))
    }

    pub fn retain(&self, visible: Vec<String>) -> Vec<String> {
        let visible = ids(visible);
        let mut selection = locked(&self.0);
        selection.retain_visible(&visible);
        strings(selection.ordered(&visible))
    }

    pub fn clear(&self) {
        locked(&self.0).clear();
    }
}

fn ids(values: Vec<String>) -> Vec<TaskId> {
    values.iter().filter_map(|id| task_id(id).ok()).collect()
}
fn strings(tasks: Vec<TaskId>) -> Vec<String> {
    tasks.iter().map(ToString::to_string).collect()
}
fn day(value: &str) -> Result<Date, FfiError> {
    value.parse().map_err(|_| invalid("Choose a valid date."))
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BulkEdit {
    Completion { done: bool },
    Important { on: bool },
    MyDay { date: String, on: bool },
    Move { list: String },
    Due { date: Option<String> },
    Delete,
}

impl BulkEdit {
    fn action(self) -> Result<BulkTaskAction, FfiError> {
        Ok(match self {
            Self::Completion { done } => BulkTaskAction::Completion { done },
            Self::Important { on } => BulkTaskAction::Important { on },
            Self::MyDay { date, on } => BulkTaskAction::MyDay {
                day: day(&date)?,
                on,
            },
            Self::Move { list } => BulkTaskAction::Move {
                list: list_id(&list)?,
            },
            Self::Due { date } => BulkTaskAction::Due {
                date: date.as_deref().map(day).transpose()?,
            },
            Self::Delete => BulkTaskAction::Delete,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BulkControl {
    Apply { edit: BulkEdit },
    Move,
    Due,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BulkTool {
    pub control: BulkControl,
    pub label: String,
    pub short_label: String,
}

impl From<Tool> for BulkTool {
    fn from(tool: Tool) -> Self {
        let control = match tool {
            Tool::Move => BulkControl::Move,
            Tool::Due => BulkControl::Due,
            Tool::Apply(action) => BulkControl::Apply {
                edit: match action {
                    BulkTaskAction::Completion { done } => BulkEdit::Completion { done },
                    BulkTaskAction::Important { on } => BulkEdit::Important { on },
                    BulkTaskAction::MyDay { day, on } => BulkEdit::MyDay {
                        date: day.to_string(),
                        on,
                    },
                    BulkTaskAction::Delete => BulkEdit::Delete,
                    BulkTaskAction::Move { list } => BulkEdit::Move {
                        list: list.to_string(),
                    },
                    BulkTaskAction::Due { date } => BulkEdit::Due {
                        date: date.map(|d| d.to_string()),
                    },
                },
            },
        };
        Self {
            control,
            label: tool.label().into(),
            short_label: tool.short_label().into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BulkBar {
    pub label: String,
    pub tools: Vec<BulkTool>,
    pub has_due: bool,
    pub has_repeat: bool,
}

#[uniffi::export]
impl TodoEngine {
    pub fn bulk_bar(&self, tasks: Vec<String>, today: String) -> BulkBar {
        let ids = ids(tasks);
        let state = self.client.view();
        let tasks = || ids.iter().filter_map(|id| state.tasks.get(id));
        BulkBar {
            label: selection::count_label(ids.len()),
            tools: day(&today).ok().map_or_else(Vec::new, |day| {
                selection::tools(&state, &ids, day)
                    .into_iter()
                    .map(Into::into)
                    .collect()
            }),
            has_due: tasks().any(|t| t.due.is_some()),
            has_repeat: tasks().any(|t| t.recurrence.is_some()),
        }
    }

    pub fn bulk_destinations(&self, tasks: Vec<String>, filter: String) -> Vec<DestinationGroup> {
        let ids = ids(tasks);
        if ids.len() < 2 {
            return Vec::new();
        }
        let state = self.client.view();
        destinations(task_destination_groups(&state, &filter, |list| {
            !selection::can_move(&state, &ids, list)
        }))
    }

    pub fn bulk_edit(
        &self,
        tasks: Vec<String>,
        edit: BulkEdit,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let ids = ids(tasks);
        let action = edit.action()?;
        let Some((cmd, label)) = selection::batch(&self.client.view(), &ids, action)
            .map_err(|e| invalid(e.to_string()))?
        else {
            return Ok(None);
        };
        Ok(Some(Arc::new(OrganizeChange::new(
            self.client.submit_tracked(cmd)?,
            Undone::Change("Undo changing selected tasks", "task"),
            label,
        ))))
    }
}
