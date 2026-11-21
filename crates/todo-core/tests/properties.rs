#![allow(clippy::unwrap_used)]
//! Property tests: replay, undo, redo, position keys and the query round trip.

mod common;

use common::World;
use proptest::prelude::*;
use todo_core::access::AccessLevel;
use todo_core::command::{Command, NewTask, Placement, TaskPatch};
use todo_core::error::{ApplyError, DomainError, UndoError};
use todo_core::event::{Event, EventPayload, FieldChange, FieldKey};
use todo_core::ids::{EntityId, EventId, TaskId};
use todo_core::model::{EmailLink, Entity, LinkKind, Priority};
use todo_core::ordering::{MAX_KEY_LEN, OrderError, PosKey};
use todo_core::query::*;
use todo_core::recurrence::{MAX_INTERVAL, Recurrence, Rule};
use todo_core::time::{Date, Weekday};
use todo_core::undo::{UndoOptions, undo_batch, undo_with};
use todo_core::{Actor, AgentId, State, apply};

// ---- history generation -------------------------------------------------------------------------

type Op = (u8, u16, u16, u8);

/// Number of op kinds [`build`] understands.
const OP_KINDS: u8 = 27;
/// The op kind that tries to create a task with the ID of an earlier task (which must fail).
const REUSE_OP: u8 = 26;

fn ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        (0u8..OP_KINDS, any::<u16>(), any::<u16>(), any::<u8>()),
        1..70,
    )
}

fn pick<T: Copy>(items: &[T], n: u16) -> Option<T> {
    if items.is_empty() {
        None
    } else {
        Some(items[usize::from(n) % items.len()])
    }
}

fn placement(w: &World, scope_ids: &[EntityId], n: u16, c: u8) -> Placement {
    match (c % 4, pick(scope_ids, n)) {
        (0, _) | (_, None) => Placement::Last,
        (1, _) => Placement::First,
        (2, Some(x)) => Placement::Before(x),
        (_, Some(x)) => {
            let _ = w;
            Placement::After(x)
        }
    }
}

