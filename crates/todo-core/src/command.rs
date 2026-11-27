//! Commands: requests to change the state.
//!
//! A client generates the IDs of the entities it creates, so a retried command is recognisable. The
//! server validates a command with [`decide`](crate::decide::decide) and stores the resulting events.

use crate::ids::{
    AttachmentId, CollectionId, EntityId, LinkId, ListId, ReminderId, StepId, TagId, TaskId,
};
use crate::model::{
    Entity, LinkKind, ListFeatures, ListView, Priority, Task, TaskSort, WorkflowStatus,
};
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

/// An explicit action on several tasks, never a toggle against changing state.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BulkTaskAction {
    /// Complete open tasks or reopen done tasks.
    Completion { done: bool },
    /// Set or clear High priority, without clearing Low or Normal.
    Important { on: bool },
    /// Add to the given day, or remove only tasks on that day.
    MyDay {
        #[cfg_attr(feature = "openapi", schema(value_type = String, format = Date))]
        day: Date,
        on: bool,
    },
    /// Move to the front of another list.
    Move { list: ListId },
    /// Place open tasks from a single list together, in the supplied order.
    Reorder { list: ListId, placement: Placement },
    /// Change the open stage of open tasks; completed tasks are left alone.
    Status { status: WorkflowStatus },
    /// Set due, or clear both due and repeat.
    Due {
        #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
        date: Option<Date>,
    },
    /// Move to Trash.
    Delete,
}

impl BulkTaskAction {
    /// The ordinary single-task command, if this action changes this task.
    #[must_use]
    pub fn change(self, task: &Task) -> Option<Command> {
        let id = task.id;
        Some(match self {
            Self::Completion { done } if done != task.is_done() => {
                if done {
                    Command::CompleteTask { id }
                } else {
                    Command::UncompleteTask { id }
                }
            }
            Self::Important { on } if on != (task.priority == Priority::High) => {
                Command::UpdateTask {
                    id,
                    patch: TaskPatch {
                        priority: Some(if on { Priority::High } else { Priority::None }),
                        ..TaskPatch::default()
                    },
                }
            }
            Self::MyDay { day, on } if on != (task.my_day == Some(day)) => Command::SetMyDay {
                id,
                date: on.then_some(day),
            },
            Self::Move { list } if list != task.list => Command::MoveTask {
                id,
                list,
                from_list: Some(task.list),
                placement: Placement::First,
            },
            Self::Reorder { list, placement } if list == task.list && !task.is_done() => {
                Command::Reorder {
                    id: id.into(),
                    placement,
                }
            }
            Self::Status { status } if !task.is_done() && status != task.status => {
                Command::UpdateTask {
                    id,
                    patch: TaskPatch {
                        status: Some(status),
                        ..TaskPatch::default()
                    },
                }
            }
            Self::Due { date } if date != task.due => Command::UpdateTask {
                id,
                patch: TaskPatch {
                    due: Some(date),
                    recurrence: (date.is_none() && task.recurrence.is_some()).then_some(None),
                    ..TaskPatch::default()
                },
            },
            Self::Delete => Command::Delete { id: id.into() },
            _ => return None,
        })
    }
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
    /// Initial open stage.
    #[serde(default, skip_serializing_if = "WorkflowStatus::is_default")]
    pub status: WorkflowStatus,
    /// Optional start date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
    pub start_date: Option<Date>,
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
            status: WorkflowStatus::Todo,
            start_date: None,
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
    /// New due date, or `Some(None)` to clear. If this is the only date edited, a due date before
    /// the existing start brings the start back to the new due date.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
    pub due: Option<Option<Date>>,
    /// New open stage. Does not change completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkflowStatus>,
    /// New start date, or `Some(None)` to clear it. If this is the only date edited, a start date
    /// after the existing due date brings due forward to the new start date.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = Date))]
    pub start_date: Option<Option<Date>>,
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

/// A change to individual list settings. An omitted setting keeps its current value.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListFeaturesPatch {
    /// Enable or disable workflow and the Board view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<bool>,
    /// Enable or disable start dates and the Timeline view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<bool>,
    /// Change the default presentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_view: Option<ListView>,
}

impl ListFeaturesPatch {
    /// Merge the patch, before checking whether its resulting view is enabled.
    #[must_use]
    pub fn merge(self, current: ListFeatures) -> ListFeatures {
        ListFeatures {
            workflow: self.workflow.unwrap_or(current.workflow),
            timeline: self.timeline.unwrap_or(current.timeline),
            default_view: self.default_view.unwrap_or(current.default_view),
        }
    }
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
    /// Rename or recolour a list, or change its optional features.
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
        /// Partial feature settings; omitted fields are left alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        features: Option<ListFeaturesPatch>,
    },
    /// Change the order of open tasks in a collection or list.
    SetTaskSort {
        /// The collection or list.
        id: EntityId,
        /// The selected order.
        sort: TaskSort,
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
    /// Place a card in an open board column, reopening it if completed. One undoable update.
    MoveTaskStatus {
        /// The card.
        id: TaskId,
        /// The list the board was built from; reject a stale drop after a move.
        list: ListId,
        /// Destination open stage.
        status: WorkflowStatus,
        /// Destination in the list's shared ordering, relative to an open card in this stage.
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
    /// Apply one action to a set of tasks as a single event group, in room order.
    BulkTasks {
        /// Distinct, live task IDs.
        tasks: Vec<TaskId>,
        /// The same explicit action on each applicable task.
        action: BulkTaskAction,
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
