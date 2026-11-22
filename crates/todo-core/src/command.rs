//! Commands: requests to change the state.
//!
//! A client generates the IDs of the entities it creates, so a retried command is recognisable. The
//! server validates a command with [`decide`](crate::decide::decide) and stores the resulting events.

use crate::ids::{
    AttachmentId, CollectionId, EntityId, LinkId, ListId, ReminderId, StepId, TagId, TaskId,
};
use crate::model::{Entity, LinkKind, Priority};
use crate::recurrence::Recurrence;
use crate::time::{Date, Timestamp};
use serde::{Deserialize, Serialize};

fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Where to put an item among its siblings.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", content = "sibling", rename_all = "snake_case")]
pub enum Placement {
    /// Before every sibling.
    First,
    /// After every sibling.
    #[default]
    Last,
    /// Immediately before the given sibling.
    Before(EntityId),
    /// Immediately after the given sibling.
    After(EntityId),
}

/// Fields of a new task.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    /// ID chosen by the client.
    pub id: TaskId,
    /// The list to create it in.
    pub list: ListId,
    /// Title (required).
    pub title: String,
    /// Notes.
    #[serde(default)]
    pub notes: String,
    /// Due date.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
    pub due: Option<Date>,
    /// Priority.
    #[serde(default)]
    pub priority: Priority,
    /// My Day date.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
    pub my_day: Option<Date>,
    /// Repeat rule (needs a due date).
    #[serde(default)]
    pub recurrence: Option<Recurrence>,
    /// Existing tags to attach.
    #[serde(default)]
    pub tags: Vec<TagId>,
    /// Position in the list.
    #[serde(default)]
    pub placement: Placement,
}

impl NewTask {
    /// A task with only an ID, list and title; everything else defaulted.
    #[must_use]
    pub fn new(id: TaskId, list: ListId, title: impl Into<String>) -> Self {
        Self {
            id,
            list,
            title: title.into(),
            notes: String::new(),
            due: None,
            priority: Priority::None,
            my_day: None,
            recurrence: None,
            tags: Vec::new(),
            placement: Placement::Last,
        }
    }
}

/// A partial update of a task. `None` leaves a field alone; `Some(None)` clears an optional field.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPatch {
    /// New title.
    #[serde(default)]
    pub title: Option<String>,
    /// New notes.
    #[serde(default)]
    pub notes: Option<String>,
    /// New due date, or `Some(None)` to clear.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
    pub due: Option<Option<Date>>,
    /// New priority.
    #[serde(default)]
    pub priority: Option<Priority>,
    /// New recurrence, or `Some(None)` to clear.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub recurrence: Option<Option<Recurrence>>,
}

