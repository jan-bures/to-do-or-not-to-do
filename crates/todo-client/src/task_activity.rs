//! Task detail's compact activity summary, from the local replica rather than the Activity feed.

use std::cmp::Reverse;

use todo_core::ids::TaskId;
use todo_core::time::Timestamp;
use todo_core::{Actor, State};

use crate::activity::{actor_name, relative_time};

/// One read-only line in task detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The actor, when known, for choosing a mark. Never display an owner device ID as a name.
    pub actor: Option<Actor>,
    /// Display name, for example `You` or `Codex`.
    pub who: String,
    /// Action, for example `created this`.
    pub action: &'static str,
    /// Relative time, for example `2 hr ago`.
    pub when: String,
}

/// The retained creation, latest edit and current completion, newest first. A completion is
/// not also an edit. This is a summary, not a log of every change or a source of Undo actions.
#[must_use]
pub fn lines(state: &State, id: TaskId, now: Timestamp) -> Vec<Line> {
    let Some(task) = state.tasks.get(&id) else {
        return Vec::new();
    };
    let mut rows = Vec::with_capacity(3);
    let mut push = |at, actor: Option<&Actor>, action| {
        rows.push((
            at,
            Line {
                actor: actor.cloned(),
                who: match actor {
                    Some(Actor::EmailIngest) => "Email".to_owned(),
                    Some(actor) => actor_name(actor),
                    None => "Unknown actor".to_owned(),
                },
                action,
                when: relative_time(at, now),
            },
        ));
    };
    let edited = task.updated != task.created;
    let mut completion_is_edit = false;
    if let Some(at) = task.completed_at {
        let same_time = edited && task.updated.at == at;
        // Older or reduced snapshots may lack field provenance. Only infer from a matching
        // last-change stamp, never from a later editor or the importer of a historical task.
        let actor = state.task_completion_actor(id).or_else(|| {
            (same_time && task.updated.by != Actor::Importer).then_some(&task.updated.by)
        });
        completion_is_edit = same_time && actor == Some(&task.updated.by);
        push(at, actor, "completed this");
    }
    if edited && !completion_is_edit {
        push(task.updated.at, Some(&task.updated.by), "edited this");
    }
    let action = if task.created.by == Actor::EmailIngest {
        "created this from an email"
    } else {
        "created this"
    };
    push(task.created.at, Some(&task.created.by), action);
    // Stable on ties: completion, edit, then creation.
    rows.sort_by_key(|(at, _)| Reverse(*at));
    rows.into_iter().map(|(_, row)| row).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use todo_core::event::{Change, Event, EventPayload, FieldChange};
    use todo_core::ids::{EventId, ListId};
    use todo_core::model::{Priority, Stamp, Task};
    use todo_core::ordering::PosKey;
    use uuid::Uuid;

    fn at(s: i64) -> Timestamp {
        Timestamp::from_second(s).unwrap()
    }

    fn sample(by: Actor) -> (State, TaskId) {
        let mut state = State::new();
        let id = TaskId::from_uuid(Uuid::from_u128(1));
        let stamp = Stamp { at: at(0), by };
        state.tasks.insert(
            id,
            Task {
                id,
                list: ListId::from_uuid(Uuid::from_u128(2)),
                title: "Write a draft".into(),
                notes: String::new(),
                due: None,
                priority: Priority::None,
                my_day: None,
                completed_at: None,
                recurrence: None,
                tags: Default::default(),
                position: PosKey::between(None, None).unwrap(),
                created: stamp.clone(),
                updated: stamp,
                deleted: None,
                merged_into: None,
            },
        );
        (state, id)
    }

    fn shown(state: &State, id: TaskId) -> Vec<(String, &'static str, String)> {
        lines(state, id, at(7_200))
            .into_iter()
            .map(|l| (l.who, l.action, l.when))
            .collect()
    }

    #[test]
    fn creation_alone_identifies_owner_email_and_named_agents() {
        for (by, name, action) in [
            (
                Actor::owner("sample-device").unwrap(),
                "You",
                "created this",
            ),
            (Actor::EmailIngest, "Email", "created this from an email"),
            (Actor::agent("codex").unwrap(), "Codex", "created this"),
            (
                Actor::agent("sample-agent").unwrap(),
                "Sample-agent",
                "created this",
            ),
            (Actor::Importer, "Import", "created this"),
            (Actor::System, "System", "created this"),
        ] {
            let (state, id) = sample(by.clone());
            assert_eq!(
                shown(&state, id),
                [(name.into(), action, "2 hr ago".into())]
            );
            assert_eq!(lines(&state, id, at(0))[0].actor, Some(by));
        }
    }

    #[test]
    fn completion_is_not_an_edit_and_later_edits_keep_the_completer() {
        let (mut state, id) = sample(Actor::EmailIngest);
        let complete = Actor::agent("pi").unwrap();
        let owner = Actor::owner("sample-device").unwrap();
        let old = state.tasks[&id].updated.clone();
        let completion = Stamp {
            at: at(3_600),
            by: complete.clone(),
        };
        let edit = Stamp {
            at: at(7_300),
            by: owner.clone(),
        };
        let updated = |n: u128, stamp: &Stamp, changes| Event {
            id: EventId::from_uuid(Uuid::from_u128(n)),
            group: EventId::from_uuid(Uuid::from_u128(n)),
            seq: None,
            at: stamp.at,
            actor: stamp.by.clone(),
            payload: EventPayload::Updated {
                target: id.into(),
                changes,
            },
            undoes: None,
            forced: false,
        };
        todo_core::apply(
            &mut state,
            &updated(
                3,
                &completion,
                vec![
                    FieldChange::CompletedAt(Change::new(None, Some(completion.at))),
                    FieldChange::Touch(Change::new(old, completion.clone())),
                ],
            ),
        )
        .unwrap();
        assert_eq!(
            shown(&state, id),
            [
                ("Pi".into(), "completed this", "1 hr ago".into()),
                (
                    "Email".into(),
                    "created this from an email",
                    "2 hr ago".into()
                ),
            ]
        );
        todo_core::apply(
            &mut state,
            &updated(
                4,
                &edit,
                vec![FieldChange::Touch(Change::new(completion, edit.clone()))],
            ),
        )
        .unwrap();
        assert_eq!(
            shown(&state, id),
            [
                ("You".into(), "edited this", "Just now".into()),
                ("Pi".into(), "completed this", "1 hr ago".into()),
                (
                    "Email".into(),
                    "created this from an email",
                    "2 hr ago".into()
                ),
            ]
        );
        todo_core::apply(
            &mut state,
            &updated(
                5,
                &edit,
                vec![FieldChange::CompletedAt(Change::new(Some(at(3_600)), None))],
            ),
        )
        .unwrap();
        assert_eq!(shown(&state, id).len(), 2);
    }

    #[test]
    fn imported_completion_without_provenance_does_not_guess_an_actor() {
        let (mut state, id) = sample(Actor::Importer);
        let task = state.tasks.get_mut(&id).unwrap();
        task.updated.at = at(60);
        task.completed_at = Some(at(60));
        assert_eq!(
            shown(&state, id)[0],
            ("Unknown actor".into(), "completed this", "1 hr ago".into())
        );
        assert!(lines(&State::new(), id, at(60)).is_empty());
    }
}
