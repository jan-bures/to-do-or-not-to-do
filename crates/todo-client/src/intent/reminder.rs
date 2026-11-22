//! Reminders: which one a task's Reminder row shows and edits, the commands that set, move or remove it,
//! and the alarms a client schedules from the synced reminders.
//!
//! A task can carry several reminders (an agent or the API can add more). The detail pane shows one: the
//! first on or after the owner's local today, else the latest. Setting a time moves that one; a task
//! without one gets a new one. Removing deletes that one only.
//!
//! An edit names the reminder it was made on ([`Seen`]: its ID and the time the picker showed), so a
//! picker left open while the reminder changed elsewhere never moves or deletes another reminder, or a
//! time the owner didn't see: the write is refused as [`Stale`] instead.

use todo_core::State;
use todo_core::command::Command;
use todo_core::ids::{ReminderId, TaskId};
use todo_core::model::Reminder;
use todo_core::query::EvalContext;
use todo_core::time::Timestamp;
use uuid::Uuid;

/// Shown when the reminder no longer matches what the editor was opened on.
pub const REMINDER_CHANGED: &str =
    "The reminder changed while the picker was open. Check it, then try again.";

/// Shown when the chosen time has passed.
pub const REMINDER_PAST: &str = "Pick a time that hasn’t passed.";

/// Confirms a reminder was set, with a time label formatted in the client's local time.
pub fn reminder_set_text(time: &str) -> String {
    format!("Reminder set for {time}")
}

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

/// The reminder an edit was made on, as the row showed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The reminder.
    pub id: ReminderId,
    /// Its time when the picker opened.
    pub at: Timestamp,
}

impl Seen {
    /// What the row shows for `task` now (see [`shown`]).
    pub fn of(state: &State, task: TaskId, ctx: &EvalContext) -> Option<Self> {
        shown(state, task, ctx).map(|r| Self { id: r.id, at: r.at })
    }
}

/// Why an edit wasn't made: what it was made on has changed since.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Stale {
    /// The task is gone (deleted, or in a deleted list).
    #[error("the task is gone")]
    Task,
    /// The reminder was moved or removed, or (for a task that had none) one was added.
    #[error("the reminder changed")]
    Reminder,
}

/// Checks that what the edit was made on still stands: the task is live, and `seen` is a live reminder
/// of it at the same time, or (`None`) the task still has no reminder.
pub fn check_seen(state: &State, task: TaskId, seen: Option<Seen>) -> Result<(), Stale> {
    if !state.is_live(task.into()) {
        return Err(Stale::Task);
    }
    let unchanged = match seen {
        Some(s) => state
            .reminders
            .get(&s.id)
            .is_some_and(|r| r.task == task && r.at == s.at && state.is_live(s.id.into())),
        None => live(state, task).is_empty(),
    };
    unchanged.then_some(()).ok_or(Stale::Reminder)
}

/// Sets the reminder the owner saw (`seen`) to `at`, or adds one to a task that had none (`None`).
/// `Ok(None)` when it is already at `at`; [`Stale`] when what the owner saw has changed since.
pub fn set_reminder(
    state: &State,
    task: TaskId,
    seen: Option<Seen>,
    at: Timestamp,
    new_id: impl FnOnce() -> Uuid,
) -> Result<Option<Command>, Stale> {
    check_seen(state, task, seen)?;
    Ok(match seen {
        Some(s) if s.at == at => None,
        Some(s) => Some(Command::UpdateReminder { id: s.id, at }),
        None => Some(Command::AddReminder {
            id: ReminderId::from_uuid(new_id()),
            task,
            at,
        }),
    })
}

