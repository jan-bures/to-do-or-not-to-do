//! The browser's presentation of refused changes and the fixture's exact local undo.

use todo_client::{CommandId, FailedEntry, SubmitError, failure};
use todo_core::State;
use todo_core::error::DomainError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub id: CommandId,
    pub what: String,
    pub why: &'static str,
}

impl Failure {
    pub fn from_entry(f: &FailedEntry, state: &State) -> Self {
        let what = failure::what(&f.entry);
        let what = failure::name(&f.entry, state)
            .map_or_else(|| what.to_owned(), |name| format!("{what}: “{name}”"));
        let why = if f.entry.command().is_none() {
            undo_failure(&f.rejection.code)
        } else {
            failure::why(f)
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
        "undo_no_event" | "event_not_found" => {
            "Couldn't undo: the original change couldn't be found."
        }
        "epoch_mismatch" | "event_mismatch" => "Couldn't undo: the server's history changed.",
        "not_found" => "Couldn't undo: the item no longer exists.",
        _ => "Couldn't undo that.",
    }
}

/// Validation comes from local `decide`, not from a server response. Storage errors are only logged.
pub fn submission_error(error: &SubmitError) -> String {
    let message = match error {
        SubmitError::Invalid(DomainError::Invalid { field, reason }) => {
            return format!("Couldn’t save: {field} {reason}.");
        }
        SubmitError::Invalid(DomainError::DuplicateName(_)) => "That name is already taken.",
        SubmitError::Invalid(
            DomainError::Deleted(_) | DomainError::AlreadyDeleted(_) | DomainError::NotFound(_),
        ) => "The item or where it belongs no longer exists.",
        SubmitError::Invalid(DomainError::BadPlacement(_)) => {
            "The items no longer belong to the same list or collection."
        }
        _ => "Couldn’t save that change.",
    };
    message.into()
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
        }
    }

    #[test]
    fn submission_keeps_local_validation_but_not_storage_details() {
        let invalid = DomainError::Invalid {
            field: "name",
            reason: "must not be empty".into(),
        };
        assert_eq!(
            submission_error(&invalid.into()),
            "Couldn’t save: name must not be empty."
        );
        let storage = todo_client::StorageError::Backend("internal details".into());
        assert_eq!(
            submission_error(&storage.into()),
            "Couldn’t save that change."
        );
    }
}
