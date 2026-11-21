//! What the user's gestures mean as `todo_core` commands. Pure: the caller passes the state, today's date
//! and a source of fresh IDs (`SyncClient::new_uuid`), and submits the result.

use todo_core::State;
use todo_core::command::{Command, NewTask, Placement, TaskPatch};
use todo_core::ids::{CollectionId, EntityId, ListId, StepId, TaskId};
use todo_core::model::{List, Priority};
use todo_core::query::fold;
use todo_core::time::Date;
use uuid::Uuid;

use super::lists;
use super::quick_add::Parsed;
use super::{RoomKey, Smart};

/// The name of the list tasks land in when the room doesn't pick one.
pub const DEFAULT_LIST: &str = "Tasks";

fn sorted_lists(state: &State, collection: Option<Option<CollectionId>>) -> Vec<&List> {
    let mut lists: Vec<&List> = state
        .lists
        .values()
        .filter(|l| state.is_live(l.id.into()))
        .filter(|l| collection.is_none_or(|c| l.collection == c))
        .collect();
    let col_pos = |l: &List| {
        l.collection
            .and_then(|c| state.collections.get(&c))
            .map(|c| c.position.clone())
    };
    // Top-level lists first, then by collection, then by position.
    lists.sort_by(|a, b| (col_pos(a), &a.position, a.id).cmp(&(col_pos(b), &b.position, b.id)));
    lists
}

/// The list new tasks go to from a smart list or the global quick-add: one named "Tasks" (or "Úkoly"),
/// else the first list.
pub fn default_list(state: &State) -> Option<ListId> {
    let lists = sorted_lists(state, None);
    lists
        .iter()
        .find(|l| matches!(fold(&l.name).as_str(), "tasks" | "ukoly" | "inbox"))
        .or(lists.first())
        .map(|l| l.id)
}

/// A fresh list, with a swatch picked from its ID.
pub fn create_list(id: ListId, collection: Option<CollectionId>, name: &str) -> Command {
    let hue = lists::hue(None, id.as_uuid().as_bytes());
    Command::CreateList {
        id,
        collection,
        name: name.trim().to_string(),
        color: Some(lists::swatch_key(hue).to_string()),
        placement: Placement::Last,
    }
}

/// Commands that add a task typed into a room's quick-add (or the global one, with `RoomKey::Smart(MyDay)`).
/// Creates the default list first if there is none to put it in. Empty titles add nothing.
pub fn add_task(
    state: &State,
    room: RoomKey,
    parsed: &Parsed,
    today: Date,
    mut new_id: impl FnMut() -> Uuid,
) -> (Option<TaskId>, Vec<Command>) {
    if parsed.title.trim().is_empty() {
        return (None, Vec::new());
    }
    let mut commands = Vec::new();
    let existing = match room {
        RoomKey::List(l) if state.is_live(l.into()) => Some(l),
        RoomKey::Collection(c) => sorted_lists(state, Some(Some(c))).first().map(|l| l.id),
        _ => default_list(state),
    };
    let list = existing.unwrap_or_else(|| {
        let id = ListId::from_uuid(new_id());
        let collection = match room {
            RoomKey::Collection(c) if state.is_live(c.into()) => Some(c),
            _ => None,
        };
        commands.push(create_list(id, collection, DEFAULT_LIST));
        id
    });
    let id = TaskId::from_uuid(new_id());
    let mut task = NewTask::new(id, list, parsed.title.trim());
    task.placement = Placement::First;
    task.due = parsed.due;
    task.priority = parsed.priority.unwrap_or_default();
    match room {
        RoomKey::Smart(Smart::MyDay) => task.my_day = Some(today),
        RoomKey::Smart(Smart::Important) if parsed.priority.is_none() => {
            task.priority = Priority::High;
        }
        RoomKey::Smart(Smart::Planned) if parsed.due.is_none() => task.due = Some(today),
        _ => {}
    }
    commands.push(Command::CreateTask(Box::new(task)));
    (Some(id), commands)
}

/// Completes an open task or reopens a done one.
pub fn toggle_complete(state: &State, id: TaskId) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    Some(if t.is_done() {
        Command::UncompleteTask { id }
    } else {
        Command::CompleteTask { id }
    })
}

