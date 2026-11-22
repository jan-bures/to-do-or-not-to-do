//! Writes must not reveal what an agent cannot see.
//!
//! `todo-core` checks levels before it looks entities up, so without this a command on a task in a
//! list the agent cannot read would get `forbidden` while one on a missing task gets `not_found`, which
//! tells the agent the hidden task exists. Likewise for undo, by sequence number.
//!
//! So the shared write paths (`POST /commands` and `POST /events/{seq}/undo`, which MCP goes through
//! too, and the attachment upload) run these checks first:
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
//! **Inside the writer.** Core checks the level only on the entity a command changes, not on every
//! entity it refers to (`add_tag` checks the task, and only that the tag exists), so visibility decided
//! here must not go stale before the write. The authoritative checks ([`command_check`],
//! [`undo_check`]) run inside the engine's writer ([`Engine::execute_checked`],
//! [`Engine::undo_checked`]), against the exact state the idempotency lookup and `decide` use; grant
//! changes take the writer too ([`Engine::set_grants`]). A target that became hidden while the request
//! waited for the writer (its grant was revoked, its list deleted or moved) gets the same answer as a
//! missing one and nothing is stored. For commands, [`check_command`] also runs before the writer on a
//! snapshot, as a fast path that refuses without waiting; it is cheap (in memory) and never the last
//! word. Undo is checked only inside the writer, as its check reads the log. An attachment upload
//! retry decides its replay inside the writer too, after its (blocking) lookups of the stored result
//! and the blob, which stay outside it.
//!
//! **Cost.** The check holds the single writer, so it stays bounded: the visible tags (a scan of every
//! task) and collections are built at most once per check, and a command lists at most
//! [`MAX_TASK_TAGS`](todo_core::decide::MAX_TASK_TAGS) tags, refused with 422 before the writer
//! ([`check_command`], for every caller, so the answer depends on nothing hidden).
//!
//! **Timing.** A hidden target and a missing one get the same response, but not necessarily in the
//! same time: refusing a hidden one may take a little longer (it is found, then its visibility is
//! computed). Accepted: the difference is small and noisy next to network latency and the writer
//! queue, and closing it would mean doing the visibility work for missing targets too. The same holds
//! for `HEAD` and `GET` on `/api/v1/attachments/{id}`, which confirm visibility in the writer after
//! their blob lookup (see [`super::attachments`]).
//!
//! Email ingest writes as `email-ingest`, which is not an agent (it sees everything), and picks its list
//! inside the writer, so it needs none of this.
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
#[cfg(doc)]
use crate::engine::Engine;
use crate::engine::{EngineError, Locked};
use axum::http::StatusCode;
use std::cell::{Cell, OnceCell};
use std::collections::BTreeSet;
use todo_core::AccessLevel;
use todo_core::access::entity_target;
use todo_core::command::{Command, Placement};
use todo_core::decide::check_limits;
use todo_core::error::{AccessDenied, DomainError};
use todo_core::ids::{CollectionId, EntityId, TagId};
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
            ..
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

/// What `actor` can read in `state`, by the read rules in [`visibility`]. The visible collections and
/// tags are each computed at most once (the tags scan every task), however many references a check
/// looks at.
struct Readable<'a> {
    state: &'a State,
    actor: &'a Actor,
    collections: OnceCell<Option<BTreeSet<CollectionId>>>,
    tags: OnceCell<Option<BTreeSet<TagId>>>,
    /// How many visibility sets were built, for the tests.
    scans: Cell<usize>,
}

impl<'a> Readable<'a> {
    fn new(state: &'a State, actor: &'a Actor) -> Self {
        Self {
            state,
            actor,
            collections: OnceCell::new(),
            tags: OnceCell::new(),
            scans: Cell::new(0),
        }
    }

    fn scanned<T>(&self, f: impl FnOnce() -> T) -> T {
        self.scans.set(self.scans.get() + 1);
        f()
    }

