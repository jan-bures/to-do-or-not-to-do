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
        let mut replay = new_batch(&base, NOW);
        let mut retimed = new_batch(&base, at);
        let mut all_events = Vec::new();
        let mut ids = BTreeSet::new();
        for (command, item) in pending.commands().iter().zip(&pending.preview().items) {
            let seed = format!("{}:{}", pending.preview().id, item.index);
            let events = decide_seeded(&replay.scratch, &replay.actor, command, NOW, &seed).unwrap();
            prop_assert!(!events.is_empty());
            for event in events {
                prop_assert!(ids.insert(event.id));
                apply(&mut replay.scratch, &event).unwrap();
                all_events.push(event);
            }
            prop_assert!(retimed.push(command.clone(), item.index).unwrap());
        }
        let mut rebuilt = new_batch(&base, NOW).scratch;
        for event in &all_events { apply(&mut rebuilt, event).unwrap(); }
        prop_assert_eq!(&rebuilt, &simulated);
        let replayed = retimed.freeze().unwrap().unwrap();
        // Different event stamps/IDs don't change the actual effect review.
        prop_assert_eq!(pending.preview(), replayed.preview());
        prop_assert_eq!(displayed(&rebuilt), visible);
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
    assert_eq!(batch.push(delete, 0), Err(AssistantError::UnsupportedTool));
    assert!(batch.freeze().unwrap().is_none());
}
