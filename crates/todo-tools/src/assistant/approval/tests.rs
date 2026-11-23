#![allow(clippy::unwrap_used)]

use super::*;
use proptest::prelude::*;
use serde_json::{Value, json};
use uuid::Uuid;

const NOW: Timestamp = Timestamp::UNIX_EPOCH;
const LIST: ListId = ListId::from_uuid(Uuid::from_u128(2));
const TASK: TaskId = TaskId::from_uuid(Uuid::from_u128(3));
const NEW: TaskId = TaskId::from_uuid(Uuid::from_u128(4));
const REVISION: Revision = Revision {
    epoch: Uuid::from_u128(5),
    cursor: 20,
};
const ID: Uuid = Uuid::from_u128(6);

fn command(value: Value) -> Command {
    serde_json::from_value(value).unwrap()
}

fn base() -> State {
    let mut base = State::new();
    for command in [
        command(json!({"cmd":"create_list","id":LIST,"name":"Inbox"})),
        command(
            json!({"cmd":"create_task","id":TASK,"list":LIST,"title":"Original","start_date":"2030-03-05","due":"2030-03-10"}),
        ),
    ] {
        for event in todo_core::decide(&base, &Actor::System, &command, NOW).unwrap() {
            apply(&mut base, &event).unwrap();
        }
    }
    base
}

