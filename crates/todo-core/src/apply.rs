//! Applying events to the state.
//!
//! [`apply`] is deterministic: the same events applied to the same state always give the same result, so
//! the server and every client converge by replaying the log. It is also atomic: if an event cannot be
//! applied, the state is left untouched.

use crate::error::ApplyError;
use crate::event::{Event, EventPayload, FieldChange, FieldKey};
use crate::ids::{EntityId, TagId};
use crate::model::{
    AttachmentRef, Collection, Entity, Link, List, Reminder, Step, Tag, Task, Tombstone,
};
use crate::state::{Applied, FieldWrite, State};

fn mismatch(target: EntityId, c: &FieldChange) -> ApplyError {
    ApplyError::FieldMismatch {
        target,
        field: c.name(),
    }
}

fn collection(e: &mut Collection, c: &FieldChange) -> bool {
    match c {
        FieldChange::Name(c) => e.name.clone_from(&c.new),
        FieldChange::Color(c) => e.color.clone_from(&c.new),
        FieldChange::Position(c) => e.position.clone_from(&c.new),
        FieldChange::TaskSort(c) => e.task_sort = c.new,
        _ => return false,
    }
    true
}

fn list(e: &mut List, c: &FieldChange) -> bool {
    match c {
        FieldChange::Name(c) => e.name.clone_from(&c.new),
        FieldChange::Color(c) => e.color.clone_from(&c.new),
        FieldChange::Position(c) => e.position.clone_from(&c.new),
        FieldChange::ListCollection(c) => e.collection = c.new,
        FieldChange::ListFeatures(c) => e.features = c.new,
        FieldChange::TaskSort(c) => e.task_sort = c.new,
        _ => return false,
    }
    true
}

fn task(e: &mut Task, c: &FieldChange) -> bool {
    match c {
        FieldChange::Title(c) => e.title.clone_from(&c.new),
        FieldChange::Notes(c) | FieldChange::NoteAdded(c) => e.notes.clone_from(&c.new),
        FieldChange::Due(c) => e.due = c.new,
        FieldChange::StartDate(c) => e.start_date = c.new,
        FieldChange::Status(c) => e.status = c.new,
        FieldChange::Priority(c) => e.priority = c.new,
        FieldChange::MyDay(c) => e.my_day = c.new,
        FieldChange::CompletedAt(c) => e.completed_at = c.new,
        FieldChange::Recurrence(c) => e.recurrence.clone_from(&c.new),
        FieldChange::TaskList(c) => e.list = c.new,
        FieldChange::Position(c) => e.position.clone_from(&c.new),
        FieldChange::MergedInto(c) => e.merged_into = c.new,
        FieldChange::TagAdded { tag: t } => {
            e.tags.insert(*t);
        }
        FieldChange::TagRemoved { tag: t } => {
            e.tags.remove(t);
        }
        FieldChange::Touch(c) => e.updated.clone_from(&c.new),
        _ => return false,
    }
    true
}

fn step(e: &mut Step, c: &FieldChange) -> bool {
    match c {
        FieldChange::Title(c) => e.title.clone_from(&c.new),
        FieldChange::CompletedAt(c) => e.completed_at = c.new,
        FieldChange::Position(c) => e.position.clone_from(&c.new),
        FieldChange::ParentTask(c) => e.task = c.new,
        _ => return false,
    }
    true
}

fn tag(e: &mut Tag, c: &FieldChange) -> bool {
    match c {
        FieldChange::Name(c) => e.name.clone_from(&c.new),
        FieldChange::Color(c) => e.color.clone_from(&c.new),
        _ => return false,
    }
    true
}

fn link(e: &mut Link, c: &FieldChange) -> bool {
    match c {
        FieldChange::ParentTask(c) => e.task = c.new,
        _ => return false,
    }
    true
}

fn reminder(e: &mut Reminder, c: &FieldChange) -> bool {
    match c {
        FieldChange::ReminderAt(c) => e.at = c.new,
        FieldChange::ParentTask(c) => e.task = c.new,
        _ => return false,
    }
    true
}

fn attachment(e: &mut AttachmentRef, c: &FieldChange) -> bool {
    match c {
        FieldChange::ParentTask(c) => e.task = c.new,
        _ => return false,
    }
    true
}

/// Applies changes to an entity value, failing on the first field that does not belong to its kind.
pub(crate) fn apply_changes(e: &mut Entity, changes: &[FieldChange]) -> Result<(), ApplyError> {
    let target = e.id();
    for c in changes {
        let ok = match e {
            Entity::Collection(x) => collection(x, c),
            Entity::List(x) => list(x, c),
            Entity::Task(x) => task(x, c),
            Entity::Step(x) => step(x, c),
            Entity::Tag(x) => tag(x, c),
            Entity::Link(x) => link(x, c),
            Entity::Attachment(x) => attachment(x, c),
            Entity::Reminder(x) => reminder(x, c),
        };
        if !ok {
            return Err(mismatch(target, c));
        }
    }
    Ok(())
}

/// Applies all changes to a copy of the entity, then stores it, so a bad change leaves no trace.
fn update(state: &mut State, target: EntityId, changes: &[FieldChange]) -> Result<(), ApplyError> {
    let mut e = state.entity(target).ok_or(ApplyError::NotFound(target))?;
    apply_changes(&mut e, changes)?;
    state.replace(e)
}

