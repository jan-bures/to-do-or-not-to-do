//! Writes must not reveal what an agent cannot see.
//!
//! `todo-core` checks levels before it looks entities up, so without this a command on a task in a
//! list the agent cannot read would get `forbidden` while one on a missing task gets `not_found`, which
//! tells the agent the hidden task exists. Likewise for undo, by sequence number.
//!
//! So the shared write path (`POST /commands` and `POST /events/{seq}/undo`, which MCP goes through
//! too) runs these checks first, against the current state:
//!
//! - every entity a command refers to (not the ones it creates) must exist and be readable by the
//!   caller, or the response is exactly what core gives for a missing entity: 404 `not_found` with that
//!   entity;
//! - the event an undo targets must exist and be visible to the caller (both where it happened, by
//!   the scope recorded with it, and where its entities are now: [`visibility::event_visible`]), or the
//!   response is exactly the one for a missing sequence number: 404 `event_not_found`. This runs before
//!   the expected-event check, so an agent never learns which event sits at a hidden seq.
//!
//! A visible target the caller lacks the level for still gets `forbidden` from core, and a visible event
//! of another actor still gets `not_own_event`. The checks run before the engine's idempotency lookup,
//! so a retry whose targets have since become hidden gets the same `not_found` instead of the stored
//! result; the record is kept and the write never runs twice.
//!
//! They read a snapshot taken just before the engine takes the writer, so a grant change that lands in
//! between is not seen by this one request.
//!
//! Imports (`import_entity`, owner-only) are refused for agents here with one fixed `forbidden`,
//! whatever list they name.
//!
//! IDs a command *creates* are global, so a colliding one is refused whatever list holds the entity
//! that has it, and even if that entity's creation was undone (IDs are never reused). For agents the
//! refusal is one generic 409 `id_unavailable` ([`write_error`], [`domain_write_error`]): it carries
//! only the ID the agent chose and reads the same for an entity they can see, one they cannot, and an
//! ID held only by the undo journal, so it never depends on hidden state.
//!
//! Only agents are filtered (see [`visibility::sees_all`]); the helpers there decide what is readable.

use super::error::{ApiError, domain_error};
use super::visibility;
use crate::engine::{Engine, EngineError};
use axum::http::StatusCode;
use todo_core::AccessLevel;
use todo_core::access::entity_target;
use todo_core::command::{Command, Placement};
use todo_core::error::{AccessDenied, DomainError};
use todo_core::ids::EntityId;
use todo_core::{Actor, State};

/// The entities `cmd` refers to and expects to exist (not the IDs it creates).
#[must_use]
pub fn referenced(cmd: &Command) -> Vec<EntityId> {
    use Command as C;
    let mut v = Vec::new();
    let place = |p: &Placement, v: &mut Vec<EntityId>| {
        if let Placement::Before(s) | Placement::After(s) = p {
            v.push(*s);
        }
    };
    match cmd {
        C::CreateCollection { placement, .. } => place(placement, &mut v),
        C::UpdateCollection { id, .. } => v.push(EntityId::Collection(*id)),
        C::CreateList {
            collection,
            placement,
            ..
        } => {
            v.extend(collection.map(EntityId::Collection));
            place(placement, &mut v);
        }
        C::UpdateList { id, .. } => v.push(EntityId::List(*id)),
        C::MoveList {
            id,
            collection,
            placement,
        } => {
            v.push(EntityId::List(*id));
            v.extend(collection.map(EntityId::Collection));
            place(placement, &mut v);
        }
        C::CreateTask(n) => {
            v.push(EntityId::List(n.list));
            v.extend(n.tags.iter().map(|t| EntityId::Tag(*t)));
            place(&n.placement, &mut v);
        }
        C::UpdateTask { id, .. }
        | C::CompleteTask { id }
        | C::UncompleteTask { id }
        | C::SetMyDay { id, .. } => v.push(EntityId::Task(*id)),
        C::MoveTask {
            id,
            list,
            placement,
        } => {
            v.push(EntityId::Task(*id));
            v.push(EntityId::List(*list));
            place(placement, &mut v);
        }
        C::AddTag { task, tag } | C::RemoveTag { task, tag } => {
            v.push(EntityId::Task(*task));
            v.push(EntityId::Tag(*tag));
        }
        C::MergeTasks { source, target } => {
            v.push(EntityId::Task(*source));
            v.push(EntityId::Task(*target));
        }
        C::AddNote { task, .. }
        | C::AddLink { task, .. }
        | C::AddAttachment { task, .. }
        | C::AddReminder { task, .. } => v.push(EntityId::Task(*task)),
        C::CreateStep {
            task, placement, ..
        } => {
            v.push(EntityId::Task(*task));
            place(placement, &mut v);
        }
        C::UpdateStep { id, .. } | C::CompleteStep { id } | C::UncompleteStep { id } => {
            v.push(EntityId::Step(*id));
        }
        C::CreateTag { .. } => {}
        C::UpdateTag { id, .. } => v.push(EntityId::Tag(*id)),
        C::UpdateReminder { id, .. } => v.push(EntityId::Reminder(*id)),
        C::Reorder { id, placement } => {
            v.push(*id);
            place(placement, &mut v);
        }
        C::Delete { id } | C::Restore { id } => v.push(*id),
        // Owner-only, and refused for agents before any lookup (see `check_command`).
        C::ImportEntity { .. } => {}
    }
    v
}

