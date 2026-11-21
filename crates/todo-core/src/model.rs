//! Entities of the domain: collections, lists, tasks, steps, tags, links, attachments and reminders.
//!
//! Deleting never removes data: an entity gets a [`Tombstone`] and can be restored. Entities that hang
//! off a parent (tasks in a list, steps in a task, ...) are hidden when the parent is tombstoned, but are
//! not tombstoned themselves, so undoing the parent's delete restores everything with one event.

use crate::actor::Actor;
use crate::ids::{
    AttachmentId, CollectionId, EntityId, LinkId, ListId, ReminderId, StepId, TagId, TaskId,
};
use crate::ordering::PosKey;
use crate::recurrence::Recurrence;
use crate::time::{Date, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// When and by whom something happened.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Stamp {
    /// The time.
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = DateTime))]
    pub at: Timestamp,
    /// The actor.
    pub by: Actor,
}

/// Marks an entity as deleted.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tombstone {
    /// When it was deleted.
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = DateTime))]
    pub at: Timestamp,
    /// Who deleted it.
    pub by: Actor,
}

/// Task priority. Ordered `None < Low < Normal < High`.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// No priority set.
    #[default]
    None,
    /// Low.
    Low,
    /// Normal.
    Normal,
    /// High (the "Important" smart list).
    High,
}

impl Priority {
    /// Lowercase name used in the query language.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }

    /// Parses a lowercase name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        [Self::None, Self::Low, Self::Normal, Self::High]
            .into_iter()
            .find(|p| p.name() == s)
    }
}

/// A group of lists (Microsoft To Do: "list group").
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    /// Identifier.
    pub id: CollectionId,
    /// Display name.
    pub name: String,
    /// Optional colour, for example `#3366ff`.
    pub color: Option<String>,
    /// Sort key among collections.
    pub position: PosKey,
    /// Creation stamp.
    pub created: Stamp,
    /// Set while deleted.
    pub deleted: Option<Tombstone>,
}

/// A list of tasks, optionally inside a collection.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct List {
    /// Identifier.
    pub id: ListId,
    /// Parent collection, or `None` for a top-level list.
    pub collection: Option<CollectionId>,
    /// Display name.
    pub name: String,
    /// Optional colour.
    pub color: Option<String>,
    /// Sort key among the lists of the same collection.
    pub position: PosKey,
    /// Creation stamp.
    pub created: Stamp,
    /// Set while deleted.
    pub deleted: Option<Tombstone>,
}

/// A task.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// Identifier.
    pub id: TaskId,
    /// The list it belongs to.
    pub list: ListId,
    /// Title.
    pub title: String,
    /// Notes (Markdown).
    pub notes: String,
    /// Due date.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
    pub due: Option<Date>,
    /// Priority.
    pub priority: Priority,
    /// The date this task is in My Day; it counts as My Day only while that date is today.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
    pub my_day: Option<Date>,
    /// Completion time; `None` while open.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = DateTime))]
    pub completed_at: Option<Timestamp>,
    /// Repeat rule; requires a due date.
    pub recurrence: Option<Recurrence>,
    /// Tags.
    pub tags: BTreeSet<TagId>,
    /// Sort key among the tasks of the list.
    pub position: PosKey,
    /// Creation stamp.
    pub created: Stamp,
    /// Last modification stamp (updated by task-level events, ignored by undo conflict detection).
    pub updated: Stamp,
    /// Set while deleted.
    pub deleted: Option<Tombstone>,
    /// Set when this task was merged into another one.
    pub merged_into: Option<TaskId>,
}

impl Task {
    /// True when the task has been completed.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.completed_at.is_some()
    }
}

/// A checklist step of a task.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Identifier.
    pub id: StepId,
    /// The task it belongs to.
    pub task: TaskId,
    /// Title.
    pub title: String,
    /// Completion time; `None` while open.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = DateTime))]
    pub completed_at: Option<Timestamp>,
    /// Sort key among the steps of the task.
    pub position: PosKey,
    /// Creation stamp.
    pub created: Stamp,
    /// Set while deleted.
    pub deleted: Option<Tombstone>,
}

/// A tag (Microsoft To Do: "category").
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    /// Identifier.
    pub id: TagId,
    /// Name, unique (case-insensitively) among live tags.
    pub name: String,
    /// Optional colour.
    pub color: Option<String>,
    /// Creation stamp.
    pub created: Stamp,
    /// Set while deleted.
    pub deleted: Option<Tombstone>,
}

/// Metadata of an email a task was created from. Never contains the message body.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EmailLink {
    /// Subject line.
    pub subject: String,
    /// Sender address or display name.
    pub sender: String,
    /// When the message was received.
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = DateTime))]
    pub received_at: Timestamp,
    /// RFC 5322 `Message-ID`.
    pub message_id: String,
    /// Optional link that opens the message in a web mail client.
    pub web_link: Option<String>,
}

