//! Undo: compensating events built from the values the undone event replaced.
//!
//! Nothing is ever deleted from the event log. [`undo`] returns a new event that inverts an earlier one
//! (a creation is withdrawn, a tombstone is removed, changed fields get back the values the event
//! replaced). Each compensation records, as its own previous values, what it actually overwrote, so
//! undoing a compensation (redo) restores exactly the state before it, even after a forced undo.
//!
//! # Checks
//!
//! The supplied event must be exactly the applied event with that ID ([`UndoError::EventMismatch`]
//! otherwise; only the server-assigned `seq` may differ), it must be the newest link of its undo/redo
//! chain, and the actor must be allowed to make the compensating change: the owner may undo anything,
//! anyone else only their own events, at the access level the equivalent command would need on every
//! list the compensation touches (both ends of a move, both tasks of a merge). Reverting notes needs plan
//! access, except for a note added with `AddNote` ([`FieldChange::NoteAdded`]) that is still exactly as
//! it was added.
//!
//! Entity IDs are never reused (see [`Journal::id_used`](crate::state::Journal::id_used)), so an event and
//! its compensation always refer to the same entity; only the undo/redo chain of the original creation
//! can bring a withdrawn ID back.
//!
//! # Conflicts
//!
//! The journal tracks the provenance of every field's value: the event whose write is in effect.
//! Compensations count as writes. Undoing event `E` is a conflict if a field it wrote no longer holds
//! what `E` left there, because a later event (ordinary or compensating) changed it; overwriting it would
//! silently lose that change. Withdrawing a creation, and re-tombstoning a restored entity, touch every
//! field of the entity: any field whose value changed since `E` (and, for a tombstone, any change in or
//! under it), by an ordinary or a compensating event, is a conflict. [`undo`] then fails with
//! [`UndoError::Conflict`] listing what changed, and [`undo_with`] with `force: true` overrides the check
//! (the compensating event is marked `forced`).
//!
//! # Position keys
//!
//! A compensation that puts an entity back among its siblings (undoing a deletion or a move, redoing a
//! creation) may find its old key taken by a sibling placed there since the undone event. Then [`undo`]
//! returns two events: the exact compensation, and an ordinary position update (a *rekey*, in the same
//! group) that gives the entity a fresh key just before that sibling, as `Command::Restore` does, so
//! siblings do not end up sharing a key. Undo such a group as a unit with [`undo_batch`], like any
//! multi-event command; that also reverts the new key. (Undoing only the compensation conflicts on the
//! position.) Undoing a rekey needs only capture access on its target, so whoever could make the
//! compensation can take it back.
//!
//! The last-modified stamp is bookkeeping: it is rolled back only if the stamp in place is still the one
//! the undone event left (tracked like a field), and it never causes a conflict.
//!
//! # Invariants
//!
//! A compensation must leave a valid entity (the rules [`decide`](crate::decide::decide) enforces). If it
//! would not, for example a recurring task losing its due date because the recurrence was added by a
//! later event, the undo fails with [`UndoError::Invariant`], even when forced.

use crate::access::{authorize_inverse, is_rekey};
use crate::actor::Actor;
use crate::apply::{apply, apply_changes};
use crate::decide::{unique_tag_name, validate_entity};
use crate::error::{ApplyError, FieldConflict, UndoConflict, UndoError};
use crate::event::{Change, Event, EventPayload, FieldChange, FieldKey};
use crate::ids::{EntityId, EventId};
use crate::model::Entity;
use crate::state::{Applied, State};
use crate::time::Timestamp;

/// Options for [`undo_with`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UndoOptions {
    /// Undo even if later events changed the same fields.
    pub force: bool,
}

/// The entities a compensation points at must still exist. They may be tombstoned: putting a task back
/// into a list that was deleted meanwhile restores the earlier state exactly (the task is hidden with the
/// list, as it was before).
fn check_refs(state: &State, changes: &[FieldChange]) -> Result<(), UndoError> {
    for c in changes {
        match c {
            FieldChange::TaskList(c) => require_exists(state, c.new.into())?,
            FieldChange::ListCollection(c) => {
                if let Some(col) = c.new {
                    require_exists(state, col.into())?;
                }
            }
            FieldChange::ParentTask(c) => require_exists(state, c.new.into())?,
            FieldChange::MergedInto(c) => {
                if let Some(t) = c.new {
                    require_exists(state, t.into())?;
                }
            }
            FieldChange::TagAdded { tag } => require_exists(state, (*tag).into())?,
            _ => {}
        }
    }
    Ok(())
}

