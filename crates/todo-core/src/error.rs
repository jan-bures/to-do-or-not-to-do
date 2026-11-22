//! Error types for validation, apply and undo.

use crate::access::AccessLevel;
use crate::event::FieldKey;
use crate::ids::{EntityId, EventId};
use crate::ordering::OrderError;
use crate::recurrence::RecurrenceError;

/// The actor lacks the access level a command needs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("access denied: needs {needed:?} access (owner only: {owner_only}) but has {have:?}")]
pub struct AccessDenied {
    /// The level the command needs.
    pub needed: AccessLevel,
    /// The level the actor has on the target.
    pub have: AccessLevel,
    /// True if the command is reserved for the owner regardless of level.
    pub owner_only: bool,
}

/// A command was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// The actor may not do this.
    #[error(transparent)]
    Denied(#[from] AccessDenied),
    /// The entity does not exist.
    #[error("{0} not found")]
    NotFound(EntityId),
    /// The entity is deleted (or lives under a deleted parent).
    #[error("{0} is deleted")]
    Deleted(EntityId),
    /// Restore was requested for an entity that is not deleted.
    #[error("{0} is not deleted")]
    NotDeleted(EntityId),
    /// Delete was requested for an entity that is already deleted.
    #[error("{0} is already deleted")]
    AlreadyDeleted(EntityId),
    /// A create command used an ID that is taken.
    #[error("{0} already exists")]
    AlreadyExists(EntityId),
    /// A field value is not acceptable.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// The offending field.
        field: &'static str,
        /// What is wrong.
        reason: String,
    },
    /// A guarded move's task is no longer in the expected list or has been merged.
    #[error("the task was moved or merged elsewhere")]
    MoveSourceChanged,
    /// A tag with this name already exists.
    #[error("a tag named `{0}` already exists")]
    DuplicateName(String),
    /// The sibling named in a placement is not in the same list of siblings.
    #[error("placement refers to {0}, which is not a sibling")]
    BadPlacement(EntityId),
    /// Source and target of a merge (or similar) are the same.
    #[error("operation needs two different entities")]
    SameEntity,
    /// The recurrence rule is invalid.
    #[error(transparent)]
    Recurrence(#[from] RecurrenceError),
    /// Position key generation failed.
    #[error(transparent)]
    Ordering(#[from] OrderError),
}

impl DomainError {
    pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            field,
            reason: reason.into(),
        }
    }
}

/// An event could not be applied to the state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    /// An event with this ID was already applied.
    #[error("event {0} was already applied")]
    DuplicateEvent(EventId),
    /// The target entity does not exist.
    #[error("{0} not found")]
    NotFound(EntityId),
    /// A created entity already exists.
    #[error("{0} already exists")]
    AlreadyExists(EntityId),
    /// A field change does not belong to the target's entity kind.
    #[error("field `{field}` does not apply to {target}")]
    FieldMismatch {
        /// The target.
        target: EntityId,
        /// The field name.
        field: &'static str,
    },
}

/// A later event changed a field that the undo would overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldConflict {
    /// The entity.
    pub target: EntityId,
    /// The field.
    pub field: FieldKey,
    /// The later, still-active events that changed it, oldest first.
    pub later_events: Vec<EventId>,
}

/// Everything that blocks an undo; pass `force` to override.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{} field(s) were changed by later events", .conflicts.len())]
pub struct UndoConflict {
    /// The event that was to be undone.
    pub event: EventId,
    /// The conflicting fields.
    pub conflicts: Vec<FieldConflict>,
}

/// An undo was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UndoError {
    /// The actor may not undo this event.
    #[error(transparent)]
    Denied(#[from] AccessDenied),
    /// An agent tried to undo an event it did not cause.
    #[error("agents may only undo their own events")]
    NotOwnEvent,
    /// The event was never applied to this state.
    #[error("event {0} is unknown")]
    UnknownEvent(EventId),
    /// An event with this ID was applied, but the supplied event differs from it (actor, payload or any
    /// other field except `seq`).
    #[error("event {0} does not match the applied event with that ID")]
    EventMismatch(EventId),
    /// The event is not the latest word on its change: it is already undone, or a redo of it exists.
    #[error("event {0} is already undone")]
    AlreadyUndone(EventId),
    /// Later events changed the same fields.
    #[error(transparent)]
    Conflict(#[from] UndoConflict),
    /// Undoing a creation would orphan other entities that still exist.
    #[error("{0} still has dependents and cannot be withdrawn")]
    HasDependents(EntityId),
    /// The compensating change would point at an entity that no longer exists (its creation was undone).
    #[error("undo would reference missing {0}")]
    BrokenReference(EntityId),
    /// The compensating event would leave the entity invalid (for example a recurring task without a due
    /// date, because the due date was set and the recurrence added by separate events). Not overridable
    /// with `force`: undo the dependent change first.
    #[error("undo would leave {target} invalid: {source}")]
    Invariant {
        /// The entity that would become invalid.
        target: EntityId,
        /// The violated rule, as [`decide`](crate::decide::decide) reports it.
        source: DomainError,
    },
    /// The compensating event could not be applied.
    #[error(transparent)]
    Apply(#[from] ApplyError),
}