/// Builds a command from a random op, reading targets from the current state.
fn build(w: &mut World, (kind, a, b, c): Op) -> Option<Command> {
    let s = &w.state;
    let live_tasks: Vec<_> = s
        .tasks
        .keys()
        .copied()
        .filter(|t| s.is_live((*t).into()))
        .collect();
    let live_lists: Vec<_> = s
        .lists
        .keys()
        .copied()
        .filter(|l| s.is_live((*l).into()))
        .collect();
    let live_cols: Vec<_> = s
        .collections
        .keys()
        .copied()
        .filter(|l| s.is_live((*l).into()))
        .collect();
    let live_tags: Vec<_> = s
        .tags
        .keys()
        .copied()
        .filter(|l| s.is_live((*l).into()))
        .collect();
    let live_steps: Vec<_> = s
        .steps
        .keys()
        .copied()
        .filter(|l| s.is_live((*l).into()))
        .collect();
    let task_scope = |s: &State, list| -> Vec<EntityId> {
        s.siblings(todo_core::state::Scope::Tasks(list))
            .into_iter()
            .map(|x| x.1)
            .collect()
    };
    let name = format!("n{}", c % 16);
    Some(match kind {
        0 => Command::CreateCollection {
            id: w.ids.collection(),
            name,
            color: (c % 2 == 0).then(|| "#abc".to_owned()),
            placement: Placement::Last,
        },
        1 => Command::CreateList {
            id: w.ids.list(),
            collection: if c % 3 == 0 {
                None
            } else {
                pick(&live_cols, a)
            },
            name,
            color: None,
            placement: if b % 2 == 0 {
                Placement::First
            } else {
                Placement::Last
            },
        },
        2..=4 => {
            let list = pick(&live_lists, a)?;
            let mut n = NewTask::new(w.ids.task(), list, format!("task {c}"));
            if c % 3 == 0 {
                n.due = Date::new(2026, 1 + (c % 12) as i8, 1 + (b % 28) as i8).ok();
            }
            if c % 7 == 0 && n.due.is_some() {
                n.recurrence = Some(Recurrence::new(Rule::Daily {
                    interval: 1 + u32::from(c % 3),
                }));
            }
            n.priority = [
                Priority::None,
                Priority::Low,
                Priority::Normal,
                Priority::High,
            ][usize::from(b % 4)];
            n.placement = placement(w, &task_scope(&w.state, list), b, c);
            Command::CreateTask(Box::new(n))
        }
        5 => Command::UpdateTask {
            id: pick(&live_tasks, a)?,
            patch: TaskPatch {
                title: (c % 2 == 0).then(|| format!("title {b}")),
                notes: (c % 3 == 0).then(|| format!("notes {b}")),
                due: (c % 5 == 0)
                    .then(|| (b % 2 == 0).then(|| Date::constant(2026, 6, 1 + (b % 28) as i8))),
                priority: (c % 4 == 0).then_some(Priority::Low),
                recurrence: None,
            },
        },
        6 => {
            let list = pick(&live_lists, b)?;
            Command::MoveTask {
                id: pick(&live_tasks, a)?,
                list,
                placement: placement(w, &task_scope(&w.state, list), a ^ b, c),
            }
        }
        7 => Command::CompleteTask {
            id: pick(&live_tasks, a)?,
        },
        8 => Command::UncompleteTask {
            id: pick(&live_tasks, a)?,
        },
        9 => Command::SetMyDay {
            id: pick(&live_tasks, a)?,
            date: (c % 2 == 0).then(|| Date::constant(2026, 6, 1 + (b % 28) as i8)),
        },
        10 => Command::CreateTag {
            id: w.ids.tag(),
            name: format!("tag{}", c % 6),
            color: None,
        },
        11 => Command::AddTag {
            task: pick(&live_tasks, a)?,
            tag: pick(&live_tags, b)?,
        },
        12 => Command::RemoveTag {
            task: pick(&live_tasks, a)?,
            tag: pick(&live_tags, b)?,
        },
        13 => Command::CreateStep {
            id: w.ids.step(),
            task: pick(&live_tasks, a)?,
            title: format!("step {c}"),
            placement: if b % 2 == 0 {
                Placement::First
            } else {
                Placement::Last
            },
        },
        14 => {
            if c % 2 == 0 {
                Command::CompleteStep {
                    id: pick(&live_steps, a)?,
                }
            } else {
                Command::UncompleteStep {
                    id: pick(&live_steps, a)?,
                }
            }
        }
        15 => {
            let all: Vec<EntityId> = live_tasks
                .iter()
                .map(|x| (*x).into())
                .chain(live_lists.iter().map(|x| (*x).into()))
                .chain(live_cols.iter().map(|x| (*x).into()))
                .chain(live_tags.iter().map(|x| (*x).into()))
                .chain(live_steps.iter().map(|x| (*x).into()))
                .chain(s.reminders.keys().map(|x| (*x).into()))
                .collect();
            Command::Delete { id: pick(&all, a)? }
        }
        16 => {
            let mut dead: Vec<EntityId> = Vec::new();
            dead.extend(
                s.tasks
                    .values()
                    .filter(|x| x.deleted.is_some())
                    .map(|x| EntityId::from(x.id)),
            );
            dead.extend(
                s.lists
                    .values()
                    .filter(|x| x.deleted.is_some())
                    .map(|x| EntityId::from(x.id)),
            );
            dead.extend(
                s.collections
                    .values()
                    .filter(|x| x.deleted.is_some())
                    .map(|x| EntityId::from(x.id)),
            );
            dead.extend(
                s.tags
                    .values()
                    .filter(|x| x.deleted.is_some())
                    .map(|x| EntityId::from(x.id)),
            );
            dead.extend(
                s.steps
                    .values()
                    .filter(|x| x.deleted.is_some())
                    .map(|x| EntityId::from(x.id)),
            );
            Command::Restore {
                id: pick(&dead, a)?,
            }
        }
        17 => {
            let all: Vec<EntityId> = live_tasks
                .iter()
                .map(|x| (*x).into())
                .chain(live_lists.iter().map(|x| (*x).into()))
                .chain(live_steps.iter().map(|x| (*x).into()))
                .collect();
            let id = pick(&all, a)?;
            let scope = s.scope_of(id)?;
            let sibs: Vec<EntityId> = s.siblings(scope).into_iter().map(|x| x.1).collect();
            Command::Reorder {
                id,
                placement: placement(w, &sibs, b, c),
            }
        }
        18 => Command::AddNote {
            task: pick(&live_tasks, a)?,
            text: format!("note {c}"),
        },
        19 => Command::MergeTasks {
            source: pick(&live_tasks, a)?,
            target: pick(&live_tasks, b)?,
        },
        20 => Command::AddLink {
            id: w.ids.link(),
            task: pick(&live_tasks, a)?,
            kind: if c % 2 == 0 {
                LinkKind::Web {
                    url: format!("https://example.com/{c}"),
                    title: None,
                }
            } else {
                LinkKind::Email(EmailLink {
                    subject: format!("s{c}"),
                    sender: "a@example.com".into(),
                    received_at: common::ts(i64::from(c)),
                    message_id: format!("<{c}@example.com>"),
                    web_link: None,
                })
            },
        },
        21 => Command::AddReminder {
            id: w.ids.reminder(),
            task: pick(&live_tasks, a)?,
            at: common::ts(10_000 + i64::from(b)),
        },
        22 => Command::UpdateList {
            id: pick(&live_lists, a)?,
            name: Some(name),
            color: Some(None),
        },
        23 => {
            let id = pick(&live_lists, a)?;
            Command::MoveList {
                id,
                collection: if c % 2 == 0 {
                    None
                } else {
                    pick(&live_cols, b)
                },
                placement: Placement::First,
            }
        }
        24 => Command::UpdateTag {
            id: pick(&live_tags, a)?,
            name: Some(format!("renamed{}", c % 6)),
            color: None,
        },
        REUSE_OP => {
            // Any task ever created, including withdrawn ones.
            let used: Vec<TaskId> = w
                .log
                .iter()
                .filter_map(|e| match &e.payload {
                    EventPayload::Created {
                        entity: Entity::Task(t),
                    } => Some(t.id),
                    _ => None,
                })
                .collect();
            let n = NewTask::new(pick(&used, a)?, pick(&live_lists, b)?, "reused");
            Command::CreateTask(Box::new(n))
        }
        _ => Command::AddAttachment {
            id: w.ids.attachment(),
            task: pick(&live_tasks, a)?,
            file_name: format!("f{c}.txt"),
            media_type: "text/plain".into(),
            size_bytes: u64::from(b),
            storage_key: format!("blob/{c}"),
        },
    })
}

/// Runs the ops as a mix of owner and a fully trusted agent; invalid commands are skipped.
fn run_history(ops: &[Op]) -> World {
    let mut w = World::new();
    w.grant_default("bot", AccessLevel::Plan);
    let bot = Actor::agent("bot").unwrap();
    let owner = w.owner.clone();
    // Seed so that most ops have something to act on.
    let col = w.collection("seed");
    let list = w.list(Some(col), "seed list");
    w.task(list, "seed task");
    for op in ops {
        if let Some(cmd) = build(&mut w, *op) {
            let actor = if op.3 % 5 == 0 { &bot } else { &owner };
            let _ = w.run_as(actor, cmd);
        }
    }
    w
}

/// `cases` by default; `PROPTEST_CASES` overrides it (for occasional long runs).
fn cases(cases: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cases);
    ProptestConfig {
        cases,
        ..ProptestConfig::default()
    }
}