fn new_batch(base: &State, at: Timestamp) -> Batch {
    Batch::new(ID, REVISION, 7, base, BTreeSet::from([LIST]), at).unwrap()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DisplayTask {
    id: TaskId,
    list: ListId,
    title: String,
    due: Option<Date>,
    priority: Priority,
    status: WorkflowStatus,
    start_date: Option<Date>,
    done: bool,
}

fn displayed(state: &State) -> Vec<DisplayTask> {
    let mut tasks: Vec<_> = state.tasks.values().collect();
    tasks.sort_by_key(|task| (&task.position, task.id));
    tasks
        .into_iter()
        .map(|task| DisplayTask {
            id: task.id,
            list: task.list,
            title: task.title.clone(),
            due: task.due,
            priority: task.priority,
            status: task.status,
            start_date: task.start_date,
            done: task.is_done(),
        })
        .collect()
}

fn apply_effect(view: &mut Vec<DisplayTask>, effect: &Effect) -> Result<(), TestCaseError> {
    match effect {
        Effect::Created {
            task,
            priority,
            status,
            placement,
        } => {
            prop_assert_eq!(*placement, Placement::Last);
            view.push(DisplayTask {
                id: task.id,
                list: task.list.id,
                title: task.title.clone(),
                due: task.due,
                priority: *priority,
                status: *status,
                start_date: None,
                done: false,
            });
        }
        Effect::Edited { task, changes } => {
            let current = view.iter_mut().find(|row| row.id == task.id).unwrap();
            prop_assert_eq!(current.list, task.list.id);
            prop_assert_eq!(&current.title, &task.title);
            prop_assert_eq!(current.due, task.due);
            fn change<T: Clone + std::fmt::Debug + PartialEq>(
                current: &mut T,
                edit: &Change<T>,
            ) -> Result<(), TestCaseError> {
                prop_assert_eq!(&*current, &edit.old);
                *current = edit.new.clone();
                Ok(())
            }
            for edit in changes {
                match edit {
                    Edit::Title(edit) => change(&mut current.title, edit)?,
                    Edit::Due(edit) => change(&mut current.due, edit)?,
                    Edit::StartDate(edit) => change(&mut current.start_date, edit)?,
                    Edit::Priority(edit) => change(&mut current.priority, edit)?,
                    Edit::Completed(edit) => change(&mut current.done, edit)?,
                }
            }
        }
        Effect::Deleted { task } => view.retain(|row| row.id != task.id),
    }
    Ok(())
}

proptest! {
    #[test]
    fn frozen_batch_replays_and_preview_explains_every_intermediate_effect(
        edits in prop::collection::vec((any::<bool>(), 0u8..6, 1u8..28, "[a-z]{1,10}"), 0..12),
        at_ms in 1i64..2_000_000,
    ) {
        let base = base();
        let mut batch = new_batch(&base, NOW);
        let mut visible = displayed(&base);
        let mut commands = vec![command(json!({"cmd":"create_task","id":NEW,"list":LIST,"title":"New","priority":"high"}))];
        for (existing, kind, day, title) in edits {
            let id = if existing { TASK } else { NEW };
            commands.push(command(match kind {
                0 => json!({"cmd":"complete_task","id":id}),
                1 => json!({"cmd":"uncomplete_task","id":id}),
                2 => json!({"cmd":"update_task","id":id,"patch":{"title":title}}),
                3 => json!({"cmd":"update_task","id":id,"patch":{"due":format!("2030-03-{day:02}")}}),
                4 => json!({"cmd":"update_task","id":id,"patch":{"due":null}}),
                _ => json!({"cmd":"update_task","id":id,"patch":{"priority":"low"}}),
            }));
        }
        for (index, command) in commands.into_iter().enumerate() {
            if batch.push(command, index).unwrap() {
                for effect in &batch.items.last().unwrap().effects { apply_effect(&mut visible, effect)?; }
            }
            prop_assert_eq!(&visible, &displayed(&batch.scratch));
        }
        let simulated = batch.scratch.clone();
        let pending = batch.freeze().unwrap().unwrap();
        let at = Timestamp::from_millisecond(at_ms).unwrap();
        let req = ApprovalRequest { command_id: pending.preview().id, batch: pending.approval().unwrap() };
        let req: ApprovalRequest = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        let all_events = req.preflight().unwrap().redecide(&base).unwrap();
        let mut retimed = new_batch(&base, at);
        let mut ids = BTreeSet::new();
        for event in &all_events {
            prop_assert!(ids.insert(event.id));
            prop_assert_eq!(event.group, all_events[0].id);
        }
        for (command, item) in pending.commands().iter().zip(&pending.preview().items) {
            prop_assert!(retimed.push(command.clone(), item.index).unwrap());
        }
        let mut rebuilt = new_batch(&base, NOW).scratch;
        for event in &all_events { apply(&mut rebuilt, event).unwrap(); }
        // Event fingerprints include the server's final group ID, unlike the scratch journal.
        prop_assert!(rebuilt.data_eq(&simulated));
        let undo = todo_core::undo::undo_batch(&rebuilt, &all_events, &Actor::owner("test").unwrap(), at, todo_core::UndoOptions::default()).unwrap();
        for event in &undo { apply(&mut rebuilt, event).unwrap(); }
        prop_assert_eq!(displayed(&rebuilt), displayed(&base));
        let replayed = retimed.freeze().unwrap().unwrap();
        // The effect review is the same, but the digest binds the decision timestamp.
        prop_assert_eq!(&pending.preview().items, &replayed.preview().items);
        prop_assert_ne!(pending.preview().digest, replayed.preview().digest);
        prop_assert_eq!(displayed(&simulated), visible);
    }
}

#[test]
fn repeated_commands_have_distinct_event_ids_and_transient_effects_are_kept() {
    let mut batch = new_batch(&base(), NOW);
    for (index, kind) in ["complete_task", "uncomplete_task", "complete_task"]
        .into_iter()
        .enumerate()
    {
        assert!(
            batch
                .push(command(json!({"cmd":kind,"id":TASK})), index)
                .unwrap()
        );
    }
    assert!(batch.scratch.tasks[&TASK].is_done());
    let pending = batch.freeze().unwrap().unwrap();
    assert_eq!(pending.preview().items.len(), 3);
    assert!(
        pending
            .preview()
            .items
            .iter()
            .all(|item| item.effects[0].kind() == ProposalKind::Complete)
    );
}

#[test]
fn destructive_effects_can_be_displayed_but_cannot_be_drafted() {
    let mut batch = new_batch(&base(), NOW);
    let delete = command(json!({"cmd":"delete","id":{"kind":"task","id":TASK}}));
    let events = todo_core::decide(&batch.scratch, &Actor::System, &delete, NOW).unwrap();
    let effect = batch.effect(&events[0].payload).unwrap().unwrap();
    assert_eq!(effect.kind(), ProposalKind::Destructive);
    assert_eq!(batch.push(delete, 0), Err(BatchError::UnsupportedTool));
    assert!(batch.freeze().unwrap().is_none());
}

#[test]
fn model_titles_reject_controls_format_private_use_unassigned_and_line_breaks() {
    for bad in [
        '\n', '\r', '\t', '\u{0085}', '\u{202e}', '\u{200b}', '\u{2066}', '\u{2028}', '\u{2029}',
        '\u{200d}', '\u{e000}', '\u{0378}',
    ] {
        let title = format!("Vis{bad}ible");
        let mut batch = new_batch(&base(), NOW);
        for cmd in [
            json!({"cmd":"create_task","id":NEW,"list":LIST,"title":title}),
            json!({"cmd":"update_task","id":TASK,"patch":{"title":title}}),
        ] {
            assert_eq!(batch.push(command(cmd), 0), Err(BatchError::InvalidDraft));
        }
    }
}

#[test]
fn normalizes_new_and_edited_titles_before_preview_digest_and_storage() {
    let base = base();
    let mut batch = new_batch(&base, NOW);
    let decomposed = "Z\u{30c}lut\u{30c}ouc\u{30c}ky\u{301}";
    let composed = "Žluťoučký";
    assert!(
        batch
            .push(
                command(json!({"cmd":"create_task","id":NEW,"list":LIST,"title":decomposed})),
                0
            )
            .unwrap()
    );
    assert!(
        batch
            .push(
                command(json!({"cmd":"update_task","id":TASK,"patch":{"title":decomposed}})),
                1
            )
            .unwrap()
    );
    let draft = batch.freeze().unwrap().unwrap();
    assert!(
        matches!(&draft.preview().items[0].effects[0], Effect::Created { task, .. } if task.title == composed)
    );
    assert!(
        matches!(&draft.preview().items[1].effects[0], Effect::Edited { changes, .. } if matches!(&changes[0], Edit::Title(c) if c.new == composed))
    );
    let mut req = ApprovalRequest {
        command_id: ID,
        batch: draft.approval().unwrap(),
    };
    let mut confirmed = base.clone();
    for event in req.preflight().unwrap().redecide(&base).unwrap() {
        apply(&mut confirmed, &event).unwrap();
    }
    assert_eq!(confirmed.tasks[&NEW].title, composed);
    assert_eq!(confirmed.tasks[&TASK].title, composed);
    req.batch.commands[1]
        .title_mut()
        .unwrap()
        .clone_from(&decomposed.to_owned());
    assert!(matches!(req.preflight(), Err(BatchError::InvalidDraft)));
}

#[test]
fn caps_commands_and_events_before_adding_to_the_batch() {
    let mut batch = new_batch(&base(), NOW);
    for n in 0..MAX_COMMANDS {
        assert!(
            batch
                .push(
                    command(json!({"cmd":"update_task","id":TASK,"patch":{"title":n.to_string()}})),
                    n
                )
                .unwrap()
        );
    }
    let extra = command(json!({"cmd":"complete_task","id":TASK}));
    assert_eq!(
        batch.push(extra.clone(), MAX_COMMANDS),
        Err(BatchError::Limit)
    );
    let mut req = ApprovalRequest {
        command_id: ID,
        batch: batch.freeze().unwrap().unwrap().approval().unwrap(),
    };
    req.batch
        .commands
        .push(ApprovalCommand::try_from(&extra).unwrap());
    assert!(matches!(req.preflight(), Err(BatchError::Limit)));

    // Current supported commands are typically one event. Prime the counter to exercise the
    // independent event budget without advertising other commands that have broader effects.
    let mut batch = new_batch(&base(), NOW);
    let event = todo_core::decide(&batch.scratch, &Actor::System, &extra, NOW)
        .unwrap()
        .remove(0);
    batch.events.resize(MAX_EVENTS - 1, event);
    assert!(batch.push(extra, 0).unwrap());
    assert_eq!(
        batch.push(command(json!({"cmd":"uncomplete_task","id":TASK})), 1),
        Err(BatchError::Limit)
    );
    assert!(batch.scratch.tasks[&TASK].is_done());
}