/// Adds the task to today's My Day, or takes it out.
pub fn toggle_my_day(state: &State, id: TaskId, today: Date) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    Some(Command::SetMyDay {
        id,
        date: (t.my_day != Some(today)).then_some(today),
    })
}

/// Marks the task important (high priority), or takes the priority away.
pub fn toggle_important(state: &State, id: TaskId) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    let priority = if t.priority == Priority::High {
        Priority::None
    } else {
        Priority::High
    };
    Some(update_task(
        id,
        TaskPatch {
            priority: Some(priority),
            ..TaskPatch::default()
        },
    ))
}

/// The next priority: none → low → normal → high → none.
pub fn cycle_priority(state: &State, id: TaskId) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    let next = match t.priority {
        Priority::None => Priority::Low,
        Priority::Low => Priority::Normal,
        Priority::Normal => Priority::High,
        Priority::High => Priority::None,
    };
    Some(update_task(
        id,
        TaskPatch {
            priority: Some(next),
            ..TaskPatch::default()
        },
    ))
}

pub fn update_task(id: TaskId, patch: TaskPatch) -> Command {
    Command::UpdateTask { id, patch }
}

/// A new title; `None` when it is empty or unchanged.
pub fn retitle(state: &State, id: TaskId, title: &str) -> Option<Command> {
    let title = title.trim();
    let t = state.tasks.get(&id)?;
    (!title.is_empty() && title != t.title).then(|| {
        update_task(
            id,
            TaskPatch {
                title: Some(title.to_string()),
                ..TaskPatch::default()
            },
        )
    })
}

/// New notes; `None` when unchanged. Trailing whitespace is dropped.
pub fn set_notes(state: &State, id: TaskId, notes: &str) -> Option<Command> {
    let notes = notes.trim_end();
    let t = state.tasks.get(&id)?;
    (notes != t.notes).then(|| {
        update_task(
            id,
            TaskPatch {
                notes: Some(notes.to_string()),
                ..TaskPatch::default()
            },
        )
    })
}

/// Sets or clears the due date. Clearing it also clears a repeat rule, which needs a due date.
pub fn set_due(state: &State, id: TaskId, due: Option<Date>) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    if t.due == due {
        return None;
    }
    Some(update_task(
        id,
        TaskPatch {
            due: Some(due),
            recurrence: (due.is_none() && t.recurrence.is_some()).then_some(None),
            ..TaskPatch::default()
        },
    ))
}

pub fn add_step(task: TaskId, title: &str, new_id: impl FnOnce() -> Uuid) -> Option<Command> {
    let title = title.trim();
    (!title.is_empty()).then(|| Command::CreateStep {
        id: StepId::from_uuid(new_id()),
        task,
        title: title.to_string(),
        placement: Placement::Last,
    })
}

pub fn toggle_step(state: &State, id: StepId) -> Option<Command> {
    let s = state.steps.get(&id)?;
    Some(if s.completed_at.is_some() {
        Command::UncompleteStep { id }
    } else {
        Command::CompleteStep { id }
    })
}

pub fn retitle_step(state: &State, id: StepId, title: &str) -> Option<Command> {
    let title = title.trim();
    let s = state.steps.get(&id)?;
    (!title.is_empty() && title != s.title).then(|| Command::UpdateStep {
        id,
        title: title.to_string(),
    })
}

pub fn rename_list(state: &State, id: ListId, name: &str) -> Option<Command> {
    let name = name.trim();
    let l = state.lists.get(&id)?;
    (!name.is_empty() && name != l.name).then(|| Command::UpdateList {
        id,
        name: Some(name.to_string()),
        color: None,
    })
}

pub fn rename_collection(state: &State, id: CollectionId, name: &str) -> Option<Command> {
    let name = name.trim();
    let c = state.collections.get(&id)?;
    (!name.is_empty() && name != c.name).then(|| Command::UpdateCollection {
        id,
        name: Some(name.to_string()),
        color: None,
    })
}

pub fn delete(id: impl Into<EntityId>) -> Command {
    Command::Delete { id: id.into() }
}

