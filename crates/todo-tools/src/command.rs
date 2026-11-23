//! Map typed tool calls to commands. Authority, idempotency and execution belong to the caller.

use crate::args::*;
use serde_json::{Value, json};
use todo_core::command::{Command, Placement};
use todo_core::ids::{LinkId, ListId, ReminderId, StepId, TagId, TaskId};
use todo_core::model::{LinkKind, List};
use todo_core::query::fold;
use todo_core::state::State;
use todo_core::time::{Date, Timestamp};

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("use complete_task to mark a task done")]
    DoneStatus,
    #[error("status must be backlog, todo, or in_progress")]
    InvalidStatus,
    #[error(
        "nothing to change: give title, notes, due, start_date, status, priority or recurrence"
    )]
    EmptyTaskPatch,
    #[error("give title or completed, not both: make two update_step calls")]
    MixedStepPatch,
    #[error("nothing to change: give title or completed")]
    EmptyStepPatch,
    #[error(
        "at must be an RFC 3339 time such as 2030-01-02T09:00:00Z, or a local time such as 2030-01-02T09:00"
    )]
    InvalidReminderTime,
    #[error("invalid command: {0}")]
    InvalidCommand(#[from] serde_json::Error),
}

fn status_arg(status: &str) -> Result<&str, CommandError> {
    match status {
        "backlog" | "todo" | "in_progress" => Ok(status),
        "done" => Err(CommandError::DoneStatus),
        _ => Err(CommandError::InvalidStatus),
    }
}

/// Find a live, permitted Inbox in the same order as MCP: top level first, then manual position.
#[must_use]
pub fn resolve_inbox(state: &State, permits: impl Fn(&List) -> bool) -> Option<ListId> {
    state
        .lists
        .values()
        .filter(|l| state.is_live(l.id.into()) && permits(l) && fold(&l.name) == "inbox")
        .min_by_key(|l| (l.collection.is_some(), l.position.to_string(), l.id))
        .map(|l| l.id)
}

impl CreateTaskArgs {
    /// The host supplies the new task ID and resolves the explicit list or default Inbox.
    pub fn into_command(self, id: TaskId, list: ListId) -> Result<Command, CommandError> {
        // Let the REST/domain decoder interpret date and recurrence strings, preserving its errors.
        let mut cmd = json!({ "cmd": "create_task", "id": id, "list": list, "title": self.title });
        if let Some(n) = self.notes {
            cmd["notes"] = json!(n);
        }
        if let Some(d) = self.due {
            cmd["due"] = json!(d);
        }
        if let Some(start) = self.start_date {
            cmd["start_date"] = json!(start);
        }
        if let Some(status) = self.status {
            cmd["status"] = json!(status_arg(&status)?);
        }
        if let Some(p) = self.priority {
            cmd["priority"] = json!(p);
        }
        if let Some(r) = self.recurrence {
            cmd["recurrence"] = json!(r);
        }
        Ok(serde_json::from_value(cmd)?)
    }
}

impl UpdateTaskArgs {
    /// Omitted patch fields stay omitted; explicit null dates and recurrence clear the field.
    pub fn into_command(self) -> Result<Command, CommandError> {
        let mut patch = serde_json::Map::new();
        if let Some(t) = self.title {
            patch.insert("title".into(), json!(t));
        }
        if let Some(n) = self.notes {
            patch.insert("notes".into(), json!(n));
        }
        if let Some(d) = self.due {
            patch.insert("due".into(), json!(d));
        }
        if let Some(start) = self.start_date {
            patch.insert("start_date".into(), json!(start));
        }
        if let Some(status) = self.status {
            patch.insert("status".into(), json!(status_arg(&status)?));
        }
        if let Some(p) = self.priority {
            patch.insert("priority".into(), json!(p));
        }
        if let Some(r) = self.recurrence {
            patch.insert("recurrence".into(), json!(r));
        }
        if patch.is_empty() {
            return Err(CommandError::EmptyTaskPatch);
        }
        Ok(serde_json::from_value(
            json!({ "cmd": "update_task", "id": self.task_id, "patch": patch }),
        )?)
    }
}

impl CompleteTaskArgs {
    #[must_use]
    pub fn into_command(self) -> Command {
        let id = TaskId::from_uuid(self.task_id);
        if self.completed {
            Command::CompleteTask { id }
        } else {
            Command::UncompleteTask { id }
        }
    }
}