/// Removes the reminder the owner saw (others stay); [`Stale`] when it has changed or gone since.
pub fn remove_reminder(state: &State, task: TaskId, seen: Seen) -> Result<Command, Stale> {
    check_seen(state, task, Some(seen))?;
    Ok(Command::Delete { id: seen.id.into() })
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
        w.add(task, "2026-11-18T08:00:00Z");
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(old));
        // Earlier today (local) still counts as today.
        let today = w.add(task, "2026-11-20T23:30:00Z");
        let later = w.add(task, "2026-11-25T08:00:00Z");
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(today));
        w.run(delete(today));
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(later));
    }

    #[test]
    fn setting_adds_one_then_moves_it_and_removing_deletes_only_it() {
        let mut w = World::new();
        let (_, task) = w.task("Call the dentist");
        let at = ts("2026-11-21T14:00:00Z");
        let fresh = Uuid::from_u128(42);
        let add = set_reminder(&w.state, task, None, at, || fresh)
            .expect("unchanged")
            .expect("add");
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
        let seen = Seen::of(&w.state, task, &ctx()).expect("shown");
        assert_eq!(seen, Seen { id, at });
        // The same time again changes nothing; another moves it, never adding a second.
        assert_eq!(
            set_reminder(&w.state, task, Some(seen), at, Uuid::nil),
            Ok(None)
        );
        let later = ts("2026-11-22T08:00:00Z");
        let moved = set_reminder(&w.state, task, Some(seen), later, Uuid::nil)
            .expect("unchanged")
            .expect("move");
        assert_eq!(moved, Command::UpdateReminder { id, at: later });
        w.run(moved);
        assert_eq!(w.state.reminders[&id].at, later);
        let other = w.add(task, "2026-12-01T08:00:00Z");
        let seen = Seen { id, at: later };
        let remove = remove_reminder(&w.state, task, seen).expect("remove");
        assert_eq!(remove, Command::Delete { id: id.into() });
        w.run(remove);
        assert!(w.state.is_live(other.into()));
        assert_eq!(shown(&w.state, task, &ctx()).map(|r| r.id), Some(other));
        // Removing it again: it is gone, and the other one is never removed in its place.
        assert_eq!(remove_reminder(&w.state, task, seen), Err(Stale::Reminder));
    }

    #[test]
    fn an_edit_on_a_reminder_that_changed_meanwhile_writes_nothing() {
        let mut w = World::new();
        let (_, task) = w.task("Call the dentist");
        let id = w.add(task, "2026-11-22T08:00:00Z");
        let seen = Seen::of(&w.state, task, &ctx()).expect("shown");
        let at = ts("2026-11-23T08:00:00Z");
        // Moved elsewhere while the picker was open.
        w.run(Command::UpdateReminder {
            id,
            at: ts("2026-11-22T09:00:00Z"),
        });
        assert_eq!(
            set_reminder(&w.state, task, Some(seen), at, Uuid::nil),
            Err(Stale::Reminder)
        );
        assert_eq!(remove_reminder(&w.state, task, seen), Err(Stale::Reminder));
        // Removed elsewhere, with another left: the other is neither moved nor removed.
        let seen = Seen::of(&w.state, task, &ctx()).expect("shown");
        let other = w.add(task, "2026-11-25T08:00:00Z");
        w.run(delete(id));
        assert_eq!(
            set_reminder(&w.state, task, Some(seen), at, Uuid::nil),
            Err(Stale::Reminder)
        );
        assert_eq!(remove_reminder(&w.state, task, seen), Err(Stale::Reminder));
        // A task that had none got one elsewhere: adding a second is refused too.
        let (_, bare) = w.task("Water the plants");
        w.add(bare, "2026-11-22T08:00:00Z");
        assert_eq!(
            set_reminder(&w.state, bare, None, at, Uuid::nil),
            Err(Stale::Reminder)
        );
        // A reminder of another task is never edited through this one.
        let theirs = Seen::of(&w.state, bare, &ctx()).expect("shown");
        assert_eq!(
            remove_reminder(&w.state, task, theirs),
            Err(Stale::Reminder)
        );
        assert!(w.state.is_live(other.into()));
    }

    #[test]
    fn a_gone_task_takes_no_reminder() {
        let mut w = World::new();
        let (_, task) = w.task("Call the dentist");
        w.add(task, "2026-11-22T08:00:00Z");
        let seen = Seen::of(&w.state, task, &ctx()).expect("shown");
        w.run(delete(task));
        let at = ts("2026-11-23T08:00:00Z");
        assert_eq!(
            set_reminder(&w.state, task, Some(seen), at, Uuid::nil),
            Err(Stale::Task)
        );
        assert_eq!(remove_reminder(&w.state, task, seen), Err(Stale::Task));
        let unknown = TaskId::from_uuid(Uuid::from_u128(9));
        assert_eq!(
            set_reminder(&w.state, unknown, None, at, Uuid::nil),
            Err(Stale::Task)
        );
    }

    #[test]
    fn alarms_are_the_live_reminders_of_open_tasks() {
        let mut w = World::new();
        let (list, task) = w.task("Call the dentist");
        let since = ts("2026-11-20T09:35:00Z");
        w.add(task, "2026-11-19T08:00:00Z");
        let soon = w.add(task, "2026-11-21T14:00:00Z");
        let recent = w.add(task, "2026-11-21T08:00:00Z");
        assert_eq!(
            alarms(&w.state, since),
            [
                Alarm {
                    reminder: recent,
                    task,
                    at: ts("2026-11-21T08:00:00Z")
                },
                Alarm {
                    reminder: soon,
                    task,
                    at: ts("2026-11-21T14:00:00Z")
                },
            ]
        );
        assert_eq!(alarms(&w.state, ts("2026-11-01T00:00:00Z")).len(), 3);
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