/// The change that puts `c.old` back, recording the entity's current value as the previous one.
fn restore_value(e: &Entity, c: &FieldChange) -> Result<FieldChange, UndoError> {
    use Entity as E;
    use FieldChange as F;
    macro_rules! back {
        ($var:ident, $cur:expr, $x:expr) => {
            F::$var(Change::new($cur.clone(), $x.old.clone()))
        };
    }
    Ok(match (c, e) {
        (F::Name(x), E::Collection(v)) => back!(Name, v.name, x),
        (F::Name(x), E::List(v)) => back!(Name, v.name, x),
        (F::Name(x), E::Tag(v)) => back!(Name, v.name, x),
        (F::Color(x), E::Collection(v)) => back!(Color, v.color, x),
        (F::Color(x), E::List(v)) => back!(Color, v.color, x),
        (F::Color(x), E::Tag(v)) => back!(Color, v.color, x),
        (F::Position(x), E::Collection(v)) => back!(Position, v.position, x),
        (F::Position(x), E::List(v)) => back!(Position, v.position, x),
        (F::Position(x), E::Task(v)) => back!(Position, v.position, x),
        (F::Position(x), E::Step(v)) => back!(Position, v.position, x),
        (F::Title(x), E::Task(v)) => back!(Title, v.title, x),
        (F::Title(x), E::Step(v)) => back!(Title, v.title, x),
        (F::Notes(x), E::Task(v)) => back!(Notes, v.notes, x),
        // Only an exact take-back of the added note stays append-only. If the notes changed since (a forced
        // undo), the compensation replaces someone else's notes, so it is an ordinary notes change and
        // reversing it needs plan access.
        (F::NoteAdded(x), E::Task(v)) if v.notes == x.new => back!(NoteAdded, v.notes, x),
        (F::NoteAdded(x), E::Task(v)) => back!(Notes, v.notes, x),
        (F::Due(x), E::Task(v)) => back!(Due, v.due, x),
        (F::StartDate(x), E::Task(v)) => back!(StartDate, v.start_date, x),
        (F::Status(x), E::Task(v)) => back!(Status, v.status, x),
        (F::ListFeatures(x), E::List(v)) => back!(ListFeatures, v.features, x),
        (F::Priority(x), E::Task(v)) => back!(Priority, v.priority, x),
        (F::MyDay(x), E::Task(v)) => back!(MyDay, v.my_day, x),
        (F::CompletedAt(x), E::Task(v)) => back!(CompletedAt, v.completed_at, x),
        (F::CompletedAt(x), E::Step(v)) => back!(CompletedAt, v.completed_at, x),
        (F::Recurrence(x), E::Task(v)) => back!(Recurrence, v.recurrence, x),
        (F::ListCollection(x), E::List(v)) => back!(ListCollection, v.collection, x),
        (F::TaskList(x), E::Task(v)) => back!(TaskList, v.list, x),
        (F::ParentTask(x), E::Step(v)) => back!(ParentTask, v.task, x),
        (F::ParentTask(x), E::Link(v)) => back!(ParentTask, v.task, x),
        (F::ParentTask(x), E::Attachment(v)) => back!(ParentTask, v.task, x),
        (F::ParentTask(x), E::Reminder(v)) => back!(ParentTask, v.task, x),
        (F::MergedInto(x), E::Task(v)) => back!(MergedInto, v.merged_into, x),
        (F::ReminderAt(x), E::Reminder(v)) => back!(ReminderAt, v.at, x),
        _ => {
            return Err(ApplyError::FieldMismatch {
                target: e.id(),
                field: c.name(),
            }
            .into());
        }
    })
}

fn require_exists(state: &State, id: EntityId) -> Result<(), UndoError> {
    if state.exists(id) {
        Ok(())
    } else {
        Err(UndoError::BrokenReference(id))
    }
}