impl SetMyDayArgs {
    /// The host supplies today, using the original attempt's date if replaying a committed call.
    pub fn into_command(self, today: impl FnOnce() -> Date) -> Result<Command, CommandError> {
        let date = if self.in_my_day {
            self.date.map_or_else(|| json!(today()), Value::String)
        } else {
            Value::Null
        };
        Ok(serde_json::from_value(
            json!({"cmd": "set_my_day", "id": self.task_id, "date": date}),
        )?)
    }
}

impl From<PositionArg> for Placement {
    fn from(value: PositionArg) -> Self {
        match value {
            PositionArg::First => Self::First,
            PositionArg::Last => Self::Last,
        }
    }
}

impl MoveTaskArgs {
    #[must_use]
    pub fn into_command(self) -> Command {
        Command::MoveTask {
            id: TaskId::from_uuid(self.task_id),
            list: ListId::from_uuid(self.list_id),
            from_list: self.from_list.map(ListId::from_uuid),
            placement: self.position.into(),
        }
    }
}

impl MergeTasksArgs {
    #[must_use]
    pub fn into_command(self) -> Command {
        Command::MergeTasks {
            source: TaskId::from_uuid(self.source_task_id),
            target: TaskId::from_uuid(self.target_task_id),
        }
    }
}

impl ArchiveTaskArgs {
    #[must_use]
    pub fn into_command(self) -> Command {
        Command::Delete {
            id: TaskId::from_uuid(self.task_id).into(),
        }
    }
}

impl AddNoteArgs {
    #[must_use]
    pub fn into_command(self) -> Command {
        Command::AddNote {
            task: TaskId::from_uuid(self.task_id),
            text: self.text,
        }
    }
}

impl AddLinkArgs {
    #[must_use]
    pub fn into_command(self, id: LinkId) -> Command {
        Command::AddLink {
            id,
            task: TaskId::from_uuid(self.task_id),
            kind: LinkKind::Web {
                url: self.url,
                title: self.title,
            },
        }
    }
}

impl AddStepArgs {
    #[must_use]
    pub fn into_command(self, id: StepId) -> Command {
        Command::CreateStep {
            id,
            task: TaskId::from_uuid(self.task_id),
            title: self.title,
            placement: self.position.into(),
        }
    }
}

impl UpdateStepArgs {
    pub fn into_command(self) -> Result<Command, CommandError> {
        let id = StepId::from_uuid(self.step_id);
        match (self.title, self.completed) {
            (Some(_), Some(_)) => Err(CommandError::MixedStepPatch),
            (Some(title), None) => Ok(Command::UpdateStep { id, title }),
            (None, Some(true)) => Ok(Command::CompleteStep { id }),
            (None, Some(false)) => Ok(Command::UncompleteStep { id }),
            (None, None) => Err(CommandError::EmptyStepPatch),
        }
    }
}

impl TagTaskArgs {
    #[must_use]
    pub fn into_command(self, on: bool) -> Command {
        let task = TaskId::from_uuid(self.task_id);
        let tag = TagId::from_uuid(self.tag_id);
        if on {
            Command::AddTag { task, tag }
        } else {
            Command::RemoveTag { task, tag }
        }
    }
}

impl CreateTagArgs {
    #[must_use]
    pub fn into_command(self, id: TagId) -> Command {
        Command::CreateTag {
            id,
            name: self.name,
            color: self.color,
        }
    }
}

/// An absolute time, or a civil date-time resolved with the supplied owner's zone rules.
pub fn reminder_time(at: &str, zone: &jiff::tz::TimeZone) -> Result<Timestamp, CommandError> {
    if let Ok(t) = at.parse::<Timestamp>() {
        return Ok(t);
    }
    at.parse::<jiff::civil::DateTime>()
        .ok()
        .and_then(|dt| zone.to_timestamp(dt).ok())
        .ok_or(CommandError::InvalidReminderTime)
}

impl AddReminderArgs {
    #[must_use]
    pub fn into_command(self, id: ReminderId, at: Timestamp) -> Command {
        Command::AddReminder {
            id,
            task: TaskId::from_uuid(self.task_id),
            at,
        }
    }
}

impl UpdateReminderArgs {
    #[must_use]
    pub fn into_command(self, at: Timestamp) -> Command {
        Command::UpdateReminder {
            id: ReminderId::from_uuid(self.reminder_id),
            at,
        }
    }
}
