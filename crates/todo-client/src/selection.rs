//! Room-local multi-selection and shared bulk action presentation.

use std::collections::BTreeSet;

pub use todo_core::command::BulkTaskAction;
use todo_core::decide::MAX_BULK_TASKS;
use todo_core::ids::{ListId, TaskId};
use todo_core::model::{Priority, Task};
use todo_core::time::Date;
use todo_core::{Command, State};

/// The detail selection is separate. The frontend passes its current cursor when starting a range
/// or toggling the first item with a modifier. Touch selection can start without a cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    members: BTreeSet<TaskId>,
    anchor: Option<TaskId>,
    cursor: Option<TaskId>,
}

impl Selection {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn contains(&self, id: TaskId) -> bool {
        self.members.contains(&id)
    }

    #[must_use]
    pub fn cursor(&self) -> Option<TaskId> {
        self.cursor
    }

    /// Drop rows filtered out, collapsed or removed by sync. They don't come back selected.
    pub fn retain_visible(&mut self, visible: &[TaskId]) {
        if self.members.is_empty() {
            return;
        }
        let order = visible;
        let visible: BTreeSet<_> = visible.iter().copied().collect();
        self.members.retain(|id| visible.contains(id));
        self.anchor = self.anchor.filter(|id| visible.contains(id));
        self.cursor = self
            .cursor
            .filter(|id| visible.contains(id))
            .or_else(|| order.iter().copied().find(|id| self.members.contains(id)));
        if self.members.is_empty() {
            self.clear();
        }
    }

    /// Return members in the current visible order, not click order or ID order.
    #[must_use]
    pub fn ordered(&self, visible: &[TaskId]) -> Vec<TaskId> {
        visible
            .iter()
            .copied()
            .filter(|id| self.contains(*id))
            .collect()
    }

    pub fn toggle(&mut self, visible: &[TaskId], id: TaskId, previous: Option<TaskId>) {
        if !visible.contains(&id) {
            return;
        }
        self.retain_visible(visible);
        if self.members.is_empty()
            && let Some(previous) = previous.filter(|p| visible.contains(p))
        {
            self.members.insert(previous);
        }
        if !self.members.remove(&id) {
            self.members.insert(id);
        }
        self.anchor = Some(id);
        self.cursor = Some(id);
        if self.members.is_empty() {
            self.clear();
        }
    }

    pub fn range(&mut self, visible: &[TaskId], id: TaskId, previous: Option<TaskId>) {
        let Some(end) = visible.iter().position(|v| *v == id) else {
            return;
        };
        let start = self
            .anchor
            .or(previous)
            .and_then(|a| visible.iter().position(|v| *v == a))
            .or_else(|| {
                self.cursor
                    .and_then(|c| visible.iter().position(|v| *v == c))
            })
            .unwrap_or(end);
        self.anchor = Some(visible[start]);
        self.cursor = Some(id);
        self.members = visible[start.min(end)..=start.max(end)]
            .iter()
            .copied()
            .collect();
    }

    pub fn extend(&mut self, visible: &[TaskId], by: isize, previous: Option<TaskId>) {
        let Some(last) = visible.len().checked_sub(1) else {
            return;
        };
        let index = self
            .cursor
            .or(previous)
            .and_then(|id| visible.iter().position(|v| *v == id));
        let next = index.map_or(if by < 0 { last } else { 0 }, |i| {
            i.saturating_add_signed(by).min(last)
        });
        self.range(visible, visible[next], previous);
    }

    pub fn all(&mut self, visible: &[TaskId], previous: Option<TaskId>) {
        self.members = visible.iter().copied().collect();
        self.anchor = self
            .cursor
            .or(previous)
            .filter(|id| self.contains(*id))
            .or_else(|| visible.first().copied());
        self.cursor = self.anchor;
    }
}

/// A toolbar entry with a fixed action (including the direction for a mixed selection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Apply(BulkTaskAction),
    Move,
    Due,
}