/// True if the entity is in the state (live or deleted).
fn exists(state: &State, id: EntityId) -> bool {
    match id {
        EntityId::Collection(x) => state.collections.contains_key(&x),
        EntityId::List(x) => state.lists.contains_key(&x),
        EntityId::Task(x) => state.tasks.contains_key(&x),
        EntityId::Step(x) => state.steps.contains_key(&x),
        EntityId::Tag(x) => state.tags.contains_key(&x),
        EntityId::Link(x) => state.links.contains_key(&x),
        EntityId::Attachment(x) => state.attachments.contains_key(&x),
        EntityId::Reminder(x) => state.reminders.contains_key(&x),
    }
}

/// True if `actor` can see the existing entity `id`, by the read rules in [`visibility`].
fn readable(state: &State, actor: &Actor, id: EntityId) -> bool {
    match id {
        EntityId::Collection(c) => {
            visibility::visible_collections(state, actor).is_none_or(|visible| visible.contains(&c))
        }
        EntityId::Tag(t) => {
            visibility::visible_tags(state, actor).is_none_or(|visible| visible.contains(&t))
        }
        other => visibility::can_read(state, actor, entity_target(state, other)),
    }
}

/// 409 `id_unavailable`: the ID an agent chose for a new entity cannot be used. The same for every
/// collision, whoever holds the ID and whether the agent can see it.
#[must_use]
pub fn id_unavailable(id: EntityId) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "id_unavailable",
        "this ID cannot be used; choose another",
    )
    .with_details(serde_json::json!({ "entity": id }))
}

/// [`domain_error`], with ID collisions answered as [`id_unavailable`] for agents.
#[must_use]
pub fn domain_write_error(actor: &Actor, e: &DomainError) -> ApiError {
    match e {
        DomainError::AlreadyExists(id) if !visibility::sees_all(actor) => id_unavailable(*id),
        e => domain_error(e),
    }
}

/// The response to a failed write, with ID collisions answered as [`id_unavailable`] for agents.
#[must_use]
pub fn write_error(actor: &Actor, e: EngineError) -> ApiError {
    match e {
        EngineError::Domain(d) => domain_write_error(actor, &d),
        e => e.into(),
    }
}

/// `not_found` for the first entity `cmd` refers to that is missing or that `actor` cannot read, and
/// `forbidden` for an import.
pub fn check_command(state: &State, actor: &Actor, cmd: &Command) -> Result<(), ApiError> {
    if visibility::sees_all(actor) {
        return Ok(());
    }
    if matches!(cmd, Command::ImportEntity { .. }) {
        // Core would refuse it too, but with the caller's level on the import's target list, which
        // tells a hidden list from a missing one. No agent may import anywhere: one fixed answer.
        return Err(domain_error(&DomainError::Denied(AccessDenied {
            needed: AccessLevel::Plan,
            have: AccessLevel::None,
            owner_only: true,
        })));
    }
    match referenced(cmd)
        .into_iter()
        .find(|id| !exists(state, *id) || !readable(state, actor, *id))
    {
        Some(id) => Err(domain_error(&DomainError::NotFound(id))),
        None => Ok(()),
    }
}

/// `event_not_found` (as for a missing sequence number) unless the event at `seq` exists and `actor`
/// can see it. Blocking: reads the event from the log.
pub fn check_undo(engine: &Engine, actor: &Actor, seq: u64) -> Result<(), ApiError> {
    if visibility::sees_all(actor) {
        return Ok(());
    }
    // One event, with the scope recorded when it was written; no byte budget, as nothing is sent.
    let page = engine.events_since_capped(seq.saturating_sub(1), 1, usize::MAX)?;
    let visible = page
        .events
        .iter()
        .find(|e| e.event.seq == Some(seq))
        .is_some_and(|e| {
            let scope = page.scopes.get(&seq);
            // `state` before `whereabouts`, as `Engine::whereabouts` asks.
            let state = engine.state();
            let gone = engine.whereabouts();
            visibility::event_visible(&state, &gone, actor, &e.event, scope)
        });
    if visible {
        Ok(())
    } else {
        Err(EngineError::EventNotFound(seq).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn creations_refer_to_their_parents_not_to_their_new_ids() {
        let (task, list) = (Uuid::new_v4(), Uuid::new_v4());
        let cmd: Command = serde_json::from_value(json!({
            "cmd": "create_task", "id": task, "list": list, "title": "t",
        }))
        .unwrap();
        let refs = referenced(&cmd);
        assert_eq!(refs.len(), 1);
        assert!(matches!(refs[0], EntityId::List(_)));
        let cmd: Command = serde_json::from_value(json!({
            "cmd": "move_task", "id": task, "list": list,
            "placement": { "at": "after", "sibling": { "kind": "task", "id": Uuid::new_v4() } },
        }))
        .unwrap();
        assert_eq!(referenced(&cmd).len(), 3);
    }
}