/// Builds the payload that inverts `payload` (the payload of the applied event `event_id`, recorded as
/// `applied`).
fn invert(
    state: &State,
    event_id: EventId,
    applied: &Applied,
    payload: &EventPayload,
) -> Result<EventPayload, UndoError> {
    Ok(match payload {
        EventPayload::Created { entity } => {
            let id = entity.id();
            if let Some(dep) = state.dependents(id).first() {
                return Err(UndoError::HasDependents(*dep));
            }
            let current = state.entity(id).ok_or(UndoError::BrokenReference(id))?;
            EventPayload::Withdrawn { entity: current }
        }
        EventPayload::Withdrawn { entity } => {
            let id = entity.id();
            // Only the chain of the original creation may bring the ID back (IDs are never reused).
            let original = state.journal.creation_of(id);
            if state.exists(id) || original != Some(state.journal.root_of(event_id)) {
                return Err(ApplyError::AlreadyExists(id).into());
            }
            if let Some(p) = entity.parent() {
                require_exists(state, p)?;
            }
            if let Entity::Task(t) = entity {
                for tag in &t.tags {
                    require_exists(state, (*tag).into())?;
                }
                if let Some(m) = t.merged_into {
                    require_exists(state, m.into())?;
                }
            }
            EventPayload::Created {
                entity: entity.clone(),
            }
        }
        EventPayload::Updated { target, changes } => {
            let current = state
                .entity(*target)
                .ok_or(UndoError::BrokenReference(*target))?;
            let mut inverted = Vec::with_capacity(changes.len());
            for c in changes {
                match c {
                    // Only roll the last-modified stamp back if nobody touched the task since, i.e.
                    // the stamp in place is still the one this event left.
                    FieldChange::Touch(t) => {
                        let untouched = applied.touch.is_some_and(|(_, after)| {
                            state.journal.touch_tokens.get(target).copied() == after
                        });
                        if let Entity::Task(task) = &current
                            && untouched
                        {
                            inverted.push(FieldChange::Touch(Change::new(
                                task.updated.clone(),
                                t.old.clone(),
                            )));
                        }
                    }
                    // Put the membership back to what it was before the event (recorded at apply time).
                    FieldChange::TagAdded { tag } | FieldChange::TagRemoved { tag } => {
                        let was_set = applied
                            .write(&(*target, FieldKey::Tag(*tag)))
                            .and_then(|w| w.tag_was_set)
                            .unwrap_or(matches!(c, FieldChange::TagRemoved { .. }));
                        inverted.push(if was_set {
                            FieldChange::TagAdded { tag: *tag }
                        } else {
                            FieldChange::TagRemoved { tag: *tag }
                        });
                    }
                    other => inverted.push(restore_value(&current, other)?),
                }
            }
            check_refs(state, &inverted)?;
            EventPayload::Updated {
                target: *target,
                changes: inverted,
            }
        }
        // Put the tombstone back to what it was before the event (recorded at apply time), so undoing a
        // forced compensation restores the tombstone it actually replaced.
        EventPayload::Deleted { target, tombstone }
        | EventPayload::Restored { target, tombstone } => {
            let current = state
                .entity(*target)
                .ok_or(UndoError::BrokenReference(*target))?;
            match &applied.tombstone_before {
                None => EventPayload::Restored {
                    target: *target,
                    tombstone: current.deleted().unwrap_or(tombstone).clone(),
                },
                Some(t) => EventPayload::Deleted {
                    target: *target,
                    tombstone: t.clone(),
                },
            }
        }
    })
}