impl Tool {
    /// An optional short visible label for narrow action strips. Keep the full accessible name.
    #[must_use]
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Apply(BulkTaskAction::Important { on: true }) => "Star",
            Self::Apply(BulkTaskAction::Important { on: false }) => "Unstar",
            Self::Apply(BulkTaskAction::MyDay { on: true, .. }) => "My Day",
            Self::Apply(BulkTaskAction::MyDay { on: false, .. }) => "Remove",
            Self::Move => "Move",
            Self::Due => "Due",
            other => other.label(),
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Apply(BulkTaskAction::Completion { done: true }) => "Complete",
            Self::Apply(BulkTaskAction::Completion { done: false }) => "Reopen",
            Self::Apply(BulkTaskAction::Important { on: true }) => "Mark important",
            Self::Apply(BulkTaskAction::Important { on: false }) => "Remove importance",
            Self::Apply(BulkTaskAction::MyDay { on: true, .. }) => "Add to My Day",
            Self::Apply(BulkTaskAction::MyDay { on: false, .. }) => "Remove from My Day",
            Self::Move | Self::Apply(BulkTaskAction::Move { .. }) => "Move to…",
            Self::Due | Self::Apply(BulkTaskAction::Due { .. }) => "Set due…",
            Self::Apply(BulkTaskAction::Reorder { .. }) => "Reorder",
            Self::Apply(BulkTaskAction::Status { .. }) => "Status",
            Self::Apply(BulkTaskAction::Delete) => "Delete",
        }
    }
}

#[must_use]
pub fn count_label(count: usize) -> String {
    format!(
        "{count} {} selected",
        if count == 1 { "task" } else { "tasks" }
    )
}

fn live<'a>(state: &'a State, ids: &'a [TaskId]) -> impl Iterator<Item = &'a Task> {
    ids.iter()
        .filter_map(|id| state.tasks.get(id).filter(|_| state.is_live((*id).into())))
}

/// In mixed selections, prefer the positive action; apply it only to tasks that need the change.
/// The reverse is offered when every selected task is already in that state.
#[must_use]
pub fn tools(state: &State, ids: &[TaskId], today: Date) -> Vec<Tool> {
    let tasks: Vec<_> = live(state, ids).collect();
    if tasks.len() < 2 {
        return Vec::new();
    }
    vec![
        Tool::Apply(BulkTaskAction::Completion {
            done: tasks.iter().any(|t| !t.is_done()),
        }),
        Tool::Apply(BulkTaskAction::Important {
            on: tasks.iter().any(|t| t.priority != Priority::High),
        }),
        Tool::Apply(BulkTaskAction::MyDay {
            day: today,
            on: tasks.iter().any(|t| t.my_day != Some(today)),
        }),
        Tool::Move,
        Tool::Due,
        Tool::Apply(BulkTaskAction::Delete),
    ]
}

#[must_use]
pub fn can_move(state: &State, ids: &[TaskId], list: ListId) -> bool {
    state.is_live(list.into()) && live(state, ids).any(|t| t.list != list)
}

/// More tasks would change than the server accepts in one command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Choose at most {MAX_BULK_TASKS} affected tasks to change at once.")]
pub struct BulkLimit;

