//! Trash over the optimistic replica. Only explicit tombstones get rows: restoring a
//! parent reveals its children that were not themselves deleted. Merge sources are
//! not independent tasks, so they can only be brought back by undoing the merge.

use std::cmp::Reverse;

use todo_core::ids::EntityId;
use todo_core::time::Timestamp;
use todo_core::{Command, State};

use crate::activity::relative_time;
use crate::intent::confirm::one_line;

pub const TITLE: &str = "Trash";
pub const DESCRIPTION: &str = "Restore deleted items to where they were.";
pub const EMPTY: &str = "Nothing in Trash.";
pub const CHANGED: &str = "This item can no longer be restored. Check Trash and try again.";

/// One directly deleted task, list or collection in the owner's replica.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: EntityId,
    pub title: String,
    pub deleted_at: Timestamp,
    /// Deletion age, entity kind and original parent name (where applicable).
    pub metadata: String,
    /// Why Restore is unavailable. The parent has its own independently undoable Restore.
    pub blocked: Option<&'static str>,
}

fn parent_block(state: &State, parent: Option<EntityId>) -> Option<&'static str> {
    let parent = parent?;
    if state.is_live(parent) {
        return None;
    }
    match parent {
        EntityId::List(id) if state.lists.get(&id).is_some_and(|l| l.deleted.is_none()) => {
            Some("Restore its collection first.")
        }
        EntityId::List(_) => Some("Restore its list first."),
        _ => Some("Restore its collection first."),
    }
}

/// Newest first, with the typed ID breaking ties consistently across replicas.
#[must_use]
pub fn rows(state: &State, now: Timestamp) -> Vec<Row> {
    let tasks = state.tasks.values().filter_map(|t| {
        let deleted = t.deleted.as_ref()?;
        if t.merged_into.is_some() {
            return None;
        }
        Some((
            EntityId::Task(t.id),
            t.title.as_str(),
            deleted.at,
            "Task",
            Some(EntityId::List(t.list)),
            state.lists.get(&t.list).map(|l| l.name.as_str()),
        ))
    });
    let lists = state.lists.values().filter_map(|l| {
        Some((
            EntityId::List(l.id),
            l.name.as_str(),
            l.deleted.as_ref()?.at,
            "List",
            l.collection.map(EntityId::Collection),
            l.collection
                .and_then(|id| state.collections.get(&id))
                .map(|c| c.name.as_str()),
        ))
    });
    let collections = state.collections.values().filter_map(|c| {
        Some((
            EntityId::Collection(c.id),
            c.name.as_str(),
            c.deleted.as_ref()?.at,
            "Collection",
            None,
            None,
        ))
    });
    let mut rows: Vec<_> = tasks
        .chain(lists)
        .chain(collections)
        .map(|(id, title, deleted_at, kind, parent, name)| {
            let age = relative_time(deleted_at, now);
            let mut metadata = format!(
                "{kind} · Deleted {}",
                if age == "Just now" { "just now" } else { &age }
            );
            if let Some(name) = name {
                metadata.push_str(" · from ");
                metadata.push_str(&one_line(name));
            }
            Row {
                id,
                title: one_line(title),
                deleted_at,
                metadata,
                blocked: parent_block(state, parent),
            }
        })
        .collect();
    rows.sort_unstable_by_key(|r| (Reverse(r.deleted_at), r.id));
    rows
}

/// Checks the clicked tombstone against the current view. On native clients this also
/// runs under the submit lock. The domain still validates Restore at the server.
#[must_use]
pub fn restore(state: &State, id: EntityId, deleted_at: Timestamp) -> Option<Command> {
    let parent = match id {
        EntityId::Task(id) => Some(
            state
                .tasks
                .get(&id)
                .filter(|t| t.merged_into.is_none())?
                .list
                .into(),
        ),
        EntityId::List(id) => state.lists.get(&id)?.collection.map(EntityId::Collection),
        EntityId::Collection(_) => None,
        _ => return None,
    };
    (state.tombstone(id)?.at == deleted_at && parent.is_none_or(|p| state.is_live(p)))
        .then_some(Command::Restore { id })
}

/// Counts for the status bar; today's deletions use the owner's local day.
#[must_use]
pub fn status(
    rows: &[Row],
    now: &crate::room_header::LocalTime,
) -> crate::room_header::StatusCounts {
    let today = now.today();
    crate::room_header::StatusCounts {
        first: (rows.len(), if rows.len() == 1 { "item" } else { "items" }),
        second: (
            rows.iter()
                .filter(|r| now.date_of(r.deleted_at) == today)
                .count(),
            "deleted today",
        ),
    }
}

