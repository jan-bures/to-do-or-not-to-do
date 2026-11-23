//! Typed input arguments. Serde decoding and the corresponding input schemas share these types.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

/// Paging for collections and lists.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageArgs {
    /// Also return deleted (archived) items. Default false.
    #[serde(default)]
    pub include_deleted: bool,
    /// Page size, 1–500 (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// `next_cursor` from the previous page; omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// One task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetTaskArgs {
    /// The task's ID.
    pub task_id: Uuid,
    /// Also return it (and its steps) when deleted. Default false.
    #[serde(default)]
    pub include_deleted: bool,
}

/// A smart-list query.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryArgs {
    /// Query in the smart-list language, for example `list:inbox and not done`, `due<=today sort:due`,
    /// `status:in_progress start<=today sort:start`, `priority>=high tag:work`, `my_day`, or free
    /// words matched against titles, notes and steps (`milk`). Empty matches every live task.
    /// At most 1024 bytes, 64 nodes and 32 terms.
    #[serde(default)]
    pub query: Option<String>,
    /// Page size, 1–500 (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// `next_cursor` from the previous page of the same query; omit for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Task priority.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PriorityArg {
    /// No priority.
    None,
    /// Low.
    Low,
    /// Normal.
    Normal,
    /// High (shows in "Important").
    High,
}

/// Where to put an item among its siblings.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PositionArg {
    /// Before all others.
    First,
    /// After all others (the default).
    #[default]
    Last,
}

/// Day of the week.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WeekdayArg {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

fn one() -> u32 {
    1
}

/// How often a task repeats. `interval` is 1–999 (default 1): `{"freq": "weekly", "interval": 2,
/// "days": ["mon"]}` is every other Monday.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "freq", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleArg {
    /// Every `interval` days.
    Daily {
        /// Days between occurrences (default 1).
        #[serde(default = "one")]
        interval: u32,
    },
    /// Every `interval` weeks, on the given weekdays (weeks start on Monday).
    Weekly {
        /// Weeks between occurrences (default 1).
        #[serde(default = "one")]
        interval: u32,
        /// The weekdays it falls on; at least one.
        days: Vec<WeekdayArg>,
    },
    /// Every `interval` months on a day of the month (1–31; shorter months use their last day).
    Monthly {
        /// Months between occurrences (default 1).
        #[serde(default = "one")]
        interval: u32,
        /// Day of the month, 1–31.
        day: u8,
    },
    /// Every `interval` years on the due date's month and day (29 February becomes 28 February).
    Yearly {
        /// Years between occurrences (default 1).
        #[serde(default = "one")]
        interval: u32,
    },
}

/// A repeat rule, the same shape `get_task` returns as `recurrence`. The task must have a due date:
/// completing it creates the next occurrence, due on the next date the rule gives after the completed
/// task's due date. Example: `{"rule": {"freq": "monthly", "day": 1}, "until": "2031-12-31"}`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecurrenceArg {
    /// The repeat pattern, tagged by `freq`: `daily`, `weekly` (needs `days`), `monthly` (needs
    /// `day`) or `yearly`.
    pub rule: RuleArg,
    /// Last date, `YYYY-MM-DD` (inclusive), on which an occurrence may fall. Omit to repeat forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

/// A new task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTaskArgs {
    /// Title (required, one line).
    pub title: String,
    /// The list to create it in. Omit to use the Inbox (the list named "Inbox" that you can capture
    /// into).
    #[serde(default)]
    pub list_id: Option<Uuid>,
    /// Notes (plain text or Markdown).
    #[serde(default)]
    pub notes: Option<String>,
    /// Due date, `YYYY-MM-DD`.
    #[serde(default)]
    pub due: Option<String>,
    /// Initial open stage: `backlog`, `todo` (default), or `in_progress`.
    #[serde(default)]
    pub status: Option<String>,
    /// Start date, `YYYY-MM-DD`; must be on or before `due` when both are set.
    #[serde(default)]
    pub start_date: Option<String>,
    /// Priority. Default none.
    #[serde(default)]
    pub priority: Option<PriorityArg>,
    /// Make it repeat (needs `due`, and plan level on the list). Default: it doesn't repeat.
    #[serde(default)]
    pub recurrence: Option<RecurrenceArg>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Text to append to a task's notes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddNoteArgs {
    /// The task.
    pub task_id: Uuid,
    /// Text to append (on a new paragraph).
    pub text: String,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Changes to a task. Omitted fields are left alone.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// New title.
    #[serde(default)]
    pub title: Option<String>,
    /// New notes, replacing the old ones (use `add_note` to append instead).
    #[serde(default)]
    pub notes: Option<String>,
    /// New due date `YYYY-MM-DD`, or `null` to clear it.
    #[serde(default, deserialize_with = "double_option")]
    pub due: Option<Option<String>>,
    /// Open stage: `backlog`, `todo`, or `in_progress`. Use `complete_task` for Done.
    #[serde(default)]
    pub status: Option<String>,
    /// Start date, `YYYY-MM-DD`, or `null` to clear. Editing only one date can shift the other
    /// to keep start on or before due. When supplying both, give a valid span.
    #[serde(default, deserialize_with = "double_option")]
    pub start_date: Option<Option<String>>,
    /// New priority.
    #[serde(default)]
    pub priority: Option<PriorityArg>,
    /// New repeat rule, replacing the old one, or `null` to stop repeating. The task must have a due
    /// date (its current one, or `due` in the same call).
    #[serde(default, deserialize_with = "double_option")]
    pub recurrence: Option<Option<RecurrenceArg>>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Complete or reopen a task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompleteTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// `true` (default) completes it; a repeating task then gets its next occurrence. `false` reopens it.
    #[serde(default = "yes")]
    pub completed: bool,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

