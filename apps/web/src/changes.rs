//! The browser's presentation of refused changes and the fixture's exact local undo.

use todo_client::{CommandId, FailedEntry};
use todo_core::ids::EntityId;
use todo_core::{Command, State};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub id: CommandId,
    pub what: String,
    pub why: &'static str,
}

impl Failure {
    pub fn from_entry(f: &FailedEntry, state: &State) -> Self {
        let (what, why) = match f.entry.command() {
            Some(cmd) => (describe(cmd, state), failure_reason(&f.rejection.code)),
            None => ("Undo a change".into(), undo_failure(&f.rejection.code)),
        };
        Self {
            id: f.entry.command_id,
            what,
            why,
        }
    }
}

pub fn undo_failure(code: &str) -> &'static str {
    match code {
        "undo_conflict" => "Couldn't undo: the item was changed again.",
        "undo_no_event" => "Couldn't undo: the original change couldn't be found.",
        "epoch_mismatch" | "event_mismatch" => "Couldn't undo: the server's history changed.",
        "not_found" => "Couldn't undo: the item no longer exists.",
        _ => "Couldn't undo that.",
    }
}

fn failure_reason(code: &str) -> &'static str {
    match code {
        "deleted" | "already_deleted" | "not_found" => {
            "The item or where it belongs no longer exists."
        }
        "forbidden" => "You don't have permission to make this change.",
        "conflict" => "The item changed elsewhere before this change could be saved.",
        _ => "The server couldn't apply this change.",
    }
}

fn describe(cmd: &Command, state: &State) -> String {
    let about = |verb, id| {
        let (kind, name) = match id {
            EntityId::List(id) => ("list", state.lists.get(&id).map(|l| l.name.as_str())),
            EntityId::Collection(id) => (
                "collection",
                state.collections.get(&id).map(|c| c.name.as_str()),
            ),
            EntityId::Task(id) => ("task", state.tasks.get(&id).map(|t| t.title.as_str())),
            EntityId::Step(id) => ("step", state.steps.get(&id).map(|s| s.title.as_str())),
            _ => ("item", None),
        };
        name.map_or_else(
            || format!("{verb}: {kind}"),
            |name| format!("{verb} {kind} “{name}”"),
        )
    };
    match cmd {
        Command::CreateList { name, .. } => format!("Create list “{name}”"),
        Command::UpdateList {
            name: Some(name), ..
        } => format!("Rename list to “{name}”"),
        Command::UpdateCollection {
            name: Some(name), ..
        } => format!("Rename collection to “{name}”"),
        Command::CreateTask(task) => format!("Add task “{}”", task.title),
        Command::UpdateTask { id, .. } => about("Edit", (*id).into()),
        Command::CompleteTask { id } => about("Complete", (*id).into()),
        Command::UncompleteTask { id } => about("Reopen", (*id).into()),
        Command::SetMyDay { id, .. } => about("Change My Day for", (*id).into()),
        Command::CreateStep { title, .. } => format!("Add step “{title}”"),
        Command::UpdateStep { title, .. } => format!("Rename step to “{title}”"),
        Command::CompleteStep { id } => about("Complete", (*id).into()),
        Command::UncompleteStep { id } => about("Reopen", (*id).into()),
        Command::Reorder { id, .. } => about("Reorder", *id),
        Command::Delete { id } => about("Delete", *id),
        _ => "Save a change".into(),
    }
}

#[cfg(any(test, feature = "fixtures"))]
pub mod fixture {
    use todo_core::undo::{UndoOptions, undo_batch};
    use todo_core::{Actor, Event, State, apply, time::Timestamp};

    /// Apply a whole batch to a copy, so a failure cannot leave a partly changed fixture.
    pub fn apply_batch(state: &State, events: &[Event]) -> Result<State, String> {
        let mut next = state.clone();
        for e in events {
            apply(&mut next, e).map_err(|e| e.to_string())?;
        }
        Ok(next)
    }

    pub fn undo(
        state: &State,
        events: &[Event],
        actor: &Actor,
        at: Timestamp,
    ) -> Result<State, String> {
        let inverse = undo_batch(state, events, actor, at, UndoOptions::default())
            .map_err(|_| super::undo_failure("undo_conflict").to_owned())?;
        apply_batch(state, &inverse).map_err(|_| super::undo_failure("").to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use todo_client::intent;
    use todo_core::{Actor, State, decide};

    #[test]
    fn recurring_undo_reopens_original_and_removes_next_occurrence() {
        let before = fixtures::state();
        let id = fixtures::task(1);
        let at = fixtures::now().at;
        let cmd = intent::set_done(&before, id, true).expect("complete the original");
        let actor = Actor::owner("web").expect("owner");
        let events = decide(&before, &actor, &cmd, at).expect("completion batch");
        let completed = fixture::apply_batch(&before, &events).expect("apply completion");
        let live = |state: &State| state.tasks.values().filter(|t| t.deleted.is_none()).count();
        assert_eq!(live(&completed), live(&before) + 1);
        let undone = fixture::undo(&completed, &events, &actor, at).expect("undo batch");
        assert_eq!(live(&undone), live(&before));
        assert!(!undone.tasks[&id].is_done());
        assert_eq!(undone.tasks[&id].due, before.tasks[&id].due);
    }

    #[test]
    fn undo_reinstates_each_original_sibling_key() {
        let before = fixtures::state();
        let at = fixtures::now().at;
        let actor = Actor::owner("web").expect("owner");
        for id in before
            .collections
            .keys()
            .copied()
            .map(Into::into)
            .chain(before.lists.keys().copied().map(Into::into))
        {
            let scope = before.scope_of(id).expect("scope");
            for by in [-1, 1] {
                let Some(cmd) = intent::nudge(&before, id, by) else {
                    continue;
                };
                let events = decide(&before, &actor, &cmd, at).expect("reorder batch");
                let reordered = fixture::apply_batch(&before, &events).expect("apply reorder");
                assert_ne!(reordered.siblings(scope), before.siblings(scope));
                let undone = fixture::undo(&reordered, &events, &actor, at).expect("undo batch");
                assert_eq!(undone.siblings(scope), before.siblings(scope));
            }
        }
    }

    #[test]
    fn refusal_codes_have_plain_language_fallbacks() {
        for code in [
            "undo_conflict",
            "undo_no_event",
            "epoch_mismatch",
            "event_mismatch",
            "not_found",
            "raw server text",
        ] {
            assert!(undo_failure(code).starts_with("Couldn't undo"));
            assert!(!undo_failure(code).contains("raw server text"));
            assert!(!failure_reason(code).contains("raw server text"));
        }
    }
}
