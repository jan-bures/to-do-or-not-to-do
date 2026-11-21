//! Request and response bodies that are not `todo-core` types.

use crate::db::LogEvent;
use serde::{Deserialize, Serialize};
use todo_core::command::Command;
use todo_core::ids::EventId;
use todo_core::model::{AttachmentRef, Collection, Link, List, Reminder, Step, Tag, Task};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// `POST /api/v1/commands` body.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    /// Client-generated ID (UUIDv7 recommended). Retrying with the same ID returns the first result.
    pub command_id: Uuid,
    /// The command.
    pub command: Command,
}

/// `POST /api/v1/events/{seq}/undo` body.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoRequest {
    /// Undo even if later events changed the same fields.
    #[serde(default)]
    pub force: bool,
    /// Required. Client-generated ID that makes the undo safe to retry: after a time-out or
    /// `outcome_unknown`, retrying with the same ID returns the stored result instead of undoing
    /// twice. Missing gets 400 `command_id_required`.
    #[serde(default)]
    #[schema(required = true, value_type = Uuid)]
    pub command_id: Option<Uuid>,
    /// Required. The ID of the event at `seq` the client means to undo. If another event is there
    /// (the log was restored and regrew), the undo gets 409 `event_mismatch`. Missing gets 400
    /// `expected_event_id_required`.
    #[serde(default)]
    #[schema(required = true, value_type = String)]
    pub expected_event_id: Option<EventId>,
    /// The log epoch the client saw the event in (see `GET /api/v1/events`). If the log has
    /// another one, the undo gets 409 `epoch_mismatch`.
    #[serde(default)]
    pub epoch: Option<Uuid>,
}

/// A task with everything attached to it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TaskDetail {
    /// The task (returned even if deleted; check `deleted`).
    pub task: Task,
    /// Steps, in order.
    pub steps: Vec<Step>,
    /// Source links.
    pub links: Vec<Link>,
    /// Attachment references.
    pub attachments: Vec<AttachmentRef>,
    /// Reminders, earliest first.
    pub reminders: Vec<Reminder>,
    /// The task's tags.
    pub tags: Vec<Tag>,
    /// True if any of the lists above was cut at its maximum size (see the endpoint description).
    pub truncated: bool,
}

/// A page of the event log.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct EventPage {
    /// Events with `seq > since`, oldest first.
    pub events: Vec<LogEvent>,
    /// Pass as `since` to get the next page.
    pub next_since: u64,
    /// The newest sequence number when the page was read.
    pub head_seq: u64,
    /// True if more events follow.
    pub has_more: bool,
    /// The log epoch the sequence numbers belong to. If it is not the one the client's cursor
    /// belongs to, the client must resync from 0.
    pub epoch: Uuid,
}

/// One activity feed entry.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActivityItem {
    /// The event.
    pub event: LogEvent,
    /// One-line description.
    pub summary: String,
    /// True if this event's effect is currently undone.
    pub undone: bool,
    /// True if `POST /api/v1/events/{seq}/undo` may be tried on this event (it is the latest link of
    /// its undo/redo chain).
    pub can_undo: bool,
}

/// A page of the activity feed, newest first.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ActivityPage {
    /// Entries, newest first.
    pub items: Vec<ActivityItem>,
    /// Pass as `before` to get the next (older) page; absent at the end.
    pub next_before: Option<u64>,
}

/// A built-in smart list.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SmartList {
    /// Stable ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The query, in the smart-list DSL.
    pub query: String,
}

/// `?include_deleted=`.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct IncludeDeleted {
    /// Also return deleted entities.
    #[serde(default)]
    pub include_deleted: bool,
}

/// Default page size of collection reads.
pub const DEFAULT_PAGE: usize = 100;
/// Largest page size of collection reads.
pub const MAX_PAGE: usize = 500;

/// Paging and `?include_deleted=` for collections, lists and tags.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReadParams {
    /// Also return deleted entities.
    #[serde(default)]
    pub include_deleted: bool,
    /// Page size (1–500, default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// `next_cursor` of the previous page; absent for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Filters and paging for the tasks of one list.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListTasksParams {
    /// Also return deleted tasks.
    #[serde(default)]
    pub include_deleted: bool,
    /// Only completed (`true`) or only open (`false`) tasks.
    #[serde(default)]
    pub done: Option<bool>,
    /// Page size (1–500, default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// `next_cursor` of the previous page; absent for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// `?query=` and paging for the task search.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TaskQueryParams {
    /// Smart-list query, for example `due<=today and not done sort:due`. Empty matches every live task.
    /// At most 1024 bytes, 64 nodes and 32 terms (`query_too_complex` otherwise).
    #[serde(default)]
    pub query: Option<String>,
    /// Page size (1–500, default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// `next_cursor` of the previous page; absent for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Paging only.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageParams {
    /// Page size (1–500, default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// `next_cursor` of the previous page; absent for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
}

macro_rules! page {
    ($name:ident, $item:ty, $what:literal) => {
        #[doc = concat!("A page of ", $what, ".")]
        #[derive(Debug, Clone, Serialize, ToSchema)]
        pub struct $name {
            #[doc = concat!("The ", $what, ", in order.")]
            pub items: Vec<$item>,
            /// Pass as `cursor` to get the next page; absent on the last page. Opaque.
            pub next_cursor: Option<String>,
        }
    };
}

page!(CollectionPage, Collection, "collections");
page!(ListPage, List, "lists");
page!(TaskPage, Task, "tasks");
page!(TagPage, Tag, "tags");

/// Paging for the event log.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EventsParams {
    /// Return events after this sequence number (default 0). For SSE, `Last-Event-ID` is used when
    /// this is absent.
    #[serde(default)]
    pub since: Option<u64>,
    /// Page size for JSON (1–1000, default 500). A page also stops before 4 MiB of event JSON.
    #[serde(default)]
    pub limit: Option<usize>,
    /// The log epoch the client's cursor belongs to. If the log has another one, the request gets
    /// 409 `epoch_mismatch` and the client must resync from 0.
    #[serde(default)]
    pub epoch: Option<Uuid>,
}

/// Paging for the activity feed.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ActivityParams {
    /// Return events with `seq < before` (default: the newest).
    #[serde(default)]
    pub before: Option<u64>,
    /// Page size (1–200, default 50).
    #[serde(default)]
    pub limit: Option<usize>,
}