fn cfg() -> ProptestConfig {
    cases(256)
}

proptest! {
    #![proptest_config(cfg())]

    /// (a) Replaying the log (also after a JSON round trip) on a fresh state gives an identical state.
    #[test]
    fn replay_rebuilds_identical_state(ops in ops()) {
        let w = run_history(&ops);
        let mut fresh = State::new();
        fresh.grants = w.state.grants.clone();
        for e in &w.log {
            apply(&mut fresh, e).unwrap();
        }
        prop_assert_eq!(&fresh, &w.state);

        let json = serde_json::to_string(&w.log).unwrap();
        let decoded: Vec<Event> = serde_json::from_str(&json).unwrap();
        let mut fresh2 = State::new();
        fresh2.grants = w.state.grants.clone();
        for e in &decoded {
            apply(&mut fresh2, e).unwrap();
        }
        prop_assert_eq!(&fresh2, &w.state);

        let state_json = serde_json::to_string(&w.state).unwrap();
        prop_assert_eq!(serde_json::from_str::<State>(&state_json).unwrap(), w.state);
    }

    /// (b) Undoing every event in reverse order returns to the initial state.
    #[test]
    fn undo_in_reverse_restores_initial_state(ops in ops()) {
        let mut w = run_history(&ops);
        let mut initial = State::new();
        initial.grants = w.state.grants.clone();
        let history: Vec<Event> = w.log.clone();
        for e in history.iter().rev() {
            let r = w.undo(e);
            prop_assert!(r.is_ok(), "undo of {:?} failed: {:?}", e.payload, r);
        }
        prop_assert!(w.state.data_eq(&initial), "state differs after undoing everything");
        // And everything can be redone: undoing the compensations in reverse restores the full history.
    }

    /// (c) Undo followed by redo (undo of the compensating event) restores the event's effect.
    #[test]
    fn undo_then_redo_restores_effect(ops in ops(), pick_idx in any::<usize>(), tail in 0usize..4) {
        let mut w = run_history(&ops);
        let history = w.log.clone();
        // Prefer recent events: they are the ones most likely to be conflict-free.
        let idx = history.len().saturating_sub(1 + pick_idx % (tail + 1));
        let target = &history[idx];
        let before = w.state.clone();
        match w.undo(target) {
            Ok(comp) => {
                let undone = w.state.clone();
                // Redo the compensation together with its rekey, if any.
                match w.undo_group(&comp) {
                    Ok(_) => prop_assert!(w.state.data_eq(&before)),
                    // A redo can legitimately be blocked when a referenced entity vanished meanwhile.
                    Err(UndoError::BrokenReference(_)) | Err(UndoError::HasDependents(_)) => {
                        prop_assert!(w.state.data_eq(&undone));
                    }
                    Err(e) => prop_assert!(false, "redo failed: {:?}", e),
                }
            }
            Err(UndoError::Conflict(_) | UndoError::BrokenReference(_) | UndoError::HasDependents(_) | UndoError::AlreadyUndone(_) | UndoError::Invariant { .. }) => {
                prop_assert!(w.state.data_eq(&before));
            }
            Err(e) => prop_assert!(false, "unexpected undo error: {:?}", e),
        }
    }

    /// A forced undo always applies and the owner can always redo it.
    #[test]
    fn forced_undo_applies(ops in ops(), pick_idx in any::<usize>()) {
        let mut w = run_history(&ops);
        let history = w.log.clone();
        let target = &history[pick_idx % history.len()];
        let owner = w.owner.clone();
        if let Ok(comp) = w.undo_as(&owner, target, true) {
            prop_assert_eq!(comp[0].undoes, Some(target.id));
            // At most a rekey of a restored entity follows, in the same group.
            prop_assert!(comp.len() <= 2);
            if let Some(rekey) = comp.get(1) {
                prop_assert_eq!(rekey.group, comp[0].id);
                let is_rekey = matches!(&rekey.payload, EventPayload::Updated { changes, .. }
                    if matches!(changes.as_slice(), [FieldChange::Position(_)]));
                prop_assert!(is_rekey, "unexpected second event {:?}", rekey);
            }
        }
    }
}

// ---- mixed edits, undos, forced undos and redos ---------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Step {
    /// Run a random command as the owner.
    Cmd(Op),
    /// Undo a recent ordinary event (optionally forced).
    Undo(u16, bool),
    /// Undo a recent compensating event, i.e. redo (optionally forced). With the last flag, undo its whole
    /// group (a compensation and its rekey) as a unit, as the server does.
    Redo(u16, bool, bool),
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    let op = (0u8..OP_KINDS, any::<u16>(), any::<u16>(), any::<u8>());
    prop::collection::vec(
        prop_oneof![
            3 => op.prop_map(Step::Cmd),
            3 => (any::<u16>(), prop::bool::weighted(0.35)).prop_map(|(n, f)| Step::Undo(n, f)),
            2 => (any::<u16>(), prop::bool::weighted(0.35), any::<bool>())
                .prop_map(|(n, f, g)| Step::Redo(n, f, g)),
        ],
        1..40,
    )
}

/// Recent events that are the newest link of their chain, newest first.
fn recent_heads(w: &World, compensating: bool) -> Vec<Event> {
    w.log
        .iter()
        .rev()
        .filter(|e| e.undoes.is_some() == compensating && w.state.journal.is_head(e.id))
        .take(10)
        .cloned()
        .collect()
}

/// Entity maps equal, ignoring the last-modified stamp of tasks. The stamp is bookkeeping: an undo leaves
/// it alone when a later event touched the task, so a chain of undos and redos cannot always restore it.
fn data_eq_ignoring_touch(a: &State, b: &State) -> bool {
    let strip = |s: &State| {
        let mut s = s.clone();
        for t in s.tasks.values_mut() {
            t.updated = t.created.clone();
        }
        s
    };
    strip(a).data_eq(&strip(b))
}

