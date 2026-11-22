//! Reminders: which one a task's Reminder row shows and edits, the commands that set, move or remove it,
//! and the alarms a client schedules from the synced reminders.
//!
//! A task can carry several reminders (an agent or the API can add more). The detail pane shows one: the
//! first on or after the owner's local today, else the latest. Setting a time moves that one; a task
//! without one gets a new one. Removing deletes that one only.

use todo_core::State;
use todo_core::command::Command;
use todo_core::ids::{ReminderId, TaskId};
use todo_core::model::Reminder;
use todo_core::query::EvalContext;
use todo_core::time::Timestamp;
use uuid::Uuid;

/// The task's live reminders, earliest first.
fn live(state: &State, task: TaskId) -> Vec<&Reminder> {
    let mut all: Vec<&Reminder> = state
        .reminders
        .values()
        .filter(|r| r.task == task && state.is_live(r.id.into()))
        .collect();
    all.sort_by_key(|r| (r.at, r.id));
    all
}

/// The reminder the task's Reminder row shows and edits: the first on or after the local today, else
/// the latest. `None` when the task has none.
pub fn shown<'a>(state: &'a State, task: TaskId, ctx: &EvalContext) -> Option<&'a Reminder> {
    let all = live(state, task);
    all.iter()
        .find(|r| ctx.local_date(r.at) >= ctx.today)
        .or(all.last())
        .copied()
}

/// Sets the row's reminder to `at`: moves the [`shown`] one, or adds one when the task has none. `None`
/// when it is already at `at`, or the task is gone.
pub fn set_reminder(
    state: &State,
    task: TaskId,
    at: Timestamp,
    ctx: &EvalContext,
    new_id: impl FnOnce() -> Uuid,
) -> Option<Command> {
    if !state.is_live(task.into()) {
        return None;
    }
    match shown(state, task, ctx) {
        Some(r) if r.at == at => None,
        Some(r) => Some(Command::UpdateReminder { id: r.id, at }),
        None => Some(Command::AddReminder {
            id: ReminderId::from_uuid(new_id()),
            task,
            at,
        }),
    }
}

/// Removes the row's reminder (the [`shown`] one; others stay). `None` when there is none.
pub fn remove_reminder(state: &State, task: TaskId, ctx: &EvalContext) -> Option<Command> {
    if !state.is_live(task.into()) {
        return None;
    }
    shown(state, task, ctx).map(|r| Command::Delete { id: r.id.into() })
}

/// A reminder a client should alert for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alarm {
    /// The reminder.
    pub reminder: ReminderId,
    /// Its task.
    pub task: TaskId,
    /// When it fires.
    pub at: Timestamp,
}