/// Moves an item one place up (`-1`) or down (`1`) among its live siblings; `None` at either end.
pub fn nudge(state: &State, id: EntityId, by: i32) -> Option<Command> {
    let scope = state.scope_of(id)?;
    let siblings: Vec<EntityId> = state
        .siblings(scope)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| state.is_live(*e))
        .collect();
    let i = siblings.iter().position(|e| *e == id)?;
    let placement = if by < 0 {
        Placement::Before(*siblings.get(i.checked_sub(1)?)?)
    } else {
        Placement::After(*siblings.get(i + 1)?)
    };
    Some(Command::Reorder { id, placement })
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::actor::Actor;
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

        fn list(&mut self, name: &str) -> ListId {
            let id = ListId::from_uuid(self.id());
            self.run(create_list(id, None, name));
            id
        }
    }

    fn today() -> Date {
        "2026-11-21".parse().expect("date")
    }

    fn parsed(title: &str) -> Parsed {
        Parsed {
            title: title.into(),
            due: None,
            priority: None,
        }
    }

    #[test]
    fn quick_add_creates_a_default_list_when_there_is_none() {
        let mut w = World::new();
        let state = w.state.clone();
        let (task, cmds) = add_task(
            &state,
            RoomKey::Smart(Smart::MyDay),
            &parsed("Milk"),
            today(),
            || w.id(),
        );
        assert_eq!(cmds.len(), 2);
        for c in cmds {
            w.run(c);
        }
        let t = &w.state.tasks[&task.expect("task")];
        assert_eq!(t.my_day, Some(today()));
        assert_eq!(w.state.lists[&t.list].name, DEFAULT_LIST);
        // The next one reuses it.
        let state = w.state.clone();
        let (_, cmds) = add_task(
            &state,
            RoomKey::Smart(Smart::All),
            &parsed("Bread"),
            today(),
            || w.id(),
        );
        assert_eq!(cmds.len(), 1);
        // Empty titles add nothing.
        assert!(
            add_task(
                &w.state,
                RoomKey::Smart(Smart::All),
                &parsed("  "),
                today(),
                Uuid::nil
            )
            .1
            .is_empty()
        );
    }

    #[test]
    fn rooms_shape_new_tasks() {
        let mut w = World::new();
        let errands = w.list("Errands");
        let inbox = w.list("Tasks");
        let state = w.state.clone();
        let (id, cmds) = add_task(
            &state,
            RoomKey::List(errands),
            &parsed("Stamps"),
            today(),
            || w.id(),
        );
        for c in cmds {
            w.run(c);
        }
        assert_eq!(w.state.tasks[&id.expect("id")].list, errands);
        let state = w.state.clone();
        let (id, cmds) = add_task(
            &state,
            RoomKey::Smart(Smart::Important),
            &parsed("Taxes"),
            today(),
            || w.id(),
        );
        for c in cmds {
            w.run(c);
        }
        let t = &w.state.tasks[&id.expect("id")];
        assert_eq!((t.list, t.priority), (inbox, Priority::High));
        let state = w.state.clone();
        let (id, cmds) = add_task(
            &state,
            RoomKey::Smart(Smart::Planned),
            &parsed("Dentist"),
            today(),
            || w.id(),
        );
        for c in cmds {
            w.run(c);
        }
        assert_eq!(w.state.tasks[&id.expect("id")].due, Some(today()));
        // New tasks go to the top of the list.
        let state = w.state.clone();
        let (first, cmds) = add_task(
            &state,
            RoomKey::List(errands),
            &parsed("Keys"),
            today(),
            || w.id(),
        );
        for c in cmds {
            w.run(c);
        }
        let mut order: Vec<_> = w
            .state
            .tasks
            .values()
            .filter(|t| t.list == errands)
            .collect();
        order.sort_by(|a, b| a.position.cmp(&b.position));
        assert_eq!(Some(order[0].id), first);
    }

    #[test]
    fn toggles_and_edits_round_trip() {
        let mut w = World::new();
        let list = w.list("Errands");
        let state = w.state.clone();
        let (id, cmds) = add_task(
            &state,
            RoomKey::List(list),
            &parsed("Stamps"),
            today(),
            || w.id(),
        );
        for c in cmds {
            w.run(c);
        }
        let id = id.expect("id");
        w.run(toggle_complete(&w.state, id).expect("cmd"));
        assert!(w.state.tasks[&id].is_done());
        w.run(toggle_complete(&w.state, id).expect("cmd"));
        assert!(!w.state.tasks[&id].is_done());
        w.run(toggle_my_day(&w.state, id, today()).expect("cmd"));
        assert_eq!(w.state.tasks[&id].my_day, Some(today()));
        w.run(toggle_my_day(&w.state, id, today()).expect("cmd"));
        assert_eq!(w.state.tasks[&id].my_day, None);
        w.run(toggle_important(&w.state, id).expect("cmd"));
        assert_eq!(w.state.tasks[&id].priority, Priority::High);
        w.run(cycle_priority(&w.state, id).expect("cmd"));
        assert_eq!(w.state.tasks[&id].priority, Priority::None);
        assert!(retitle(&w.state, id, " Stamps ").is_none());
        w.run(retitle(&w.state, id, "Stamps and envelopes").expect("cmd"));
        w.run(set_notes(&w.state, id, "Two of each\nfrom the post office\n").expect("cmd"));
        assert_eq!(
            w.state.tasks[&id].notes,
            "Two of each\nfrom the post office"
        );
        w.run(set_due(&w.state, id, Some(today())).expect("cmd"));
        assert!(set_due(&w.state, id, Some(today())).is_none());
        w.run(set_due(&w.state, id, None).expect("cmd"));
        assert_eq!(w.state.tasks[&id].due, None);
    }

    #[test]
    fn steps_and_lists_reorder_among_live_siblings() {
        let mut w = World::new();
        let a = w.list("A");
        let b = w.list("B");
        let c = w.list("C");
        let order = |s: &State| -> Vec<ListId> {
            let mut l: Vec<_> = s.lists.values().filter(|l| l.deleted.is_none()).collect();
            l.sort_by(|x, y| x.position.cmp(&y.position));
            l.iter().map(|l| l.id).collect()
        };
        assert!(nudge(&w.state, a.into(), -1).is_none());
        w.run(nudge(&w.state, c.into(), -1).expect("cmd"));
        assert_eq!(order(&w.state), vec![a, c, b]);
        w.run(nudge(&w.state, a.into(), 1).expect("cmd"));
        assert_eq!(order(&w.state), vec![c, a, b]);
        w.run(delete(a));
        // The deleted list is skipped: B moves above C.
        w.run(nudge(&w.state, b.into(), -1).expect("cmd"));
        assert_eq!(order(&w.state), vec![b, c]);
        w.run(rename_list(&w.state, b, "Bee").expect("cmd"));
        assert_eq!(w.state.lists[&b].name, "Bee");

        let state = w.state.clone();
        let (task, cmds) = add_task(&state, RoomKey::List(b), &parsed("Trip"), today(), || {
            w.id()
        });
        for cmd in cmds {
            w.run(cmd);
        }
        let task = task.expect("task");
        let s1 = add_step(task, "Pack", || w.id()).expect("step");
        let s2 = add_step(task, "Go", || w.id()).expect("step");
        let (Command::CreateStep { id: s1id, .. }, Command::CreateStep { id: s2id, .. }) =
            (&s1, &s2)
        else {
            panic!("steps");
        };
        let (s1id, s2id) = (*s1id, *s2id);
        w.run(s1);
        w.run(s2);
        w.run(nudge(&w.state, s2id.into(), -1).expect("cmd"));
        assert!(w.state.steps[&s2id].position < w.state.steps[&s1id].position);
        w.run(toggle_step(&w.state, s1id).expect("cmd"));
        assert!(w.state.steps[&s1id].completed_at.is_some());
        w.run(retitle_step(&w.state, s1id, "Pack light").expect("cmd"));
        w.run(delete(s2id));
        assert!(w.state.steps[&s2id].deleted.is_some());
        assert!(add_step(task, " ", Uuid::nil).is_none());

        let work = CollectionId::from_uuid(w.id());
        w.run(Command::CreateCollection {
            id: work,
            name: "Work".into(),
            color: None,
            placement: Placement::Last,
        });
        assert!(rename_collection(&w.state, work, " Work ").is_none());
        w.run(rename_collection(&w.state, work, "Day job").expect("cmd"));
        assert_eq!(w.state.collections[&work].name, "Day job");
    }
}