fn strip_touch(e: &Entity) -> Entity {
    let mut e = e.clone();
    if let Entity::Task(t) = &mut e {
        t.updated = t.created.clone();
    }
    e
}

/// The state right after `event` was applied, rebuilt from the log (independently of the journal).
fn state_after(w: &World, event: EventId) -> State {
    let mut s = State::new();
    s.grants = w.state.grants.clone();
    for e in &w.log {
        apply(&mut s, e).unwrap();
        if e.id == event {
            break;
        }
    }
    s
}

fn all_ids(s: &State) -> Vec<EntityId> {
    let mut v: Vec<EntityId> = Vec::new();
    v.extend(s.collections.keys().map(|x| EntityId::from(*x)));
    v.extend(s.lists.keys().map(|x| EntityId::from(*x)));
    v.extend(s.tasks.keys().map(|x| EntityId::from(*x)));
    v.extend(s.steps.keys().map(|x| EntityId::from(*x)));
    v.extend(s.tags.keys().map(|x| EntityId::from(*x)));
    v.extend(s.links.keys().map(|x| EntityId::from(*x)));
    v.extend(s.attachments.keys().map(|x| EntityId::from(*x)));
    v.extend(s.reminders.keys().map(|x| EntityId::from(*x)));
    v
}

/// The fields an update writes, by conflict key (the stamp excluded), sorted.
fn written_keys(changes: &[FieldChange]) -> Vec<FieldKey> {
    let mut keys: Vec<FieldKey> = changes.iter().filter_map(FieldChange::key).collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Independent oracle for a non-forced undo of `undone`, which produced `comp` and turned `before` into
/// `after`; `then` is the state right after `undone` was applied. Checked against the states, not the
/// journal: the undo inverts exactly the fields `undone` wrote, only overwrites values `undone` itself
/// left in place, never removes or hides anything that changed since, and keeps position keys unique.
fn assert_exact_undo(
    before: &State,
    after: &State,
    then: &State,
    undone: &Event,
    comp: &[Event],
) -> Result<(), TestCaseError> {
    let c = &comp[0];
    prop_assert_eq!(c.undoes, Some(undone.id));
    match (&c.payload, &undone.payload) {
        (
            EventPayload::Updated { target, changes },
            EventPayload::Updated {
                target: original_target,
                changes: original,
            },
        ) => {
            prop_assert_eq!(target, original_target);
            prop_assert_eq!(
                written_keys(changes),
                written_keys(original),
                "the undo must invert exactly the fields the event wrote"
            );
            for ch in changes {
                match ch {
                    FieldChange::Touch(_) => {
                        let EntityId::Task(t) = target else {
                            return Err(TestCaseError::fail("stamp on a non-task"));
                        };
                        prop_assert!(original.iter().any(|o| matches!(o, FieldChange::Touch(_))));
                        prop_assert_eq!(
                            &before.tasks[t].updated,
                            &then.tasks[t].updated,
                            "the stamp was rolled back although the task was touched since"
                        );
                    }
                    FieldChange::TagAdded { tag } | FieldChange::TagRemoved { tag } => {
                        let EntityId::Task(t) = target else {
                            return Err(TestCaseError::fail("tag change on a non-task"));
                        };
                        let set_by_undone = original
                            .iter()
                            .find(|o| o.key() == ch.key())
                            .map(|o| matches!(o, FieldChange::TagAdded { .. }));
                        prop_assert_eq!(
                            Some(before.tasks[t].tags.contains(tag)),
                            set_by_undone,
                            "tag membership changed since the undone event"
                        );
                    }
                    _ => {
                        let expected = original
                            .iter()
                            .find(|o| o.key() == ch.key())
                            .map(FieldChange::inverted);
                        prop_assert_eq!(
                            Some(ch.clone()),
                            expected,
                            "a later event changed the field the undo overwrote"
                        );
                    }
                }
            }
        }
        (EventPayload::Withdrawn { entity }, EventPayload::Created { entity: created }) => {
            prop_assert_eq!(entity.id(), created.id());
            prop_assert_eq!(
                Some(strip_touch(entity)),
                then.entity(entity.id()).as_ref().map(strip_touch),
                "withdrawal would lose later changes"
            );
        }
        (EventPayload::Created { entity }, EventPayload::Withdrawn { entity: snapshot }) => {
            prop_assert!(!before.exists(entity.id()));
            prop_assert_eq!(entity, snapshot);
        }
        // The tombstone in place must still be the one the undone event left.
        (
            EventPayload::Deleted { target, .. } | EventPayload::Restored { target, .. },
            EventPayload::Deleted {
                target: t0,
                tombstone,
            },
        ) => {
            prop_assert_eq!(target, t0);
            prop_assert_eq!(before.tombstone(*target), Some(tombstone.clone()));
        }
        (
            EventPayload::Deleted { target, .. } | EventPayload::Restored { target, .. },
            EventPayload::Restored { target: t0, .. },
        ) => {
            prop_assert_eq!(target, t0);
            prop_assert_eq!(before.tombstone(*target), None);
        }
        (c, u) => {
            return Err(TestCaseError::fail(format!(
                "unexpected inverse {c:?} of {u:?}"
            )));
        }
    }
    // Visibility, modelled from the states alone: whatever the undo removes or hides must be exactly as it
    // was right after the undone event.
    if matches!(
        c.payload,
        EventPayload::Deleted { .. } | EventPayload::Withdrawn { .. }
    ) {
        for id in all_ids(before) {
            if before.is_live(id) && !after.is_live(id) {
                prop_assert_eq!(
                    then.entity(id).as_ref().map(strip_touch),
                    before.entity(id).as_ref().map(strip_touch),
                    "{:?} changed since the undone event, and the undo hides it",
                    id
                );
            }
        }
    }
    assert_keys(before, after, then, comp)
}

/// True if, in `before`, a live sibling other than `target` held `key` in the scope `target` has in
/// `after`.
fn key_taken(before: &State, after: &State, target: EntityId, key: &PosKey) -> bool {
    after
        .entity(target)
        .and_then(|e| State::scope_of_entity(&e))
        .is_some_and(|scope| {
            before
                .siblings(scope)
                .iter()
                .any(|(k, id)| k == key && *id != target)
        })
}

/// A compensation that puts an entity among siblings (restore, redo of a creation, undo of a move) leaves
/// no new tie: any sibling sharing the entity's key already shared it right after the undone event. The
/// fresh key comes in a separate rekey event, only if the key was taken.
fn assert_keys(
    before: &State,
    after: &State,
    then: &State,
    comp: &[Event],
) -> Result<(), TestCaseError> {
    let c = &comp[0];
    let target = c.payload.target();
    prop_assert!(comp.len() <= 2);
    let places = match &c.payload {
        EventPayload::Restored { .. } | EventPayload::Created { .. } => true,
        EventPayload::Updated { changes, .. } => changes.iter().any(|ch| {
            matches!(
                ch,
                FieldChange::Position(_)
                    | FieldChange::TaskList(_)
                    | FieldChange::ListCollection(_)
                    | FieldChange::ParentTask(_)
            )
        }),
        _ => false,
    };
    let placed = after
        .entity(target)
        .filter(|e| places && e.deleted().is_none())
        .and_then(|e| Some((State::scope_of_entity(&e)?, e.position()?.clone())));
    let Some((scope, key)) = placed else {
        prop_assert_eq!(comp.len(), 1);
        return Ok(());
    };
    let held_then = then.siblings(scope);
    for (_, other) in after
        .siblings(scope)
        .into_iter()
        .filter(|(k, id)| *k == key && *id != target)
    {
        prop_assert!(
            held_then.iter().any(|(k, id)| *id == other && *k == key),
            "{:?} took the key of {:?} since the undone event, but it was not re-keyed",
            other,
            target
        );
    }
    if let Some(rekey) = comp.get(1) {
        prop_assert_eq!(rekey.group, c.id);
        prop_assert_eq!(rekey.undoes, None);
        let EventPayload::Updated { target: t, changes } = &rekey.payload else {
            return Err(TestCaseError::fail("the second event must re-key"));
        };
        prop_assert_eq!(*t, target);
        let [FieldChange::Position(p)] = changes.as_slice() else {
            return Err(TestCaseError::fail(format!(
                "unexpected re-key {changes:?}"
            )));
        };
        prop_assert_eq!(&p.new, &key);
        prop_assert!(
            key_taken(before, after, target, &p.old),
            "re-keyed although the key was free"
        );
    }
    Ok(())
}

/// For a command that reuses an ID, the ID.
fn reused_id(op: Op, cmd: &Command) -> Option<EntityId> {
    match cmd {
        Command::CreateTask(n) if op.0 == REUSE_OP => Some(n.id.into()),
        _ => None,
    }
}

proptest! {
    #![proptest_config(cases(192))]

    /// Random mixes of edits, normal and forced undos and redos, and attempts to reuse IDs:
    ///
    /// - a non-forced undo inverts exactly the fields the event wrote, never overwrites a value that a
    ///   later event changed, and never removes or hides anything changed since (checked against the
    ///   states, independently of the journal);
    /// - undoing a compensation right away restores exactly the state before it;
    /// - IDs are never reused, by `decide` or by `apply`;
    /// - the full log, compensations included, survives a JSON round trip, replays to the same state and
    ///   journal, and the replayed state makes the same undo decisions;
    /// - rolling every step back in reverse order (undoing each compensation and edit) restores each
    ///   earlier state (up to the last-modified stamp), without conflicts unless a forced undo was
    ///   involved.
    #[test]
    fn mixed_undo_redo_sequences_are_exact_and_never_silently_overwrite(
        ops in prop::collection::vec((0u8..OP_KINDS, any::<u16>(), any::<u16>(), any::<u8>()), 1..30),
        steps in steps(),
    ) {
        let mut w = run_history(&ops);
        let owner = w.owner.clone();
        let mut done: Vec<(Vec<Event>, State)> = Vec::new();
        let mut any_forced = false;
        for step in steps {
            let before = w.state.clone();
            match step {
                Step::Cmd(op) => {
                    let Some(cmd) = build(&mut w, op) else { continue };
                    let reused = reused_id(op, &cmd);
                    let r = w.run_as(&owner, cmd);
                    if let Some(id) = reused {
                        prop_assert_eq!(r, Err(DomainError::AlreadyExists(id)));
                        prop_assert_eq!(&w.state, &before);
                        // A forged creation event with the old ID is refused by apply too.
                        let mut forged = w
                            .log
                            .iter()
                            .find(|e| e.undoes.is_none() && matches!(&e.payload, EventPayload::Created { entity } if entity.id() == id))
                            .cloned()
                            .unwrap();
                        forged.id = EventId::derive(common::ts(3_000_000), b"forged");
                        forged.group = forged.id;
                        prop_assert_eq!(apply(&mut w.state.clone(), &forged), Err(ApplyError::AlreadyExists(id)));
                    } else if let Ok(events) = r && !events.is_empty() {
                        done.push((events, before));
                    }
                }
                Step::Undo(n, force) | Step::Redo(n, force, _) => {
                    let pool = recent_heads(&w, matches!(step, Step::Redo(..)));
                    if pool.is_empty() {
                        continue;
                    }
                    let target = pool[usize::from(n) % pool.len()].clone();
                    let group: Vec<Event> = w.log.iter().filter(|e| e.group == target.group).cloned().collect();
                    let as_group = matches!(step, Step::Redo(_, _, true))
                        && group.len() > 1
                        && group.iter().all(|e| w.state.journal.is_head(e.id));
                    let then = state_after(&w, target.id);
                    let result = if as_group {
                        w.undo_group_as(&owner, &group, force)
                    } else {
                        w.undo_as(&owner, &target, force)
                    };
                    match result {
                        Ok(comp) => {
                            prop_assert!(!comp.is_empty());
                            let forced = comp.iter().any(|c| c.forced);
                            prop_assert!(force || !forced);
                            if !as_group && !forced {
                                assert_exact_undo(&before, &w.state, &then, &target, &comp)?;
                            }
                            // Undoing the compensation (with its rekey) right away restores the state before.
                            let mut probe = w.state.clone();
                            match undo_batch(&probe, &comp, &owner, common::ts(1_000_000), UndoOptions::default()) {
                                Ok(back) => {
                                    for e in &back {
                                        apply(&mut probe, e).unwrap();
                                    }
                                    prop_assert!(probe.data_eq(&before), "undoing {:?} is not exact", comp);
                                }
                                Err(UndoError::HasDependents(_)) => {}
                                Err(e) => prop_assert!(false, "undoing a fresh compensation failed: {:?}", e),
                            }
                            any_forced |= forced;
                            done.push((comp, before));
                        }
                        Err(e) => {
                            prop_assert!(
                                matches!(
                                    e,
                                    UndoError::Conflict(_)
                                        | UndoError::BrokenReference(_)
                                        | UndoError::HasDependents(_)
                                        | UndoError::Invariant { .. }
                                ),
                                "unexpected undo error: {:?}",
                                e
                            );
                            prop_assert!(!(force && matches!(e, UndoError::Conflict(_))));
                            prop_assert_eq!(&w.state, &before);
                        }
                    }
                }
            }
        }

        // The full log survives a JSON round trip and replays to the same state, journal included, which
        // then makes the same undo decisions as the original.
        let json = serde_json::to_string(&w.log).unwrap();
        let decoded: Vec<Event> = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&decoded, &w.log);
        let mut replayed = State::new();
        replayed.grants = w.state.grants.clone();
        for e in &decoded {
            apply(&mut replayed, e).unwrap();
        }
        prop_assert_eq!(&replayed, &w.state);
        let state_json = serde_json::to_string(&w.state).unwrap();
        prop_assert_eq!(&serde_json::from_str::<State>(&state_json).unwrap(), &w.state);
        let later = common::ts(2_000_000);
        for e in decoded.iter().rev().take(40) {
            for force in [false, true] {
                let options = UndoOptions { force };
                prop_assert_eq!(
                    undo_with(&replayed, e, &owner, later, options),
                    undo_with(&w.state, e, &owner, later, options)
                );
            }
        }

        // Roll everything back, newest step first. Rolling back an undo step redoes, which makes a chain's
        // newest link younger than the later steps' rollbacks; undoing such a link may then conflict
        // with those (an entity it would hide again changed after it), so only those may be forced.
        let rollback_start = w.log.len();
        for (events, before) in done.iter().rev() {
            for e in events.iter().rev() {
                let journal = &w.state.journal;
                let head = journal.head_of(journal.root_of(e.id));
                let head = w.log.iter().find(|x| x.id == head).cloned().unwrap();
                let made_by_rollback = w.log[rollback_start..].iter().any(|x| x.id == head.id);
                // A step's rekey is rolled back right before its compensation, which is what `undo_batch`
                // does; taken back on its own it reports a position conflict, so it is forced here.
                let rekey_of_this_step = head.undoes.is_none()
                    && head.group != head.id
                    && events.iter().any(|x| x.id == head.group);
                let comp = match w.undo_as(&owner, &head, false) {
                    Ok(comp) => comp,
                    Err(UndoError::Conflict(_)) if any_forced || made_by_rollback || rekey_of_this_step => {
                        let r = w.undo_as(&owner, &head, true);
                        prop_assert!(r.is_ok(), "forced rollback of {:?} failed: {:?}", head.payload, r);
                        r.unwrap()
                    }
                    Err(err) => {
                        prop_assert!(false, "rollback of {:?} failed: {:?}", head.payload, err);
                        unreachable!()
                    }
                };
                // For the same reason such a link may find its key taken by a sibling the rollback put
                // back; the rollback takes that rekey back too, as the state it returns to had the tie. On
                // its own that is a position conflict, so it must be forced.
                if let Some(rekey) = comp.get(1) {
                    prop_assert!(any_forced || made_by_rollback, "rollback of {:?} re-keyed", head.payload);
                    let unforced = w.undo_as(&owner, rekey, false);
                    prop_assert!(matches!(&unforced, Err(UndoError::Conflict(_))), "unforced rekey undo: {:?}", unforced);
                    let r = w.undo_as(&owner, rekey, true);
                    prop_assert!(matches!(&r, Ok(c) if c.len() == 1), "undoing the rekey failed: {:?}", r);
                }
            }
            prop_assert!(data_eq_ignoring_touch(&w.state, before), "rollback is not exact");
        }
    }

    /// Undo only accepts the exact applied event: any change except `seq` is rejected.
    #[test]
    fn undo_rejects_any_altered_event(ops in ops(), pick_idx in any::<usize>(), how in 0u8..5) {
        let w = run_history(&ops);
        let original = w.log[pick_idx % w.log.len()].clone();
        let mut e = original.clone();
        match how {
            0 => e.actor = Actor::agent("mallory").unwrap(),
            1 => e.forced = !e.forced,
            2 => e.at = common::ts(-5),
            3 => e.group = todo_core::ids::EventId::from_uuid(uuid_nil()),
            _ => {
                e.seq = Some(7);
                let r = todo_core::undo::undo(&w.state, &e, &w.owner, common::ts(1_000_000));
                prop_assert!(!matches!(r, Err(UndoError::EventMismatch(_)) | Err(UndoError::UnknownEvent(_))));
                return Ok(());
            }
        }
        let r = todo_core::undo::undo(&w.state, &e, &w.owner, common::ts(1_000_000));
        prop_assert_eq!(r, Err(UndoError::EventMismatch(original.id)));
    }
}