/// What a link points at.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LinkKind {
    /// A web page.
    Web {
        /// The `http` or `https` URL.
        url: String,
        /// Optional display title.
        title: Option<String>,
    },
    /// An email.
    Email(EmailLink),
}

/// A source link of a task.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// Identifier.
    pub id: LinkId,
    /// The task it belongs to.
    pub task: TaskId,
    /// Target.
    pub kind: LinkKind,
    /// Creation stamp.
    pub created: Stamp,
    /// Set while deleted.
    pub deleted: Option<Tombstone>,
}

/// Reference to an attachment stored elsewhere: metadata and a storage key, never the bytes.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRef {
    /// Identifier.
    pub id: AttachmentId,
    /// The task it belongs to.
    pub task: TaskId,
    /// File name without directories.
    pub file_name: String,
    /// MIME type.
    pub media_type: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Opaque key in the blob store.
    pub storage_key: String,
    /// Creation stamp.
    pub created: Stamp,
    /// Set while deleted.
    pub deleted: Option<Tombstone>,
}

/// A reminder at an absolute time.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    /// Identifier.
    pub id: ReminderId,
    /// The task it belongs to.
    pub task: TaskId,
    /// When to fire.
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = DateTime))]
    pub at: Timestamp,
    /// Creation stamp.
    pub created: Stamp,
    /// Set while deleted.
    pub deleted: Option<Tombstone>,
}

/// Any entity, by value.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Entity {
    /// A collection.
    Collection(Collection),
    /// A list.
    List(List),
    /// A task.
    Task(Task),
    /// A step.
    Step(Step),
    /// A tag.
    Tag(Tag),
    /// A link.
    Link(Link),
    /// An attachment reference.
    Attachment(AttachmentRef),
    /// A reminder.
    Reminder(Reminder),
}

impl Entity {
    /// The entity's identifier.
    #[must_use]
    pub fn id(&self) -> EntityId {
        match self {
            Self::Collection(e) => e.id.into(),
            Self::List(e) => e.id.into(),
            Self::Task(e) => e.id.into(),
            Self::Step(e) => e.id.into(),
            Self::Tag(e) => e.id.into(),
            Self::Link(e) => e.id.into(),
            Self::Attachment(e) => e.id.into(),
            Self::Reminder(e) => e.id.into(),
        }
    }

    /// The entity's creation stamp.
    #[must_use]
    pub fn created(&self) -> &Stamp {
        match self {
            Self::Collection(e) => &e.created,
            Self::List(e) => &e.created,
            Self::Task(e) => &e.created,
            Self::Step(e) => &e.created,
            Self::Tag(e) => &e.created,
            Self::Link(e) => &e.created,
            Self::Attachment(e) => &e.created,
            Self::Reminder(e) => &e.created,
        }
    }

    /// The tombstone, if deleted.
    #[must_use]
    pub fn deleted(&self) -> Option<&Tombstone> {
        match self {
            Self::Collection(e) => e.deleted.as_ref(),
            Self::List(e) => e.deleted.as_ref(),
            Self::Task(e) => e.deleted.as_ref(),
            Self::Step(e) => e.deleted.as_ref(),
            Self::Tag(e) => e.deleted.as_ref(),
            Self::Link(e) => e.deleted.as_ref(),
            Self::Attachment(e) => e.deleted.as_ref(),
            Self::Reminder(e) => e.deleted.as_ref(),
        }
    }

    /// The sort key, for the kinds that are ordered among siblings.
    #[must_use]
    pub fn position(&self) -> Option<&PosKey> {
        match self {
            Self::Collection(e) => Some(&e.position),
            Self::List(e) => Some(&e.position),
            Self::Task(e) => Some(&e.position),
            Self::Step(e) => Some(&e.position),
            Self::Tag(_) | Self::Link(_) | Self::Attachment(_) | Self::Reminder(_) => None,
        }
    }

    /// The entity that must exist for this one to exist, if any.
    #[must_use]
    pub fn parent(&self) -> Option<EntityId> {
        match self {
            Self::Collection(_) | Self::Tag(_) => None,
            Self::List(e) => e.collection.map(EntityId::Collection),
            Self::Task(e) => Some(EntityId::List(e.list)),
            Self::Step(e) => Some(EntityId::Task(e.task)),
            Self::Link(e) => Some(EntityId::Task(e.task)),
            Self::Attachment(e) => Some(EntityId::Task(e.task)),
            Self::Reminder(e) => Some(EntityId::Task(e.task)),
        }
    }
}