/// A single tracked submission, not a sequence of independent submits. Its undo compensates the
/// entire server event group, including any next occurrences created by completing repeats.
pub fn batch(
    state: &State,
    ids: &[TaskId],
    action: BulkTaskAction,
) -> Result<Option<(Command, String)>, BulkLimit> {
    if ids.len() < 2 {
        return Ok(None);
    }
    if let BulkTaskAction::Move { list } = action
        && !can_move(state, ids, list)
    {
        return Ok(None);
    }
    let mut seen = BTreeSet::new();
    let tasks: Vec<_> = live(state, ids)
        .filter(|t| seen.insert(t.id) && action.change(t).is_some())
        .map(|t| t.id)
        .collect();
    let n = tasks.len();
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_BULK_TASKS {
        return Err(BulkLimit);
    }
    let count = if n == 1 {
        "1 task".to_owned()
    } else {
        format!("{n} tasks")
    };
    let label = match action {
        BulkTaskAction::Completion { done: true } => format!("Completed {count}"),
        BulkTaskAction::Completion { done: false } => format!("Reopened {count}"),
        BulkTaskAction::Important { on: true } => format!("Marked {count} important"),
        BulkTaskAction::Important { on: false } => format!("Removed importance from {count}"),
        BulkTaskAction::MyDay { on: true, .. } => format!("Added {count} to My Day"),
        BulkTaskAction::MyDay { on: false, .. } => format!("Removed {count} from My Day"),
        BulkTaskAction::Move { .. } => format!("Moved {count}"),
        BulkTaskAction::Reorder { .. } => format!("Reordered {count}"),
        BulkTaskAction::Due { date: Some(_) } => format!("Set due date for {count}"),
        BulkTaskAction::Due { date: None } => format!("Cleared due date for {count}"),
        BulkTaskAction::Status { .. } => format!("Changed status of {count}"),
        BulkTaskAction::Delete => format!("Deleted {count}"),
    };
    Ok(Some((Command::BulkTasks { tasks, action }, label)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn mixed_selection_applies_one_direction_to_only_the_tasks_that_need_it() {
        use todo_core::command::NewTask;
        use todo_core::time::Timestamp;
        use todo_core::{Actor, apply, decide};
        let mut state = State::new();
        let actor = Actor::owner("test").unwrap();
        let list = ListId::from_uuid(Uuid::from_u128(10));
        let a = TaskId::from_uuid(Uuid::from_u128(1));
        let b = TaskId::from_uuid(Uuid::from_u128(2));
        let today = "2026-11-21".parse().unwrap();
        let mut apply_cmd = |cmd| {
            for event in decide(&state, &actor, &cmd, Timestamp::UNIX_EPOCH).unwrap() {
                apply(&mut state, &event).unwrap();
            }
        };
        apply_cmd(Command::CreateList {
            id: list,
            collection: None,
            name: "Test".into(),
            color: None,
            placement: Default::default(),
        });
        apply_cmd(Command::CreateTask(Box::new(NewTask::new(a, list, "Open"))));
        let mut done = NewTask::new(b, list, "Done");
        done.priority = Priority::High;
        done.my_day = Some(today);
        apply_cmd(Command::CreateTask(Box::new(done)));
        apply_cmd(Command::CompleteTask { id: b });
        let ids = [b, a];
        let tools = tools(&state, &ids, today);
        assert_eq!(
            tools.iter().map(|t| t.label()).collect::<Vec<_>>(),
            [
                "Complete",
                "Mark important",
                "Add to My Day",
                "Move to…",
                "Set due…",
                "Delete"
            ]
        );
        let Some((Command::BulkTasks { tasks, action }, label)) =
            batch(&state, &ids, BulkTaskAction::Completion { done: true }).unwrap()
        else {
            panic!("expected a batch");
        };
        assert_eq!(tasks, [a]);
        assert_eq!(action, BulkTaskAction::Completion { done: true });
        assert_eq!(label, "Completed 1 task");
        assert!(!can_move(&state, &ids, list));
    }

    #[test]
    fn range_keeps_its_anchor_and_shrinks_when_reversing() {
        let rows: Vec<_> = [7, 1, 6, 2, 5]
            .map(|n| TaskId::from_uuid(Uuid::from_u128(n)))
            .into();
        let mut s = Selection::default();
        s.toggle(&rows, rows[3], Some(rows[1]));
        assert_eq!(s.ordered(&rows), [rows[1], rows[3]]);
        s.range(&rows, rows[0], None);
        assert_eq!(s.ordered(&rows), rows[..=3]);
        s.extend(&rows, 1, None);
        assert_eq!(s.ordered(&rows), rows[1..=3]);
        s.all(&rows, None);
        assert_eq!(s.ordered(&rows), rows);
        s.retain_visible(&rows[2..]);
        assert_eq!(s.ordered(&rows), rows[2..]);
        s.clear();
        assert!(s.ordered(&rows).is_empty());
    }
}
