//! Events: the durable record of what changed.
//!
//! Every [`Event`] carries the actor, the time and a typed [`EventPayload`]. Updates record each changed
//! field with its previous value ([`Change`]), deletes record the tombstone, and creations record the
//! full entity, so every payload can be inverted without consulting anything else.

use crate::actor::Actor;
use crate::ids::{CollectionId, EntityId, EventId, ListId, TagId, TaskId};
use crate::model::{Entity, Priority, Stamp, Tombstone};
use crate::ordering::PosKey;
use crate::recurrence::Recurrence;
use crate::time::{Date, Timestamp};
use serde::{Deserialize, Serialize};

/// A field value before and after an update.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change<T> {
    /// Value before the event.
    pub old: T,
    /// Value after the event.
    pub new: T,
}

impl<T: Clone> Change<T> {
    /// Builds a change.
    pub fn new(old: T, new: T) -> Self {
        Self { old, new }
    }

    /// The change with `old` and `new` swapped.
    #[must_use]
    pub fn inverted(&self) -> Self {
        Self {
            old: self.new.clone(),
            new: self.old.clone(),
        }
    }
}

/// One changed field of an entity, with its previous value.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum FieldChange {
    /// Name of a collection, list or tag.
    Name(Change<String>),
    /// Colour of a collection, list or tag.
    Color(Change<Option<String>>),
    /// Sort key of a collection, list, task or step.
    Position(Change<PosKey>),
    /// Title of a task or step.
    Title(Change<String>),
    /// Notes of a task.
    Notes(Change<String>),
    /// Notes of a task, extended by appending a note (`AddNote`). Applies exactly like
    /// [`Notes`](Self::Notes); the distinct variant records that the change was append-only, so undoing
    /// it needs only capture access (see [`undo_requirements`](crate::access::undo_requirements)).
    /// Compensations in the undo/redo chain of such a change keep this variant.
    NoteAdded(Change<String>),
    /// Due date of a task.
    #[cfg_attr(feature = "openapi", schema(value_type = Change<Option<String>>))]
    Due(Change<Option<Date>>),
    /// Priority of a task.
    Priority(Change<Priority>),
    /// My Day date of a task.
    #[cfg_attr(feature = "openapi", schema(value_type = Change<Option<String>>))]
    MyDay(Change<Option<Date>>),
    /// Completion time of a task or step.
    #[cfg_attr(feature = "openapi", schema(value_type = Change<Option<String>>))]
    CompletedAt(Change<Option<Timestamp>>),
    /// Recurrence of a task.
    Recurrence(Change<Option<Recurrence>>),
    /// Collection of a list.
    ListCollection(Change<Option<CollectionId>>),
    /// List of a task.
    TaskList(Change<ListId>),
    /// Owning task of a step, link, attachment or reminder (used when merging tasks).
    ParentTask(Change<TaskId>),
    /// Task a task was merged into.
    MergedInto(Change<Option<TaskId>>),
    /// Time of a reminder.
    #[cfg_attr(feature = "openapi", schema(value_type = Change<String>))]
    ReminderAt(Change<Timestamp>),
    /// A tag was added to a task.
    TagAdded {
        /// The tag.
        tag: TagId,
    },
    /// A tag was removed from a task.
    TagRemoved {
        /// The tag.
        tag: TagId,
    },
    /// Last-modified stamp of a task. Bookkeeping: never counted as a conflict.
    Touch(Change<Stamp>),
}

/// Identifies a field for undo conflict detection. [`FieldChange::Touch`] has no key.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKey {
    /// The entity's existence and tombstone.
    Existence,
    /// `name`
    Name,
    /// `color`
    Color,
    /// `position`
    Position,
    /// `title`
    Title,
    /// `notes`
    Notes,
    /// `due`
    Due,
    /// `priority`
    Priority,
    /// `my_day`
    MyDay,
    /// `completed_at`
    CompletedAt,
    /// `recurrence`
    Recurrence,
    /// The entity's parent (collection of a list, list of a task, task of a step, ...).
    Parent,
    /// `merged_into`
    MergedInto,
    /// Time of a reminder.
    ReminderAt,
    /// Membership of one tag.
    Tag(TagId),
}

impl FieldChange {
    /// The conflict key of this change, or `None` for bookkeeping.
    #[must_use]
    pub fn key(&self) -> Option<FieldKey> {
        Some(match self {
            Self::Name(_) => FieldKey::Name,
            Self::Color(_) => FieldKey::Color,
            Self::Position(_) => FieldKey::Position,
            Self::Title(_) => FieldKey::Title,
            Self::Notes(_) | Self::NoteAdded(_) => FieldKey::Notes,
            Self::Due(_) => FieldKey::Due,
            Self::Priority(_) => FieldKey::Priority,
            Self::MyDay(_) => FieldKey::MyDay,
            Self::CompletedAt(_) => FieldKey::CompletedAt,
            Self::Recurrence(_) => FieldKey::Recurrence,
            Self::ListCollection(_) | Self::TaskList(_) | Self::ParentTask(_) => FieldKey::Parent,
            Self::MergedInto(_) => FieldKey::MergedInto,
            Self::ReminderAt(_) => FieldKey::ReminderAt,
            Self::TagAdded { tag } | Self::TagRemoved { tag } => FieldKey::Tag(*tag),
            Self::Touch(_) => return None,
        })
    }

