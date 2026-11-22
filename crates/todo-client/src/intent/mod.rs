//! What the user's gestures mean as `todo_core` commands. Pure: clients pass the state, local clock
//! values and a source of fresh IDs, and submit the result.
//!
//! Submit the returned commands in order. `SyncClient::submit` queues one command at a time, so when
//! quick-add needs to create a destination list, clients (including desktop) submit the list first
//! and the task second. This is not atomic: the list can remain if the task is refused.

pub mod organize;
pub mod quick_add;
pub mod reminder;
pub mod repeat;
pub mod suggest;

pub use organize::*;

use todo_core::State;
use todo_core::command::{Command, NewTask, Placement, TaskPatch};
use todo_core::ids::{CollectionId, EntityId, ListId, StepId, TaskId};
use todo_core::model::{List, Priority};
use todo_core::query::{EvalContext, fold};
use todo_core::time::Date;
use uuid::Uuid;

use quick_add::Parsed;

/// The room's effect on the destination and defaults of a new task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskRoom {
    MyDay,
    Important,
    Planned,
    /// A smart list that does not initialize any special fields.
    Other,
    Collection(CollectionId),
    List(ListId),
}

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
    let swatch = todo_design::hues::swatch_for_seed(id.as_uuid().as_bytes());
    Command::CreateList {
        id,
        collection,
        name: name.trim().to_string(),
        color: Some(swatch.key.to_owned()),
        placement: Placement::Last,
    }
}

/// The target collection for a contextual New list action. A list gets a sibling; smart rooms
/// create top-level lists.
pub fn list_collection(state: &State, room: TaskRoom) -> Option<CollectionId> {
    match room {
        TaskRoom::Collection(id) => Some(id),
        TaskRoom::List(id) if state.is_live(id.into()) => state.lists.get(&id)?.collection,
        _ => None,
    }
    .filter(|id| state.is_live((*id).into()))
}

/// Parses and builds a quick-add submission using the same local date for syntax and defaults.
pub fn quick_add_task(
    state: &State,
    room: impl Into<TaskRoom>,
    text: &str,
    ctx: &EvalContext,
    new_id: impl FnMut() -> Uuid,
) -> (Option<TaskId>, Vec<Command>) {
    add_task(state, room, &quick_add::parse(text, ctx), ctx.today, new_id)
}

/// Commands that add a task to a room. Creates a list first if there is none to put it in.
/// Empty titles add nothing.
pub fn add_task(
    state: &State,
    room: impl Into<TaskRoom>,
    parsed: &Parsed,
    today: Date,
    mut new_id: impl FnMut() -> Uuid,
) -> (Option<TaskId>, Vec<Command>) {
    if parsed.title.trim().is_empty() {
        return (None, Vec::new());
    }
    let room = room.into();
    let mut commands = Vec::new();
    let existing = match room {
        TaskRoom::List(l) if state.is_live(l.into()) => Some(l),
        TaskRoom::Collection(c) => sorted_lists(state, Some(Some(c))).first().map(|l| l.id),
        _ => default_list(state),
    };
    let list = existing.unwrap_or_else(|| {
        let id = ListId::from_uuid(new_id());
        commands.push(create_list(id, list_collection(state, room), DEFAULT_LIST));
        id
    });
    let id = TaskId::from_uuid(new_id());
    let mut task = NewTask::new(id, list, parsed.title.trim());
    task.placement = Placement::First;
    task.due = parsed.due;
    task.priority = parsed.priority.unwrap_or_default();
    match room {
        TaskRoom::MyDay => task.my_day = Some(today),
        TaskRoom::Important if parsed.priority.is_none() => task.priority = Priority::High,
        TaskRoom::Planned if parsed.due.is_none() => task.due = Some(today),
        _ => {}
    }
    commands.push(Command::CreateTask(Box::new(task)));
    (Some(id), commands)
}

/// Completes an open task or reopens a done one.
pub fn toggle_complete(state: &State, id: TaskId) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    set_done(state, id, !t.is_done())
}

/// Requests a specific completion state; a repeated request is a no-op.
/// Unknown IDs are left for the domain to reject on submission.
pub fn set_done(state: &State, id: TaskId, done: bool) -> Option<Command> {
    (!state.tasks.get(&id).is_some_and(|t| t.is_done() == done)).then_some(if done {
        Command::CompleteTask { id }
    } else {
        Command::UncompleteTask { id }
    })
}