/// Tag memberships of the target task before an update, for the tags the update touches.
fn tags_before(state: &State, payload: &EventPayload) -> Vec<(TagId, bool)> {
    let EventPayload::Updated {
        target: EntityId::Task(t),
        changes,
    } = payload
    else {
        return Vec::new();
    };
    let current = state.tasks.get(t);
    changes
        .iter()
        .filter_map(|c| match c {
            FieldChange::TagAdded { tag } | FieldChange::TagRemoved { tag } => {
                Some((*tag, current.is_some_and(|x| x.tags.contains(tag))))
            }
            _ => None,
        })
        .collect()
}

/// Records an applied event in the journal: its fingerprint, the fields it wrote and their provenance.
fn record(
    state: &mut State,
    event: &Event,
    tag_was: &[(TagId, bool)],
    tombstone_before: Option<Tombstone>,
) {
    let journal = &mut state.journal;
    let undone_writes: Vec<FieldWrite> = event
        .undoes
        .and_then(|t| journal.applied.get(&t))
        .map(|a| a.writes.clone())
        .unwrap_or_default();
    if let (None, EventPayload::Created { entity }) = (event.undoes, &event.payload) {
        journal.created.insert(entity.id(), event.id);
    }
    let undone_touch = event
        .undoes
        .and_then(|t| journal.applied.get(&t))
        .and_then(|a| a.touch);
    let touch = match &event.payload {
        EventPayload::Updated { target, changes }
            if changes.iter().any(|c| matches!(c, FieldChange::Touch(_))) =>
        {
            let before = journal.touch_tokens.get(target).copied();
            let after = match event.undoes {
                None => Some(event.id),
                Some(_) => undone_touch.map_or(Some(event.id), |(b, _)| b),
            };
            match after {
                Some(a) => journal.touch_tokens.insert(*target, a),
                None => journal.touch_tokens.remove(target),
            };
            Some((before, after))
        }
        _ => None,
    };
    let mut writes = Vec::new();
    for key in event.payload.keys() {
        let before = journal.token(&key);
        // An undo writes back the value the undone event replaced, so it also restores its provenance.
        let after = match event.undoes {
            None => Some(event.id),
            Some(_) => undone_writes
                .iter()
                .find(|w| w.key == key)
                .map_or(Some(event.id), |w| w.before),
        };
        match after {
            Some(a) => journal.tokens.insert(key.clone(), a),
            None => journal.tokens.remove(&key),
        };
        journal
            .field_log
            .entry(key.clone())
            .or_default()
            .push(event.id);
        let tag_was_set = match key.1 {
            FieldKey::Tag(g) => tag_was.iter().find(|(t, _)| *t == g).map(|x| x.1),
            _ => None,
        };
        writes.push(FieldWrite {
            key,
            before,
            after,
            tag_was_set,
        });
    }
    if let Some(target) = event.undoes {
        let root = journal.root_of(target);
        if !journal.undone.remove(&root) {
            journal.undone.insert(root);
        }
        journal.origin.insert(event.id, target);
        journal.heads.insert(root, event.id);
    }
    let order = journal.next_order;
    journal.next_order += 1;
    journal.applied.insert(
        event.id,
        Applied {
            order,
            actor: event.actor.clone(),
            fingerprint: event.fingerprint(),
            writes,
            touch,
            tombstone_before,
        },
    );
}

/// A creation may only bring an ID to life once; after that, only the undo/redo chain of that original
/// creation may re-create it (redo of a withdrawn creation).
fn check_creation(state: &State, event: &Event, id: EntityId) -> Result<(), ApplyError> {
    let journal = &state.journal;
    let ok = match (event.undoes, journal.creation_of(id)) {
        (None, None) => true,
        (Some(undone), Some(original)) => journal.root_of(undone) == original,
        (None, Some(_)) | (Some(_), None) => false,
    };
    if ok {
        Ok(())
    } else {
        Err(ApplyError::AlreadyExists(id))
    }
}

/// Applies one event to the state.
///
/// Fails, leaving the state unchanged, if the event was already applied, targets a missing entity, creates
/// an existing one or reuses the ID of an earlier entity, or carries a field that does not belong to the
/// target. Events produced by [`decide`](crate::decide::decide) and [`undo`](crate::undo::undo) always
/// apply to the state they were derived from.
pub fn apply(state: &mut State, event: &Event) -> Result<(), ApplyError> {
    if state.journal.has_seen(event.id) {
        return Err(ApplyError::DuplicateEvent(event.id));
    }
    if let EventPayload::Created { entity } = &event.payload {
        check_creation(state, event, entity.id())?;
    }
    let tag_was = tags_before(state, &event.payload);
    let tombstone_before = match &event.payload {
        EventPayload::Deleted { target, .. } | EventPayload::Restored { target, .. } => {
            state.tombstone(*target)
        }
        _ => None,
    };
    match &event.payload {
        EventPayload::Created { entity } => state.insert(entity.clone())?,
        EventPayload::Withdrawn { entity } => state.remove(entity.id())?,
        EventPayload::Updated { target, changes } => update(state, *target, changes)?,
        EventPayload::Deleted { target, tombstone } => {
            state.set_tombstone(*target, Some(tombstone.clone()))?;
        }
        EventPayload::Restored { target, .. } => state.set_tombstone(*target, None)?,
    }
    record(state, event, &tag_was, tombstone_before);
    Ok(())
}

/// Applies events in order, stopping at the first failure.
pub fn apply_all<'a>(
    state: &mut State,
    events: impl IntoIterator<Item = &'a Event>,
) -> Result<(), ApplyError> {
    for e in events {
        apply(state, e)?;
    }
    Ok(())
}