/// A request to change the state.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Create a collection.
    CreateCollection {
        /// New ID.
        id: CollectionId,
        /// Name.
        name: String,
        /// Colour.
        #[serde(default)]
        color: Option<String>,
        /// Position among collections.
        #[serde(default)]
        placement: Placement,
    },
    /// Rename or recolour a collection.
    UpdateCollection {
        /// The collection.
        id: CollectionId,
        /// New name.
        #[serde(default)]
        name: Option<String>,
        /// New colour, or `Some(None)` to clear.
        #[serde(
            default,
            deserialize_with = "double_option",
            skip_serializing_if = "Option::is_none"
        )]
        color: Option<Option<String>>,
    },
    /// Create a list.
    CreateList {
        /// New ID.
        id: ListId,
        /// Parent collection, or `None` for top level.
        #[serde(default)]
        collection: Option<CollectionId>,
        /// Name.
        name: String,
        /// Colour.
        #[serde(default)]
        color: Option<String>,
        /// Position among the siblings.
        #[serde(default)]
        placement: Placement,
    },
    /// Rename or recolour a list.
    UpdateList {
        /// The list.
        id: ListId,
        /// New name.
        #[serde(default)]
        name: Option<String>,
        /// New colour, or `Some(None)` to clear.
        #[serde(
            default,
            deserialize_with = "double_option",
            skip_serializing_if = "Option::is_none"
        )]
        color: Option<Option<String>>,
    },
    /// Move a list into another collection (or to the top level).
    MoveList {
        /// The list.
        id: ListId,
        /// Destination collection.
        collection: Option<CollectionId>,
        /// Position among the new siblings.
        #[serde(default)]
        placement: Placement,
    },
    /// Create a task.
    CreateTask(Box<NewTask>),
    /// Update fields of a task.
    UpdateTask {
        /// The task.
        id: TaskId,
        /// The changes.
        patch: TaskPatch,
    },
    /// Move a task to another list.
    MoveTask {
        /// The task.
        id: TaskId,
        /// Destination list.
        list: ListId,
        /// If supplied, move only from this list and only if the task has not been merged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_list: Option<ListId>,
        /// Position among the new siblings.
        #[serde(default)]
        placement: Placement,
    },
    /// Complete a task. A recurring task spawns its next occurrence.
    CompleteTask {
        /// The task.
        id: TaskId,
    },
    /// Reopen a completed task.
    UncompleteTask {
        /// The task.
        id: TaskId,
    },
    /// Put a task in My Day for a date, or take it out with `None`.
    SetMyDay {
        /// The task.
        id: TaskId,
        /// The date, or `None` to clear.
        #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
        date: Option<Date>,
    },
    /// Attach a tag to a task.
    AddTag {
        /// The task.
        task: TaskId,
        /// The tag.
        tag: TagId,
    },
    /// Detach a tag from a task.
    RemoveTag {
        /// The task.
        task: TaskId,
        /// The tag.
        tag: TagId,
    },
    /// Merge `source` into `target`: children, tags and notes move over and `source` is deleted.
    MergeTasks {
        /// The task that goes away.
        source: TaskId,
        /// The task that absorbs it.
        target: TaskId,
    },
    /// Append text to a task's notes.
    AddNote {
        /// The task.
        task: TaskId,
        /// Text to append.
        text: String,
    },
    /// Create a step.
    CreateStep {
        /// New ID.
        id: StepId,
        /// The task.
        task: TaskId,
        /// Title.
        title: String,
        /// Position among the task's steps.
        #[serde(default)]
        placement: Placement,
    },
    /// Retitle a step.
    UpdateStep {
        /// The step.
        id: StepId,
        /// New title.
        title: String,
    },
    /// Complete a step.
    CompleteStep {
        /// The step.
        id: StepId,
    },
    /// Reopen a step.
    UncompleteStep {
        /// The step.
        id: StepId,
    },
    /// Create a tag.
    CreateTag {
        /// New ID.
        id: TagId,
        /// Name.
        name: String,
        /// Colour.
        #[serde(default)]
        color: Option<String>,
    },
    /// Rename or recolour a tag.
    UpdateTag {
        /// The tag.
        id: TagId,
        /// New name.
        #[serde(default)]
        name: Option<String>,
        /// New colour, or `Some(None)` to clear.
        #[serde(
            default,
            deserialize_with = "double_option",
            skip_serializing_if = "Option::is_none"
        )]
        color: Option<Option<String>>,
    },
    /// Add a link to a task.
    AddLink {
        /// New ID.
        id: LinkId,
        /// The task.
        task: TaskId,
        /// The target.
        kind: LinkKind,
    },
    /// Add an attachment reference to a task.
    AddAttachment {
        /// New ID.
        id: AttachmentId,
        /// The task.
        task: TaskId,
        /// File name.
        file_name: String,
        /// MIME type.
        media_type: String,
        /// Size in bytes.
        size_bytes: u64,
        /// Blob store key.
        storage_key: String,
    },
    /// Add a reminder to a task.
    AddReminder {
        /// New ID.
        id: ReminderId,
        /// The task.
        task: TaskId,
        /// When to fire.
        #[cfg_attr(feature = "openapi", schema(value_type = String, format = DateTime))]
        at: Timestamp,
    },
    /// Change a reminder's time.
    UpdateReminder {
        /// The reminder.
        id: ReminderId,
        /// New time.
        #[cfg_attr(feature = "openapi", schema(value_type = String, format = DateTime))]
        at: Timestamp,
    },
    /// Reorder a collection, list, task or step among its siblings.
    Reorder {
        /// The item.
        id: EntityId,
        /// Its new position.
        placement: Placement,
    },
    /// Delete (tombstone) any entity.
    Delete {
        /// The entity.
        id: EntityId,
    },
    /// Restore a deleted entity.
    Restore {
        /// The entity.
        id: EntityId,
    },
    /// Import an entity from another app, keeping its history. Owner only.
    ///
    /// Unlike the create commands, this takes the whole entity, so the times it already had are kept:
    /// its creation time, a task's last-modified time, and the completion time of a task or step. The
    /// actor in its stamps is replaced by the caller. The entity must be new, must not be deleted or
    /// merged, its parent and tags must be live, and it is validated like a created entity. A
    /// completed recurring task does not spawn its next occurrence. If a live sibling already uses
    /// the position key, the entity gets a fresh key just before that sibling.
    ImportEntity {
        /// The entity.
        entity: Box<Entity>,
    },
}