/// Sets the task's My Day date, or clears it.
pub fn set_my_day(id: TaskId, date: Option<Date>) -> Command {
    Command::SetMyDay { id, date }
}

/// Adds the task to today's My Day, or takes it out.
pub fn toggle_my_day(state: &State, id: TaskId, today: Date) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    Some(set_my_day(id, (t.my_day != Some(today)).then_some(today)))
}

/// Marks the task important (high priority), or takes the priority away.
pub fn toggle_important(state: &State, id: TaskId) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    set_important(state, id, t.priority != Priority::High)
}

/// Sets the star state without clearing a lower priority when the star is already off.
pub fn set_important(state: &State, id: TaskId, on: bool) -> Option<Command> {
    (!state
        .tasks
        .get(&id)
        .is_some_and(|t| (t.priority == Priority::High) == on))
    .then(|| set_priority(id, if on { Priority::High } else { Priority::None }))
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
    Some(set_priority(id, next))
}

pub fn set_priority(id: TaskId, priority: Priority) -> Command {
    update_task(
        id,
        TaskPatch {
            priority: Some(priority),
            ..TaskPatch::default()
        },
    )
}

/// Sets the priority (the detail pane's picker); `None` when it already has it.
pub fn pick_priority(state: &State, id: TaskId, priority: Priority) -> Option<Command> {
    let t = state.tasks.get(&id)?;
    (t.priority != priority).then(|| {
        update_task(
            id,
            TaskPatch {
                priority: Some(priority),
                ..TaskPatch::default()
            },
        )
    })
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
    set_step_done(state, id, s.completed_at.is_none())
}