    /// The field's name, for error messages.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Name(_) => "name",
            Self::Color(_) => "color",
            Self::Position(_) => "position",
            Self::Title(_) => "title",
            Self::Notes(_) | Self::NoteAdded(_) => "notes",
            Self::Due(_) => "due",
            Self::Priority(_) => "priority",
            Self::MyDay(_) => "my_day",
            Self::CompletedAt(_) => "completed_at",
            Self::Recurrence(_) => "recurrence",
            Self::ListCollection(_) => "collection",
            Self::TaskList(_) => "list",
            Self::ParentTask(_) => "task",
            Self::MergedInto(_) => "merged_into",
            Self::ReminderAt(_) => "at",
            Self::TagAdded { .. } | Self::TagRemoved { .. } => "tags",
            Self::Touch(_) => "updated",
        }
    }

    /// The exact inverse of this change.
    #[must_use]
    pub fn inverted(&self) -> Self {
        match self {
            Self::Name(c) => Self::Name(c.inverted()),
            Self::Color(c) => Self::Color(c.inverted()),
            Self::Position(c) => Self::Position(c.inverted()),
            Self::Title(c) => Self::Title(c.inverted()),
            Self::Notes(c) => Self::Notes(c.inverted()),
            Self::NoteAdded(c) => Self::NoteAdded(c.inverted()),
            Self::Due(c) => Self::Due(c.inverted()),
            Self::Priority(c) => Self::Priority(c.inverted()),
            Self::MyDay(c) => Self::MyDay(c.inverted()),
            Self::CompletedAt(c) => Self::CompletedAt(c.inverted()),
            Self::Recurrence(c) => Self::Recurrence(c.inverted()),
            Self::ListCollection(c) => Self::ListCollection(c.inverted()),
            Self::TaskList(c) => Self::TaskList(c.inverted()),
            Self::ParentTask(c) => Self::ParentTask(c.inverted()),
            Self::MergedInto(c) => Self::MergedInto(c.inverted()),
            Self::ReminderAt(c) => Self::ReminderAt(c.inverted()),
            Self::TagAdded { tag } => Self::TagRemoved { tag: *tag },
            Self::TagRemoved { tag } => Self::TagAdded { tag: *tag },
            Self::Touch(c) => Self::Touch(c.inverted()),
        }
    }
}

/// What an event did.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum EventPayload {
    /// An entity was created. Records the full entity.
    Created {
        /// The new entity.
        entity: Entity,
    },
    /// Fields of an entity changed.
    Updated {
        /// The entity.
        target: EntityId,
        /// The changed fields with their previous values.
        changes: Vec<FieldChange>,
    },
    /// An entity was tombstoned.
    Deleted {
        /// The entity.
        target: EntityId,
        /// The tombstone that was set.
        tombstone: Tombstone,
    },
    /// A tombstone was removed.
    Restored {
        /// The entity.
        target: EntityId,
        /// The tombstone that was removed.
        tombstone: Tombstone,
    },
    /// A creation was undone: the entity was removed. Records its last state so the creation can be
    /// redone. Only produced by undo.
    Withdrawn {
        /// The entity as it was when withdrawn.
        entity: Entity,
    },
}

impl EventPayload {
    /// The entity this payload targets.
    #[must_use]
    pub fn target(&self) -> EntityId {
        match self {
            Self::Created { entity } | Self::Withdrawn { entity } => entity.id(),
            Self::Updated { target, .. }
            | Self::Deleted { target, .. }
            | Self::Restored { target, .. } => *target,
        }
    }

    /// The fields this payload touches, for conflict detection.
    #[must_use]
    pub fn keys(&self) -> Vec<(EntityId, FieldKey)> {
        let target = self.target();
        match self {
            Self::Updated { changes, .. } => changes
                .iter()
                .filter_map(FieldChange::key)
                .map(|k| (target, k))
                .collect(),
            _ => vec![(target, FieldKey::Existence)],
        }
    }
}

/// A recorded change.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Identifier, derived deterministically from the command so retries are idempotent.
    pub id: EventId,
    /// Position in the server's log; `None` until the server assigns it.
    pub seq: Option<u64>,
    /// Who caused it.
    pub actor: Actor,
    /// When.
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = DateTime))]
    pub at: Timestamp,
    /// The first event produced by the same command; used to undo a command as a unit.
    pub group: EventId,
    /// For a compensating event produced by undo: the event it undoes.
    pub undoes: Option<EventId>,
    /// True if an undo overrode a detected conflict.
    pub forced: bool,
    /// What happened.
    pub payload: EventPayload,
}

/// The parts of an [`Event`] covered by its fingerprint: everything except the server-assigned `seq`.
#[derive(Serialize)]
struct Fingerprinted<'a> {
    id: &'a EventId,
    actor: &'a Actor,
    at: &'a Timestamp,
    group: &'a EventId,
    undoes: &'a Option<EventId>,
    forced: bool,
    payload: &'a EventPayload,
}

impl Event {
    /// SHA-256 of the event's canonical serialization, ignoring `seq` (which the server assigns after the
    /// event was decided). Two events with the same fingerprint are the same event.
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        crate::fingerprint::fingerprint(&Fingerprinted {
            id: &self.id,
            actor: &self.actor,
            at: &self.at,
            group: &self.group,
            undoes: &self.undoes,
            forced: self.forced,
            payload: &self.payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inversion_is_an_involution() {
        let c = FieldChange::Title(Change::new("a".into(), "b".into()));
        assert_eq!(c.inverted().inverted(), c);
        let t = TagId::from_uuid(uuid::Uuid::nil());
        assert_eq!(
            FieldChange::TagAdded { tag: t }.inverted(),
            FieldChange::TagRemoved { tag: t }
        );
    }

    #[test]
    fn touch_has_no_key() {
        let s = Stamp {
            at: Timestamp::UNIX_EPOCH,
            by: Actor::System,
        };
        assert_eq!(FieldChange::Touch(Change::new(s.clone(), s)).key(), None);
    }
}