fn yes() -> bool {
    true
}

/// Move a task to another list.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// The destination list.
    pub list_id: Uuid,
    /// Expected source list ID. If supplied, refuse the move if the task was moved or merged.
    #[serde(default)]
    pub from_list: Option<Uuid>,
    /// Where in the destination list. Default last.
    #[serde(default)]
    pub position: PositionArg,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// A new checklist step.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddStepArgs {
    /// The task.
    pub task_id: Uuid,
    /// The step's title.
    pub title: String,
    /// Where among the task's steps. Default last.
    #[serde(default)]
    pub position: PositionArg,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Changes to a step. Give `title`, `completed`, or both.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateStepArgs {
    /// The step.
    pub step_id: Uuid,
    /// New title.
    #[serde(default)]
    pub title: Option<String>,
    /// `true` ticks the step off, `false` reopens it.
    #[serde(default)]
    pub completed: Option<bool>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Put a task in My Day, or take it out.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetMyDayArgs {
    /// The task.
    pub task_id: Uuid,
    /// `true` (default) adds it to My Day, `false` removes it.
    #[serde(default = "yes")]
    pub in_my_day: bool,
    /// The day, `YYYY-MM-DD`. Default: today in the owner's time zone. Ignored when removing.
    #[serde(default)]
    pub date: Option<String>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Merge a duplicate into another task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeTasksArgs {
    /// The duplicate. Its steps, links, attachments, tags and notes move to the target, and it is
    /// deleted.
    pub source_task_id: Uuid,
    /// The task that is kept.
    pub target_task_id: Uuid,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// One task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// A tag and a task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagTaskArgs {
    /// The task.
    pub task_id: Uuid,
    /// The tag (from `list_tags` or `create_tag`).
    pub tag_id: Uuid,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// A new tag.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTagArgs {
    /// Name, unique among tags (case-insensitively), for example `work`.
    pub name: String,
    /// Colour, for example `#336699`. Default none.
    #[serde(default)]
    pub color: Option<String>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// A new reminder.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddReminderArgs {
    /// The task.
    pub task_id: Uuid,
    /// When to remind: an RFC 3339 time with an offset (`2030-01-02T09:00:00Z`,
    /// `2030-01-02T09:00:00+01:00`), or a local time without one (`2030-01-02T09:00`), read in the
    /// owner's time zone.
    pub at: String,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// A new time for a reminder.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateReminderArgs {
    /// The reminder (`reminders[].id` in `get_task`, or `reminder_id` from `add_reminder`).
    pub reminder_id: Uuid,
    /// The new time, in the same forms as `add_reminder`'s `at`.
    pub at: String,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// A web link to add to a task.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddLinkArgs {
    /// The task.
    pub task_id: Uuid,
    /// An `http` or `https` URL.
    pub url: String,
    /// Display title. Default: none (the URL is shown).
    #[serde(default)]
    pub title: Option<String>,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact write.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}

/// Paging for the activity feed.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActivityArgs {
    /// Only events with `seq` below this (the previous page's `next_before`). Default: the newest.
    #[serde(default)]
    pub before: Option<u64>,
    /// Page size, 1–200 (default 50).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Undo one change.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoArgs {
    /// `seq` of any event of the change (from a write result or `get_activity`). Undoing an undo
    /// redoes the change.
    pub seq: u64,
    /// The `id` of that same event, as given next to its `seq`. If another event has that `seq` (the
    /// list was restored from a backup since), the undo is refused with `event_mismatch` instead of
    /// undoing the wrong change.
    pub event_id: Uuid,
    /// The `epoch` of the result or feed you took the event from. If the list has been restored
    /// since, the undo is refused with `epoch_mismatch`. Optional but recommended.
    #[serde(default)]
    pub epoch: Option<Uuid>,
    /// Undo even though later changes touched the same fields (only after an `undo_conflict` you have
    /// looked at). Default false.
    #[serde(default)]
    pub force: bool,
    /// Idempotency key (UUID). Reuse it, with the same arguments, only to retry this exact undo.
    #[serde(default)]
    pub command_id: Option<Uuid>,
}