/// The fields (by conflict key) in which two values of the same entity differ, ignoring bookkeeping.
fn differing_fields(a: &Entity, b: &Entity) -> Vec<FieldKey> {
    use Entity as E;
    let mut out = Vec::new();
    let mut cmp = |differs: bool, key: FieldKey| {
        if differs {
            out.push(key);
        }
    };
    if a.deleted() != b.deleted() {
        cmp(true, FieldKey::Existence);
    }
    cmp(a.position() != b.position(), FieldKey::Position);
    cmp(a.parent() != b.parent(), FieldKey::Parent);
    match (a, b) {
        (E::Collection(x), E::Collection(y)) => {
            cmp(x.name != y.name, FieldKey::Name);
            cmp(x.color != y.color, FieldKey::Color);
        }
        (E::List(x), E::List(y)) => {
            cmp(x.name != y.name, FieldKey::Name);
            cmp(x.color != y.color, FieldKey::Color);
            cmp(x.features != y.features, FieldKey::ListFeatures);
        }
        (E::Tag(x), E::Tag(y)) => {
            cmp(x.name != y.name, FieldKey::Name);
            cmp(x.color != y.color, FieldKey::Color);
        }
        (E::Task(x), E::Task(y)) => {
            cmp(x.title != y.title, FieldKey::Title);
            cmp(x.notes != y.notes, FieldKey::Notes);
            cmp(x.due != y.due, FieldKey::Due);
            cmp(x.start_date != y.start_date, FieldKey::StartDate);
            cmp(x.status != y.status, FieldKey::Status);
            cmp(x.priority != y.priority, FieldKey::Priority);
            cmp(x.my_day != y.my_day, FieldKey::MyDay);
            cmp(x.completed_at != y.completed_at, FieldKey::CompletedAt);
            cmp(x.recurrence != y.recurrence, FieldKey::Recurrence);
            cmp(x.merged_into != y.merged_into, FieldKey::MergedInto);
            for t in x.tags.symmetric_difference(&y.tags) {
                cmp(true, FieldKey::Tag(*t));
            }
        }
        (E::Step(x), E::Step(y)) => {
            cmp(x.title != y.title, FieldKey::Title);
            cmp(x.completed_at != y.completed_at, FieldKey::CompletedAt);
        }
        (E::Reminder(x), E::Reminder(y)) => cmp(x.at != y.at, FieldKey::ReminderAt),
        _ => {}
    }
    out
}