fn uuid_nil() -> uuid::Uuid {
    uuid::Uuid::nil()
}

// ---- (d) position keys ----------------------------------------------------------------------------

fn key() -> impl Strategy<Value = PosKey> {
    "[0-9A-Za-z]{0,6}[1-9A-Za-z]".prop_map(|s| PosKey::new(s).unwrap())
}

proptest! {
    #[test]
    fn between_strictly_orders(a in key(), b in key()) {
        prop_assume!(a != b);
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let mid = PosKey::between(Some(&lo), Some(&hi)).unwrap();
        prop_assert!(lo < mid && mid < hi, "{lo} < {mid} < {hi}");
        prop_assert!(PosKey::new(mid.as_str()).is_ok());
        let after = lo.after().unwrap();
        let before = hi.before().unwrap();
        prop_assert!(after > lo && before < hi);
        prop_assert!(PosKey::new(after.as_str()).is_ok() && PosKey::new(before.as_str()).is_ok());
    }

    #[test]
    fn random_insertions_keep_order_and_stay_short(inserts in prop::collection::vec((any::<u16>(), 0u8..3), 1..400)) {
        let mut keys: Vec<PosKey> = Vec::new();
        for (n, how) in inserts {
            let i = usize::from(n) % (keys.len() + 1);
            let key = match how {
                0 => PosKey::between(i.checked_sub(1).map(|j| &keys[j]), keys.get(i)).unwrap(),
                1 => { keys.last().map_or_else(PosKey::first, |k| k.after().unwrap()) }
                _ => { keys.first().map_or_else(PosKey::first, |k| k.before().unwrap()) }
            };
            let at = if how == 1 { keys.len() } else if how == 2 { 0 } else { i };
            keys.insert(at, key);
        }
        prop_assert!(keys.windows(2).all(|w| w[0] < w[1]));
        prop_assert!(keys.iter().all(|k| k.as_str().len() <= 40), "longest {}", keys.iter().map(|k| k.as_str().len()).max().unwrap());
    }
}