    /// True if the existing entity `id` is readable.
    fn readable(&self, id: EntityId) -> bool {
        let (state, actor) = (self.state, self.actor);
        match id {
            EntityId::Collection(c) => self
                .collections
                .get_or_init(|| self.scanned(|| visibility::visible_collections(state, actor)))
                .as_ref()
                .is_none_or(|visible| visible.contains(&c)),
            EntityId::Tag(t) => self
                .tags
                .get_or_init(|| self.scanned(|| visibility::visible_tags(state, actor)))
                .as_ref()
                .is_none_or(|visible| visible.contains(&t)),
            other => visibility::can_read(state, actor, entity_target(state, other)),
        }
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

/// The error core would give if `cmd` referred to a missing entity, for the first entity it refers to
/// that is missing or that `actor` cannot read; `Denied` for an import by an agent.
fn hidden_reference(state: &State, actor: &Actor, cmd: &Command) -> Option<DomainError> {
    hidden_reference_in(&Readable::new(state, actor), cmd)
}

fn hidden_reference_in(r: &Readable<'_>, cmd: &Command) -> Option<DomainError> {
    let (state, actor) = (r.state, r.actor);
    if visibility::sees_all(actor) {
        return None;
    }
    if matches!(cmd, Command::ImportEntity { .. }) {
        // Core would refuse it too, but with the caller's level on the import's target list, which
        // tells a hidden list from a missing one. No agent may import anywhere: one fixed answer.
        return Some(DomainError::Denied(AccessDenied {
            needed: AccessLevel::Plan,
            have: AccessLevel::None,
            owner_only: true,
        }));
    }
    referenced(cmd)
        .into_iter()
        .find(|id| !exists(state, *id) || !r.readable(*id))
        .map(DomainError::NotFound)
}

/// `not_found` for the first entity `cmd` refers to that is missing or that `actor` cannot read, and
/// `forbidden` for an import. The fast path before the writer; [`command_check`] decides.
///
/// First, for every caller, the limits core checks whatever the state ([`check_limits`]), so that an
/// oversized command is refused with a 422 that depends on nothing hidden, and never reaches the
/// writer.
pub fn check_command(state: &State, actor: &Actor, cmd: &Command) -> Result<(), ApiError> {
    check_limits(cmd).map_err(|e| domain_error(&e))?;
    hidden_reference(state, actor, cmd).map_or(Ok(()), |e| Err(domain_error(&e)))
}

/// [`check_command`] as a check inside the writer, for [`Engine::execute_checked`]. Its error, mapped
/// by [`write_error`], is exactly the one [`check_command`] gives.
pub fn command_check<'a>(
    actor: &'a Actor,
    cmd: &'a Command,
) -> impl FnOnce(&Locked<'_>) -> Result<(), EngineError> + 'a {
    move |locked| hidden_reference(locked.state, actor, cmd).map_or(Ok(()), |e| Err(e.into()))
}

/// A check inside the writer, for [`Engine::undo_checked`]: `event_not_found` (as for a missing
/// sequence number) unless the event at `seq` exists and `actor` can see it.
pub fn undo_check(
    actor: &Actor,
    seq: u64,
) -> impl FnOnce(&Locked<'_>) -> Result<(), EngineError> + '_ {
    move |locked| {
        if visibility::sees_all(actor) {
            return Ok(());
        }
        // The event with the scope recorded when it was written.
        let visible = locked.event_with_scope(seq)?.is_some_and(|(event, scope)| {
            visibility::event_visible(locked.state, locked.gone, actor, &event, scope.as_ref())
        });
        if visible {
            Ok(())
        } else {
            Err(EngineError::EventNotFound(seq))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use todo_core::{Grants, apply, decide};
    use uuid::Uuid;

    #[test]
    fn a_check_builds_each_visibility_set_once_however_many_tags() {
        let owner = Actor::owner("pc").unwrap();
        let mut state = State::new();
        let run = |state: &mut State, cmd: serde_json::Value| {
            let cmd: Command = serde_json::from_value(cmd).unwrap();
            for e in decide(state, &owner, &cmd, todo_core::time::Timestamp::now()).unwrap() {
                apply(state, &e).unwrap();
            }
        };
        let (list, task) = (Uuid::new_v4(), Uuid::new_v4());
        run(
            &mut state,
            json!({ "cmd": "create_list", "id": list, "name": "L" }),
        );
        let tags: Vec<Uuid> = (0..todo_core::decide::MAX_TASK_TAGS)
            .map(|_| Uuid::new_v4())
            .collect();
        for t in &tags {
            run(
                &mut state,
                json!({ "cmd": "create_tag", "id": t, "name": t.to_string() }),
            );
        }
        run(
            &mut state,
            json!({ "cmd": "create_task", "id": task, "list": list, "title": "t", "tags": tags }),
        );
        let agent = Actor::agent("scout").unwrap();
        let mut g = Grants::with_default(AccessLevel::None);
        g.lists.insert(
            serde_json::from_value(json!(list)).unwrap(),
            AccessLevel::Plan,
        );
        state.grants.insert(agent.agent_id().unwrap().clone(), g);

        let cmd: Command = serde_json::from_value(json!({
            "cmd": "create_task", "id": Uuid::new_v4(), "list": list, "title": "u", "tags": tags,
        }))
        .unwrap();
        let r = Readable::new(&state, &agent);
        assert!(hidden_reference_in(&r, &cmd).is_none());
        assert_eq!(
            r.scans.get(),
            1,
            "the visible tags are built once per check"
        );
    }

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
