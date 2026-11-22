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

pub use todo_client::failure::undo_message as undo_failure;

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
    use todo_core::error::{DomainError, UndoError};
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
            .map_err(|error| super::undo_failure(undo_code(error)).to_owned())?;
        apply_batch(state, &inverse).map_err(|_| super::undo_failure("").to_owned())
    }

    // Match the server's api::error::undo_error without depending on its HTTP stack.
    fn undo_code(error: UndoError) -> &'static str {
        match error {
            UndoError::Denied(_) => "forbidden",
            UndoError::NotOwnEvent => "not_own_event",
            UndoError::UnknownEvent(_) => "event_not_found",
            UndoError::EventMismatch(_) => "event_mismatch",
            UndoError::Invariant {
                source: DomainError::DuplicateName(_),
                ..
            } => "duplicate_name",
            UndoError::Invariant { .. } => "undo_invariant",
            UndoError::AlreadyUndone(_) => "already_undone",
            UndoError::Conflict(_) => "undo_conflict",
            UndoError::HasDependents(_) => "has_dependents",
            UndoError::BrokenReference(_) => "broken_reference",
            UndoError::Apply(_) => "internal",
        }
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
    fn undo_tag_delete_explains_a_name_taken_by_another_tag() {
        let before = fixtures::state();
        let at = fixtures::now().at;
        let actor = Actor::owner("web").expect("owner");
        let old = intent::tag_named(&before, "ops").expect("existing fixture tag");
        let deletion = decide(&before, &actor, &intent::delete(old), at).expect("delete old tag");
        let deleted = fixture::apply_batch(&before, &deletion).expect("apply deletion");
        let other =
            todo_core::ids::TagId::from_uuid(todo_core::ids::uuid_v7(1_790_000_000_000, [7; 10]));
        let reuse = intent::create_tag(&deleted, other, "OPS").expect("name free after deletion");
        let events = decide(&deleted, &actor, &reuse, at).expect("create replacement tag");
        let taken = fixture::apply_batch(&deleted, &events).expect("apply creation");
        assert_eq!(
            fixture::undo(&taken, &deletion, &actor, at).expect_err("name now taken"),
            "Couldn't undo: another tag already uses that name."
        );
        assert_eq!(intent::tag_named(&taken, "ops"), Some(other));
        assert!(!taken.is_live(old.into()));
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