/// Keys with long runs of the extreme digits, up to the length limit.
fn adversarial_key() -> impl Strategy<Value = PosKey> {
    (
        prop::sample::select(vec!['0', 'z', '1', 'y']),
        0..=MAX_KEY_LEN - 1,
        "[1-9A-Za-z]",
    )
        .prop_map(|(c, n, last)| PosKey::new(format!("{}{last}", c.to_string().repeat(n))).unwrap())
}

proptest! {
    /// Generators never panic or overflow on adversarial keys: they return a valid key within the limit
    /// in the right order, or `TooLong`.
    #[test]
    fn generators_handle_adversarial_keys(a in adversarial_key(), b in adversarial_key()) {
        let ok = |r: Result<PosKey, OrderError>| -> Result<Option<PosKey>, TestCaseError> {
            match r {
                Ok(k) => {
                    prop_assert!(k.as_str().len() <= MAX_KEY_LEN);
                    prop_assert_eq!(PosKey::new(k.as_str()), Ok(k.clone()));
                    Ok(Some(k))
                }
                Err(OrderError::TooLong { len }) => {
                    prop_assert!(len > MAX_KEY_LEN);
                    Ok(None)
                }
                Err(e) => Err(TestCaseError::fail(format!("unexpected {e:?}"))),
            }
        };
        if let Some(k) = ok(a.after())? {
            prop_assert!(k > a);
        }
        if let Some(k) = ok(a.before())? {
            prop_assert!(k < a);
        }
        if a != b {
            let (lo, hi) = if a < b { (&a, &b) } else { (&b, &a) };
            if let Some(k) = ok(PosKey::between(Some(lo), Some(hi)))? {
                prop_assert!(*lo < k && k < *hi);
            }
        }
    }

    /// Recurrence rules survive an RRULE round trip, and the parser never panics.
    #[test]
    fn rrule_round_trips(rule in arb_rule(), until in prop::option::of((2000i16..2100, 1i8..=12, 1i8..=28))) {
        let r = Recurrence {
            rule,
            until: until.map(|(y, m, d)| Date::constant(y, m, d)),
        };
        prop_assert_eq!(Recurrence::from_rrule(&r.to_rrule()), Ok(r));
    }

    #[test]
    fn rrule_parser_never_panics(s in "(RRULE:)?([A-Z]{2,10}=[A-Z0-9,]{0,8};?){0,5}|\\PC{0,30}") {
        let _ = Recurrence::from_rrule(&s);
    }
}