/// Requests a specific step completion state; a repeated request is a no-op.
pub fn set_step_done(state: &State, id: StepId, done: bool) -> Option<Command> {
    (!state
        .steps
        .get(&id)
        .is_some_and(|s| s.completed_at.is_some() == done))
    .then_some(if done {
        Command::CompleteStep { id }
    } else {
        Command::UncompleteStep { id }
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

/// Restores a deleted step without changing its title, completion or parent.
pub fn restore_step(id: StepId) -> Command {
    Command::Restore { id: id.into() }
}

/// Deletes a live task (Undo brings it back); `None` when it is already gone.
pub fn delete_task(state: &State, id: TaskId) -> Option<Command> {
    state.is_live(id.into()).then(|| delete(id))
}

/// Moves a live task to another live list, first in that list's manual order, as a new task would be;
/// `None` when it is already there or either is gone.
pub fn move_to_list(state: &State, id: TaskId, list: ListId) -> Option<Command> {
    let task = state.tasks.get(&id)?;
    (task.list != list && state.is_live(id.into()) && state.is_live(list.into())).then_some(
        Command::MoveTask {
            id,
            list,
            placement: Placement::First,
        },
    )
}

/// Where a task can be moved, grouped as the sidebar shows them: each live collection in order with
/// its live lists (empty ones left out), then the lists outside any collection.
pub fn list_groups(state: &State) -> Vec<(Option<CollectionId>, Vec<ListId>)> {
    let mut collections: Vec<_> = state
        .collections
        .values()
        .filter(|c| state.is_live(c.id.into()))
        .collect();
    collections.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
    let lists_in = |c: Option<CollectionId>| -> Vec<ListId> {
        let mut lists: Vec<&List> = state
            .lists
            .values()
            .filter(|l| l.collection == c && state.is_live(l.id.into()))
            .collect();
        lists.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
        lists.iter().map(|l| l.id).collect()
    };
    collections
        .iter()
        .map(|c| Some(c.id))
        .chain([None])
        .map(|c| (c, lists_in(c)))
        .filter(|(_, lists)| !lists.is_empty())
        .collect()
}

/// Moves an open task past its visible neighbour in a manual list. Completed and filtered-out
/// siblings need not be adjacent in the underlying order. A stale tap never moves a task in a
/// different list, or swaps a pair that has already changed places.
pub fn reorder_open_task(
    state: &State,
    list: ListId,
    id: TaskId,
    neighbour: TaskId,
    by: i32,
) -> Option<Command> {
    let open = |id: TaskId| {
        state
            .tasks
            .get(&id)
            .filter(|t| t.list == list && !t.is_done() && state.is_live(id.into()))
    };
    let (task, other) = (open(id)?, open(neighbour)?);
    let ordering = (&task.position, id).cmp(&(&other.position, neighbour));
    let placement = match by {
        -1 if ordering.is_gt() => Placement::Before(neighbour.into()),
        1 if ordering.is_lt() => Placement::After(neighbour.into()),
        _ => return None,
    };
    Some(Command::Reorder {
        id: id.into(),
        placement,
    })
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

    impl World {
        fn task(&mut self, list: ListId, title: &str) -> TaskId {
            let id = TaskId::from_uuid(self.id());
            self.run(Command::CreateTask(Box::new(NewTask::new(id, list, title))));
            id
        }
    }

    #[test]
    fn reordering_visible_tasks_skips_hidden_rows_and_rejects_stale_neighbours() {
        let mut w = World::new();
        let list = w.list("Garden");
        let other = w.list("Home");
        let a = w.task(list, "A");
        let hidden = w.task(list, "Hidden");
        let b = w.task(list, "B");
        w.run(Command::CompleteTask { id: hidden });
        assert!(reorder_open_task(&w.state, list, a, hidden, 1).is_none());
        assert!(reorder_open_task(&w.state, other, a, b, 1).is_none());
        let move_down = reorder_open_task(&w.state, list, a, b, 1).expect("move down");
        assert_eq!(
            move_down,
            Command::Reorder {
                id: a.into(),
                placement: Placement::After(b.into())
            }
        );
        w.run(move_down);
        assert!(reorder_open_task(&w.state, list, a, b, 1).is_none());
        assert!(reorder_open_task(&w.state, list, a, b, -1).is_some());
        w.run(move_to_list(&w.state, b, other).expect("change list"));
        assert!(reorder_open_task(&w.state, list, a, b, -1).is_none());
    }

    #[test]
    fn suggestions_learn_live_open_and_completed_tasks_and_recheck_the_destination() {
        use suggest::ListSuggester;
        let mut w = World::new();
        let tools = w.list("Tools");
        let seeds = w.list("Seeds");
        for _ in 0..3 {
            w.task(tools, "blue wrench");
        }
        w.task(seeds, "žlutá řepa");
        let done = w.task(seeds, "žlutá řepa");
        w.run(Command::CompleteTask { id: done });
        let deleted = w.task(seeds, "žlutá řepa");
        w.run(Command::Delete { id: deleted.into() });
        let mut model = ListSuggester::from_state(&w.state);
        // The deleted example must not qualify the class. Completion must not disqualify it.
        assert_eq!(model.suggest("zluta repa", &w.state), None);
        assert_eq!(model.suggest("blue wrench", &w.state), None);
        w.task(seeds, "ŽLUTÁ ŘEPA");
        model.observe("ŽLUTÁ ŘEPA", seeds);
        let expected = model.suggest("zluta repa", &w.state).unwrap();
        assert_eq!(expected.list, seeds);
        assert_eq!(
            ListSuggester::from_state(&w.state).suggest("Žlutá řepa", &w.state),
            Some(expected)
        );
        assert_eq!(model.suggest("blue wrench", &w.state).unwrap().list, tools);
        for title in ["", "?!", " r !", "ř"] {
            assert_eq!(model.suggest(title, &w.state), None);
        }
        w.run(Command::Delete { id: seeds.into() });
        assert_eq!(model.suggest("zluta repa", &w.state), None);
        assert_eq!(model.suggest("blue wrench", &w.state), None);
        w.state.lists.remove(&seeds);
        assert_eq!(model.suggest("zluta repa", &w.state), None);
    }

    #[test]
    fn short_or_unseen_input_abstains_even_with_a_dominant_list() {
        let mut w = World::new();
        let a = w.list("A");
        let b = w.list("B");
        let c = w.list("C");
        let mut model = suggest::ListSuggester::default();
        for i in 0..3 {
            model.observe("spare wrench", a);
            model.observe(&format!("zz{i}"), c);
        }
        for i in 0..200 {
            model.observe(
                &format!("weekly status report number {i} for project alpha"),
                b,
            );
        }
        assert_eq!(model.suggest("xq", &w.state), None);
        // Even a known short word ("zz") is insufficient. Unseen long words cannot win on priors.
        model.observe("zz", c);
        let permissive = model.with_threshold(0.);
        for title in ["xq", "a zz", "ΨΨΨ"] {
            assert_eq!(permissive.suggest(title, &w.state), None, "{title}");
        }
    }

    #[test]
    fn suggestion_threshold_is_inclusive_and_ties_are_stable() {
        use suggest::ListSuggester;
        let mut w = World::new();
        let a = w.list("A");
        let b = w.list("B");
        let mut model = ListSuggester::default();
        for _ in 0..3 {
            for id in [b, a] {
                model.observe("identical sample", id);
            }
        }
        assert_eq!(model.suggest("identical sample", &w.state), None);
        let s = model
            .with_threshold(0.5)
            .suggest("identical sample", &w.state)
            .unwrap();
        assert_eq!((s.list, s.confidence), (a.min(b), 0.5));
    }

    #[test]
    #[ignore = "synthetic training benchmark; run in release mode"]
    fn suggest_train_1000() {
        let mut w = World::new();
        let lists: Vec<_> = (0..8).map(|i| w.list(&format!("List {i}"))).collect();
        for i in 0..1000 {
            w.task(
                lists[i % lists.len()],
                &format!("Generate sample number {i}"),
            );
        }
        let start = std::time::Instant::now();
        std::hint::black_box(suggest::ListSuggester::from_state(&w.state));
        println!("Training on 1000 synthetic tasks: {:?}", start.elapsed());
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
        let (task, cmds) = add_task(&state, TaskRoom::MyDay, &parsed("Milk"), today(), || w.id());
        assert_eq!(cmds.len(), 2);
        for c in cmds {
            w.run(c);
        }
        let t = &w.state.tasks[&task.expect("task")];
        assert_eq!(t.my_day, Some(today()));
        assert_eq!(w.state.lists[&t.list].name, DEFAULT_LIST);
        assert!(w.state.lists[&t.list].color.is_some());
        // The next one reuses it.
        let state = w.state.clone();
        let (_, cmds) = add_task(&state, TaskRoom::Other, &parsed("Bread"), today(), || {
            w.id()
        });
        assert_eq!(cmds.len(), 1);
        // Empty titles add nothing.
        assert!(
            add_task(&w.state, TaskRoom::Other, &parsed("  "), today(), Uuid::nil)
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
            TaskRoom::List(errands),
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
            TaskRoom::Important,
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
            TaskRoom::Planned,
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
            TaskRoom::List(errands),
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
    fn quick_add_creates_a_list_in_an_empty_collection_and_reuses_it() {
        let mut w = World::new();
        let other = w.list("Inbox");
        let collection = CollectionId::from_uuid(w.id());
        w.run(Command::CreateCollection {
            id: collection,
            name: "Work".into(),
            color: None,
            placement: Placement::Last,
        });
        let before = w.state.clone();
        let ctx = EvalContext::utc("2026-11-21T10:35:00Z".parse().expect("timestamp"));
        let (id, cmds) = quick_add_task(
            &before,
            TaskRoom::Collection(collection),
            "Pay the invoice by fri !!",
            &ctx,
            || w.id(),
        );
        let [Command::CreateList { id: list, .. }, Command::CreateTask(_)] = cmds.as_slice() else {
            panic!("expected a list and task");
        };
        let list = *list;
        assert_eq!(cmds[0], create_list(list, Some(collection), DEFAULT_LIST));
        assert_ne!(list, other);
        for cmd in cmds {
            w.run(cmd);
        }
        assert_eq!(w.state.tasks[&id.expect("task")].list, list);
        assert_eq!(
            list_collection(&w.state, TaskRoom::List(list)),
            Some(collection)
        );
        let before = w.state.clone();
        let (_, commands) = add_task(
            &before,
            TaskRoom::Collection(collection),
            &parsed("File it"),
            today(),
            || w.id(),
        );
        assert!(matches!(commands.as_slice(), [Command::CreateTask(t)] if t.list == list));
    }

    #[test]
    fn toggles_and_edits_round_trip() {
        let mut w = World::new();
        let list = w.list("Errands");
        let state = w.state.clone();
        let (id, cmds) = add_task(
            &state,
            TaskRoom::List(list),
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
        // Each level is set as picked, and picking the level it already has changes nothing. (The fixed
        // clock gives a repeated command the same event, so this checks the commands.)
        let mut state = w.state.clone();
        for level in [
            Priority::Low,
            Priority::Normal,
            Priority::High,
            Priority::None,
        ] {
            let cmd = pick_priority(&state, id, level).expect("cmd");
            assert!(
                matches!(&cmd, Command::UpdateTask { patch, .. } if patch.priority == Some(level)),
                "{cmd:?}"
            );
            state.tasks.get_mut(&id).expect("task").priority = level;
            assert!(pick_priority(&state, id, level).is_none());
        }
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
        let (task, cmds) = add_task(&state, TaskRoom::List(b), &parsed("Trip"), today(), || {
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
        w.run(restore_step(s2id));
        assert!(w.state.is_live(s2id.into()));
        assert!(w.state.steps[&s2id].position < w.state.steps[&s1id].position);
        assert_eq!(w.state.steps[&s2id].title, "Go");
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

    #[test]
    fn a_task_moves_first_into_another_live_list_and_is_deleted_once() {
        let mut w = World::new();
        let errands = w.list("Errands");
        let home = w.list("Home");
        let add = |w: &mut World, title: &str, list| {
            let state = w.state.clone();
            let (id, cmds) = add_task(
                &state,
                TaskRoom::List(list),
                &parsed(title),
                today(),
                || w.id(),
            );
            for c in cmds {
                w.run(c);
            }
            id.expect("task")
        };
        let fix = add(&mut w, "Fix the gate", home);
        let stamps = add(&mut w, "Stamps", errands);
        // Already there: nothing to do.
        assert!(move_to_list(&w.state, stamps, errands).is_none());
        w.run(move_to_list(&w.state, stamps, home).expect("move"));
        assert_eq!(w.state.tasks[&stamps].list, home);
        let mut order: Vec<_> = w.state.tasks.values().filter(|t| t.list == home).collect();
        order.sort_by(|a, b| a.position.cmp(&b.position));
        assert_eq!(
            order.iter().map(|t| t.id).collect::<Vec<_>>(),
            [stamps, fix],
            "first, as a new task"
        );
        // Never into a deleted list, and a deleted task is neither moved nor deleted again.
        w.run(delete(errands));
        assert!(move_to_list(&w.state, stamps, errands).is_none());
        w.run(delete_task(&w.state, stamps).expect("delete"));
        assert!(w.state.tasks[&stamps].deleted.is_some());
        assert!(delete_task(&w.state, stamps).is_none());
        assert!(move_to_list(&w.state, stamps, home).is_none());
        assert!(delete_task(&w.state, TaskId::from_uuid(Uuid::nil())).is_none());
    }

    #[test]
    fn lists_are_grouped_as_the_sidebar_shows_them() {
        let mut w = World::new();
        let loose = w.list("Loose");
        let collection = |w: &mut World, name: &str| {
            let id = CollectionId::from_uuid(w.id());
            w.run(Command::CreateCollection {
                id,
                name: name.into(),
                color: None,
                placement: Placement::Last,
            });
            id
        };
        let work = collection(&mut w, "Work");
        // A collection without lists isn't offered.
        let _empty = collection(&mut w, "Empty");
        let home = collection(&mut w, "Home");
        let in_list = |w: &mut World, c, name: &str| {
            let id = ListId::from_uuid(w.id());
            w.run(create_list(id, Some(c), name));
            id
        };
        let office = in_list(&mut w, work, "Office");
        let travel = in_list(&mut w, work, "Travel");
        let garden = in_list(&mut w, home, "Garden");
        assert_eq!(
            list_groups(&w.state),
            [
                (Some(work), vec![office, travel]),
                (Some(home), vec![garden]),
                (None, vec![loose]),
            ]
        );
        // Deleted lists and the lists of a deleted collection are not offered.
        w.run(delete(travel));
        w.run(delete(home));
        assert_eq!(
            list_groups(&w.state),
            [(Some(work), vec![office]), (None, vec![loose])]
        );
    }
}