#[must_use]
pub fn restored(title: &str) -> String {
    format!("Restored “{title}”")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{create_collection, create_list};
    use proptest::prelude::*;
    use todo_core::command::{NewTask, Placement};
    use todo_core::ids::{CollectionId, ListId, TaskId};
    use todo_core::{Actor, apply, decide, undo::undo_batch};

    fn ts(n: i64) -> Timestamp {
        Timestamp::from_second(n).unwrap()
    }
    fn owner() -> Actor {
        Actor::owner("test").unwrap()
    }

    fn run(state: &mut State, time: i64, command: Command) -> Vec<todo_core::Event> {
        let events = decide(state, &owner(), &command, ts(time)).unwrap();
        for e in &events {
            apply(state, e).unwrap();
        }
        events
    }

    fn setup() -> (State, CollectionId, ListId, TaskId, TaskId) {
        let mut s = State::new();
        let col = CollectionId::derive(ts(0), b"col");
        let list = ListId::derive(ts(0), b"list");
        let a = TaskId::derive(ts(0), b"task A");
        let b = TaskId::derive(ts(0), b"task B");
        run(&mut s, 1, create_collection(col, "Work"));
        run(&mut s, 2, create_list(list, Some(col), "Plans"));
        run(
            &mut s,
            3,
            Command::CreateTask(Box::new(NewTask::new(a, list, "Write the plan"))),
        );
        (s, col, list, a, b)
    }

    #[test]
    fn only_direct_deletions_appear_and_parents_are_restored_explicitly() {
        let (mut s, col, list, a, b) = setup();
        run(
            &mut s,
            4,
            Command::CreateTask(Box::new(NewTask::new(b, list, "Pack the proofs"))),
        );
        assert!(rows(&s, ts(4)).is_empty());
        run(&mut s, 5, Command::Delete { id: a.into() });
        run(&mut s, 5, Command::Delete { id: list.into() });
        run(&mut s, 6, Command::Delete { id: col.into() });
        let r = rows(&s, ts(3606));
        assert_eq!(
            r.iter().map(|r| r.id).collect::<Vec<_>>(),
            [col.into(), list.into(), a.into()]
        );
        assert_eq!(r[2].metadata, "Task · Deleted 1 hr ago · from Plans");
        assert_eq!(r[2].blocked, Some("Restore its list first."));
        assert_eq!(r[1].blocked, Some("Restore its collection first."));
        assert!(!s.is_live(b.into()));
        assert!(restore(&s, a.into(), ts(5)).is_none());
        let command = restore(&s, col.into(), ts(6)).unwrap();
        run(&mut s, 7, command);
        let command = restore(&s, list.into(), ts(5)).unwrap();
        run(&mut s, 8, command);
        assert!(s.is_live(b.into()));
        assert!(restore(&s, a.into(), ts(4)).is_none());
        let command = restore(&s, a.into(), ts(5)).unwrap();
        run(&mut s, 9, command);
        assert_eq!(s.tasks[&a].list, list);
        assert!(rows(&s, ts(9)).is_empty());
    }

    #[test]
    fn a_merge_source_is_not_a_restorable_task() {
        let (mut s, _, list, a, b) = setup();
        run(
            &mut s,
            4,
            Command::CreateTask(Box::new(NewTask::new(b, list, "Target"))),
        );
        run(&mut s, 5, Command::Delete { id: a.into() });
        s.tasks.get_mut(&a).unwrap().merged_into = Some(b);
        assert!(rows(&s, ts(6)).is_empty());
        assert!(restore(&s, a.into(), ts(5)).is_none());
    }

    proptest! {
        #[test]
        fn restoring_then_undoing_restores_the_original_trash_and_data(deleted in 10i64..10000, key_taken in any::<bool>()) {
            let (mut s, _, list, a, b) = setup();
            run(&mut s, deleted, Command::Delete { id: a.into() });
            if key_taken {
                let mut task = NewTask::new(b, list, "Uses the old key");
                task.placement = Placement::First;
                run(&mut s, deleted + 1, Command::CreateTask(Box::new(task)));
                prop_assert_eq!(&s.tasks[&a].position, &s.tasks[&b].position);
            }
            let before = s.clone();
            let r = rows(&s, ts(deleted + 59));
            let command = restore(&s, r[0].id, r[0].deleted_at).unwrap();
            let restored = run(&mut s, deleted + 2, command);
            prop_assert!(s.is_live(a.into()));
            prop_assert!(rows(&s, ts(deleted + 59)).is_empty());
            let undo = undo_batch(&s, &restored, &owner(), ts(deleted + 3), Default::default()).unwrap();
            for e in &undo { apply(&mut s, e).unwrap(); }
            prop_assert_eq!(rows(&s, ts(deleted + 59)), r);
            prop_assert!(s.data_eq(&before));
        }
    }
}