fn arb_rule() -> impl Strategy<Value = Rule> {
    let interval = 1u32..=MAX_INTERVAL;
    prop_oneof![
        interval
            .clone()
            .prop_map(|interval| Rule::Daily { interval }),
        (
            interval.clone(),
            prop::collection::btree_set(prop::sample::select(Weekday::ALL.to_vec()), 1..=7)
        )
            .prop_map(|(interval, days)| Rule::Weekly { interval, days }),
        (interval.clone(), 1u8..=31).prop_map(|(interval, day)| Rule::Monthly { interval, day }),
        interval.prop_map(|interval| Rule::Yearly { interval }),
    ]
}

// ---- (e) query round trip --------------------------------------------------------------------------

fn arb_text(min: usize) -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[a-zA-Z0-9_-]{1,8}",
        2 => prop::collection::vec(
            prop::sample::select(vec!['a', 'é', 'Z', ' ', '(', ')', '"', '\\', ':', '<', '>', '=', '!', '-', '+', '1', 'ž']),
            min..8
        ).prop_map(|v| v.into_iter().collect::<String>()),
        1 => prop::sample::select(vec!["and", "OR", "Not", "done", "my_day", "sort:due", "due<=today", "-x", "mléko"]).prop_map(str::to_owned),
    ]
    .prop_filter("non-empty", move |s| s.len() >= min)
}