/// Fields the compensation would overwrite although a later event changed them.
fn conflicts(
    state: &State,
    event: &Event,
    applied: &Applied,
    inverse: &EventPayload,
) -> Vec<FieldConflict> {
    let journal = &state.journal;
    let mut out: Vec<FieldConflict> = Vec::new();
    let mut add = |key: (EntityId, FieldKey), later: Vec<EventId>| {
        if !out.iter().any(|c| c.target == key.0 && c.field == key.1) {
            out.push(FieldConflict {
                target: key.0,
                field: key.1,
                later_events: later,
            });
        }
    };
    for key in inverse.keys() {
        let Some(w) = applied.write(&key) else {
            continue;
        };
        let current = journal.token(&key);
        if current != w.after {
            let mut later = journal.active_after(&key, event.id);
            if later.is_empty() {
                later.extend(current);
            }
            add(key, later);
        }
    }
    // Removing an entity (or hiding it again) touches every field: anything that changed since the event
    // being undone, by an ordinary or a compensating event, would be lost or hidden.
    let since = applied.order;
    match inverse {
        EventPayload::Withdrawn { entity } => {
            for (key, later) in journal.changed_since(entity.id(), since, false) {
                add(key, later);
            }
            // Also compare with the value the creation recorded, which catches compensating writes
            // (for example an earlier edit undone after a redo of this creation).
            if let EventPayload::Created { entity: created } = &event.payload {
                for field in differing_fields(entity, created) {
                    let key = (entity.id(), field);
                    let mut later = journal.active_after(&key, event.id);
                    if later.is_empty() {
                        later.extend(journal.token(&key));
                    }
                    add(key, later);
                }
            }
        }
        EventPayload::Deleted { target, .. } => {
            for (key, later) in journal.changed_since(*target, since, false) {
                add(key, later);
            }
            for d in state.descendants(*target) {
                if state.is_live(d) {
                    for (key, later) in journal.changed_since(d, since, true) {
                        add(key, later);
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// Refuses compensations that would leave an invalid entity.
fn validate_result(state: &State, inverse: &EventPayload) -> Result<(), UndoError> {
    let (target, entity) = match inverse {
        EventPayload::Created { entity } => (entity.id(), entity.clone()),
        EventPayload::Restored {
            target: EntityId::Tag(id),
            ..
        } => {
            let target = (*id).into();
            let tag = state
                .tags
                .get(id)
                .ok_or(UndoError::BrokenReference(target))?;
            return unique_tag_name(state, &tag.name, Some(*id))
                .map_err(|source| UndoError::Invariant { target, source });
        }
        EventPayload::Updated { target, changes } => {
            let mut e = state
                .entity(*target)
                .ok_or(UndoError::BrokenReference(*target))?;
            apply_changes(&mut e, changes)?;
            (*target, e)
        }
        _ => return Ok(()),
    };
    let invariant = |source| UndoError::Invariant { target, source };
    if let Entity::Tag(t) = &entity
        && t.deleted.is_none()
    {
        unique_tag_name(state, &t.name, Some(t.id)).map_err(invariant)?;
    }
    validate_entity(&entity).map_err(invariant)
}

/// The journal's record of `event`, after checking that `event` is exactly what was applied.
pub(crate) fn verified<'s>(state: &'s State, event: &Event) -> Result<&'s Applied, UndoError> {
    let applied = state
        .journal
        .applied
        .get(&event.id)
        .ok_or(UndoError::UnknownEvent(event.id))?;
    if applied.fingerprint != event.fingerprint() || applied.actor != event.actor {
        return Err(UndoError::EventMismatch(event.id));
    }
    Ok(applied)
}

/// The payload that would undo `event` in `state` (no permission or conflict checks).
pub(crate) fn inverse_of(state: &State, event: &Event) -> Result<EventPayload, UndoError> {
    let applied = verified(state, event)?;
    invert(state, event.id, applied, &event.payload)
}

/// True if `sibling` may have got its current position key, or its place among the siblings, after the
/// event at position `since` in apply order: it was created since, or its key, parent or tombstone
/// changed since.
fn placed_since(state: &State, sibling: EntityId, since: u64) -> bool {
    let journal = &state.journal;
    let created_since = journal
        .creation_of(sibling)
        .and_then(|e| journal.order_of(e))
        .is_none_or(|o| o > since);
    created_since
        || journal
            .changed_since(sibling, since, true)
            .iter()
            .any(|((_, f), _)| {
                matches!(
                    f,
                    FieldKey::Position | FieldKey::Parent | FieldKey::Existence
                )
            })
}

/// Keeps position keys unique when a compensation puts an entity (back) among siblings: it restores a
/// tombstone, redoes a creation, or changes the entity's key or parent. If a live sibling took the key the
/// entity would get since the undone event (`applied`), returns the position update that gives the
/// entity a fresh key just before that sibling, as [`Command::Restore`](crate::command::Command::Restore)
/// does. A tie that already existed at the time of the undone event is put back as it was.
///
/// The update is a separate event, so the compensation itself stays the exact inverse and later undos
/// and redos of it remain exact.
fn rekey_after(
    state: &State,
    applied: &Applied,
    inverse: &EventPayload,
) -> Result<Option<EventPayload>, UndoError> {
    let target = inverse.target();
    // The entity as the compensation leaves it, and whether it is then tombstoned itself.
    let (result, deleted) = match inverse {
        EventPayload::Created { entity } => (entity.clone(), entity.deleted().is_some()),
        EventPayload::Restored { .. } => (
            state
                .entity(target)
                .ok_or(UndoError::BrokenReference(target))?,
            false,
        ),
        EventPayload::Updated { changes, .. }
            if changes.iter().any(|c| {
                matches!(
                    c,
                    FieldChange::Position(_)
                        | FieldChange::TaskList(_)
                        | FieldChange::ListCollection(_)
                        | FieldChange::ParentTask(_)
                )
            }) =>
        {
            let mut e = state
                .entity(target)
                .ok_or(UndoError::BrokenReference(target))?;
            apply_changes(&mut e, changes)?;
            let deleted = e.deleted().is_some();
            (e, deleted)
        }
        _ => return Ok(None),
    };
    let (Some(scope), Some(key)) = (State::scope_of_entity(&result), result.position()) else {
        return Ok(None);
    };
    let taken_since = state
        .siblings(scope)
        .into_iter()
        .any(|(k, s)| s != target && &k == key && placed_since(state, s, applied.order));
    if deleted || !taken_since {
        return Ok(None);
    }
    let fresh = state
        .fresh_key_if_taken(scope, target, key)
        .map_err(|source| UndoError::Invariant { target, source })?;
    Ok(fresh.map(|fresh| EventPayload::Updated {
        target,
        changes: vec![FieldChange::Position(Change::new(key.clone(), fresh))],
    }))
}

/// The conflict reported for undoing a rekey without the compensation it belongs to.
fn rekey_conflict(rekey: &Event) -> UndoConflict {
    UndoConflict {
        event: rekey.id,
        conflicts: vec![FieldConflict {
            target: rekey.payload.target(),
            field: FieldKey::Position,
            later_events: vec![rekey.group],
        }],
    }
}

/// Builds the compensating events for `event`, failing on conflicts. See the [module docs](self).
pub fn undo(
    state: &State,
    event: &Event,
    actor: &Actor,
    now: Timestamp,
) -> Result<Vec<Event>, UndoError> {
    undo_with(state, event, actor, now, UndoOptions::default())
}

/// Like [`undo`], with options.
pub fn undo_with(
    state: &State,
    event: &Event,
    actor: &Actor,
    now: Timestamp,
    options: UndoOptions,
) -> Result<Vec<Event>, UndoError> {
    undo_one(state, event, actor, now, options, false)
}

fn undo_one(
    state: &State,
    event: &Event,
    actor: &Actor,
    now: Timestamp,
    options: UndoOptions,
    in_batch: bool,
) -> Result<Vec<Event>, UndoError> {
    let applied = verified(state, event)?;
    // A rekey only exists to keep keys unique next to its compensation. Taken back on its own it would
    // put the entity back on a key a sibling holds, so that is a position conflict: undo the whole group,
    // or force it to restore the tie deliberately.
    if !in_batch && !options.force && is_rekey(state, event) {
        return Err(rekey_conflict(event).into());
    }
    if !actor.is_owner() && &applied.actor != actor {
        return Err(UndoError::NotOwnEvent);
    }
    let journal = &state.journal;
    let root = journal.root_of(event.id);
    if journal.head_of(root) != event.id {
        return Err(UndoError::AlreadyUndone(event.id));
    }
    let payload = invert(state, event.id, applied, &event.payload)?;
    authorize_inverse(state, actor, event, &payload)?;
    let conflicts = conflicts(state, event, applied, &payload);
    if !conflicts.is_empty() && !options.force {
        return Err(UndoConflict {
            event: event.id,
            conflicts,
        }
        .into());
    }
    validate_result(state, &payload)?;
    let rekey = rekey_after(state, applied, &payload)?;
    let seed = format!("undo|{actor:?}|{}|{}", event.id, options.force);
    let id = EventId::derive(now, seed.as_bytes());
    let mut out = vec![Event {
        id,
        seq: None,
        actor: actor.clone(),
        at: now,
        group: id,
        undoes: Some(event.id),
        forced: !conflicts.is_empty(),
        payload,
    }];
    if let Some(payload) = rekey {
        out.push(Event {
            id: EventId::derive(now, format!("{seed}|rekey").as_bytes()),
            seq: None,
            actor: actor.clone(),
            at: now,
            group: id,
            undoes: None,
            forced: false,
            payload,
        });
    }
    Ok(out)
}

/// Undoes all the events of one command (for example a merge) as a unit, newest first.
///
/// The compensating events are returned in the order they must be applied. If any step fails, nothing is
/// returned.
pub fn undo_batch(
    state: &State,
    events: &[Event],
    actor: &Actor,
    now: Timestamp,
    options: UndoOptions,
) -> Result<Vec<Event>, UndoError> {
    // A rekey goes back together with the compensation it belongs to, unless forced (see `undo_one`).
    if !options.force {
        for e in events {
            if is_rekey(state, e) && !events.iter().any(|o| o.id == e.group) {
                return Err(rekey_conflict(e).into());
            }
        }
    }
    let mut sim = state.clone();
    let mut out = Vec::new();
    for e in events.iter().rev() {
        let comp = undo_one(&sim, e, actor, now, options, true)?;
        for c in &comp {
            apply(&mut sim, c)?;
        }
        out.extend(comp);
    }
    if let Some(first) = out.first().map(|e| e.id) {
        for e in &mut out {
            e.group = first;
        }
    }
    Ok(out)
}