/// Every reminder to alert for, from `since` on, earliest first: live reminders on live, open tasks (not
/// completed, deleted, merged away, or in a deleted list). A reminder missing here has been removed, or
/// its task completed or deleted, here or elsewhere: its alarm goes.
pub fn alarms(state: &State, since: Timestamp) -> Vec<Alarm> {
    let mut out: Vec<Alarm> = state
        .reminders
        .values()
        .filter(|r| r.at >= since && state.is_live(r.id.into()))
        .filter(|r| {
            state
                .tasks
                .get(&r.task)
                .is_some_and(|t| !t.is_done() && t.merged_into.is_none())
        })
        .map(|r| Alarm {
            reminder: r.id,
            task: r.task,
            at: r.at,
        })
        .collect();
    out.sort_by_key(|a| (a.at, a.reminder));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{create_list, delete};
    use todo_core::actor::Actor;
    use todo_core::command::NewTask;
    use todo_core::ids::ListId;
    use todo_core::{apply, decide};

    struct World {
        state: State,
        next: u64,
    }

    impl World {
        fn new() -> Self {
            Self {
                state: State::new(),
                next: 1,
            }
        }

        fn id(&mut self) -> Uuid {
            self.next += 1;
            todo_core::ids::uuid_v7(1_790_000_000_000 + self.next, [self.next as u8; 10])
        }

        fn run(&mut self, cmd: Command) {
            let actor = Actor::owner("test").expect("actor");
            let now = "2026-11-21T10:00:00Z".parse().expect("ts");
            let events = decide(&self.state, &actor, &cmd, now).expect("valid");
            for e in &events {
                apply(&mut self.state, e).expect("applies");
            }
        }

        fn task(&mut self, title: &str) -> (ListId, TaskId) {
            let list = ListId::from_uuid(self.id());
            self.run(create_list(list, None, "Errands"));
            let id = TaskId::from_uuid(self.id());
            self.run(Command::CreateTask(Box::new(NewTask::new(id, list, title))));
            (list, id)
        }

        fn add(&mut self, task: TaskId, at: &str) -> ReminderId {
            let id = ReminderId::from_uuid(self.id());
            self.run(Command::AddReminder {
                id,
                task,
                at: ts(at),
            });
            id
        }
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    /// Saturday 21 November 2026, 10:35 at UTC+1.
    fn ctx() -> EvalContext {
        EvalContext::new(
            ts("2026-11-21T09:35:00Z"),
            "2026-11-21".parse().expect("date"),
            3600,
        )
    }

    #[test]
    fn the_row_shows_the_first_reminder_from_today_else_the_latest() {
        let mut w = World::new();
        let (_, task) = w.task("Call the dentist");
        assert!(shown(&w.state, task, &ctx()).is_none());
        let old = w.add(task, "2026-11-19T08:00:00Z");
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(old));
        let older = w.add(task, "2026-11-18T08:00:00Z");
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(old));
        // Earlier today (local) still counts as today.
        let today = w.add(task, "2026-11-20T23:30:00Z");
        let later = w.add(task, "2026-11-25T08:00:00Z");
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(today));
        w.run(delete(today));
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(later));
        let _ = older;
    }

    #[test]
    fn setting_adds_one_then_moves_it_and_removing_deletes_only_it() {
        let mut w = World::new();
        let (_, task) = w.task("Call the dentist");
        let at = ts("2026-11-21T14:00:00Z");
        let fresh = Uuid::from_u128(42);
        let add = set_reminder(&w.state, task, at, &ctx(), || fresh).expect("add");
        assert_eq!(
            add,
            Command::AddReminder {
                id: ReminderId::from_uuid(fresh),
                task,
                at
            }
        );
        w.run(add);
        let id = ReminderId::from_uuid(fresh);
        // The same time again changes nothing; another moves it, never adding a second.
        assert!(set_reminder(&w.state, task, at, &ctx(), Uuid::nil).is_none());
        let later = ts("2026-11-22T08:00:00Z");
        let moved = set_reminder(&w.state, task, later, &ctx(), Uuid::nil).expect("move");
        assert_eq!(moved, Command::UpdateReminder { id, at: later });
        w.run(moved);
        assert_eq!(w.state.reminders[&id].at, later);
        let other = w.add(task, "2026-12-01T08:00:00Z");
        let remove = remove_reminder(&w.state, task, &ctx()).expect("remove");
        assert_eq!(remove, Command::Delete { id: id.into() });
        w.run(remove);
        assert!(w.state.is_live(other.into()));
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(other));
        w.run(remove_reminder(&w.state, task, &ctx()).expect("remove"));
        assert!(remove_reminder(&w.state, task, &ctx()).is_none());
    }

    #[test]
    fn a_gone_task_takes_no_reminder() {
        let mut w = World::new();
        let (_, task) = w.task("Call the dentist");
        w.add(task, "2026-11-22T08:00:00Z");
        w.run(delete(task));
        let at = ts("2026-11-23T08:00:00Z");
        assert!(set_reminder(&w.state, task, at, &ctx(), Uuid::nil).is_none());
        assert!(remove_reminder(&w.state, task, &ctx()).is_none());
        let unknown = TaskId::from_uuid(Uuid::from_u128(9));
        assert!(set_reminder(&w.state, unknown, at, &ctx(), Uuid::nil).is_none());
    }

    #[test]
    fn alarms_are_the_live_reminders_of_open_tasks() {
        let mut w = World::new();
        let (list, task) = w.task("Call the dentist");
        let since = ts("2026-11-20T09:35:00Z");
        let past = w.add(task, "2026-11-19T08:00:00Z");
        let soon = w.add(task, "2026-11-21T14:00:00Z");
        let recent = w.add(task, "2026-11-21T08:00:00Z");
        let at = |s| ts(s);
        assert_eq!(
            alarms(&w.state, since),
            [
                Alarm {
                    reminder: recent,
                    task,
                    at: at("2026-11-21T08:00:00Z")
                },
                Alarm {
                    reminder: soon,
                    task,
                    at: at("2026-11-21T14:00:00Z")
                },
            ]
        );
        assert_eq!(alarms(&w.state, ts("2026-11-01T00:00:00Z")).len(), 3);
        let _ = past;
        // A removed reminder goes; so do all of a completed task's, and come back when it is reopened.
        w.run(delete(recent));
        assert_eq!(alarms(&w.state, since).len(), 1);
        w.run(Command::CompleteTask { id: task });
        assert!(alarms(&w.state, since).is_empty());
        w.run(Command::UncompleteTask { id: task });
        assert_eq!(alarms(&w.state, since).len(), 1);
        // A deleted task, or a task in a deleted list, has none.
        w.run(delete(task));
        assert!(alarms(&w.state, since).is_empty());
        w.run(Command::Restore { id: task.into() });
        assert_eq!(alarms(&w.state, since).len(), 1);
        w.run(delete(list));
        assert!(alarms(&w.state, since).is_empty());
    }
}