fn arb_date_expr() -> impl Strategy<Value = DateExpr> {
    prop_oneof![
        Just(DateExpr::Today),
        Just(DateExpr::Tomorrow),
        Just(DateExpr::Yesterday),
        (-400i32..400).prop_map(DateExpr::Offset),
        (1990i16..2100, 1i8..=12, 1i8..=28)
            .prop_map(|(y, m, d)| DateExpr::Absolute(Date::constant(y, m, d))),
    ]
}

fn arb_op() -> impl Strategy<Value = CmpOp> {
    prop::sample::select(vec![
        CmpOp::Eq,
        CmpOp::Ne,
        CmpOp::Lt,
        CmpOp::Le,
        CmpOp::Gt,
        CmpOp::Ge,
    ])
}

fn arb_actor_spec() -> impl Strategy<Value = ActorSpec> {
    prop_oneof![
        Just(ActorSpec::Owner),
        Just(ActorSpec::AnyAgent),
        Just(ActorSpec::Importer),
        Just(ActorSpec::EmailIngest),
        Just(ActorSpec::System),
        prop::sample::select(vec!["codex", "claude-2", "a_b"])
            .prop_map(|s| ActorSpec::Agent(AgentId::new(s).unwrap())),
    ]
}

fn arb_pred() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        arb_text(0).prop_map(Predicate::List),
        arb_text(0).prop_map(Predicate::Collection),
        arb_text(0).prop_map(Predicate::Tag),
        arb_text(1).prop_map(Predicate::Text),
        prop::sample::select(vec![DueState::Overdue, DueState::None, DueState::Any])
            .prop_map(Predicate::DueState),
        (
            prop::sample::select(vec![
                DateField::Due,
                DateField::Created,
                DateField::Updated,
                DateField::Completed
            ]),
            arb_op(),
            arb_date_expr()
        )
            .prop_map(|(field, op, value)| Predicate::Date { field, op, value }),
        Just(Predicate::MyDay),
        Just(Predicate::Done),
        (
            arb_op(),
            prop::sample::select(vec![
                Priority::None,
                Priority::Low,
                Priority::Normal,
                Priority::High
            ])
        )
            .prop_map(|(op, level)| Predicate::Priority { op, level }),
        arb_actor_spec().prop_map(Predicate::CreatedBy),
        arb_actor_spec().prop_map(Predicate::UpdatedBy),
    ]
}

fn arb_expr() -> impl Strategy<Value = Expr> {
    arb_pred()
        .prop_map(Expr::Pred)
        .prop_recursive(4, 32, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 2..4).prop_map(Expr::And),
                prop::collection::vec(inner.clone(), 2..4).prop_map(Expr::Or),
                inner.prop_map(|e| Expr::Not(Box::new(e))),
            ]
        })
}

fn arb_query() -> impl Strategy<Value = Query> {
    let sorts = prop::collection::vec(
        (
            prop::sample::select(vec![
                SortKey::Due,
                SortKey::Priority,
                SortKey::Created,
                SortKey::Updated,
                SortKey::Completed,
                SortKey::Title,
                SortKey::Manual,
            ]),
            any::<bool>(),
        )
            .prop_map(|(key, descending)| Sort { key, descending }),
        0..3,
    );
    (prop_oneof![1 => Just(Expr::True), 8 => arb_expr()], sorts)
        .prop_map(|(expr, sort)| Query { expr, sort })
}

proptest! {
    #![proptest_config(cases(512))]

    #[test]
    fn query_round_trips(q in arb_query()) {
        let text = q.to_string();
        let parsed = Query::parse(&text);
        prop_assert!(parsed.is_ok(), "`{}` failed: {:?}", text, parsed);
        prop_assert_eq!(parsed.unwrap(), q, "text was `{}`", text);
    }

    #[test]
    fn parser_never_panics(s in "\\PC{0,40}") {
        let _ = Query::parse(&s);
    }
}

/// Guards against the generators silently producing trivial histories.
#[test]
fn histories_exercise_many_event_kinds() {
    use todo_core::event::{EventPayload, FieldChange};
    let mut x: u64 = 0x1234_5678_9abc_def1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let ops: Vec<Op> = (0..600)
        .map(|_| {
            let v = next();
            (
                (v % 26) as u8,
                (v >> 8) as u16,
                (v >> 24) as u16,
                (v >> 40) as u8,
            )
        })
        .collect();
    let w = run_history(&ops);
    assert!(w.log.len() > 200, "only {} events", w.log.len());
    let count = |f: &dyn Fn(&EventPayload) -> bool| w.log.iter().filter(|e| f(&e.payload)).count();
    assert!(count(&|p| matches!(p, EventPayload::Created { .. })) > 30);
    assert!(count(&|p| matches!(p, EventPayload::Deleted { .. })) > 3);
    assert!(count(&|p| matches!(p, EventPayload::Restored { .. })) > 0);
    assert!(
        count(
            &|p| matches!(p, EventPayload::Updated { changes, .. } if changes.iter().any(|c| matches!(c, FieldChange::Position(_))))
        ) > 5
    );
    assert!(
        count(
            &|p| matches!(p, EventPayload::Updated { changes, .. } if changes.iter().any(|c| matches!(c, FieldChange::ParentTask(_))))
        ) > 0,
        "no merges happened"
    );
    assert!(w.log.iter().any(|e| e.actor.is_agent()));
}
