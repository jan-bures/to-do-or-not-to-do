//! Canonical tool descriptions and JSON Schema, generated from the deserialization contract.

use crate::args::*;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::{
    marker::PhantomData,
    sync::{Arc, OnceLock},
};

/// Shared tool metadata tied to the type the caller decodes. Transport annotations stay in the adapter.
#[derive(Debug)]
pub struct Tool<A> {
    pub name: &'static str,
    pub description: &'static str,
    schema: OnceLock<Arc<Map<String, Value>>>,
    args: PhantomData<fn() -> A>,
}

impl<A: JsonSchema + DeserializeOwned> Tool<A> {
    const fn new(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            schema: OnceLock::new(),
            args: PhantomData,
        }
    }

    /// JSON Schema 2020-12 input object. Omit the Rust type's name and doc at the root.
    #[must_use]
    pub fn input_schema(&self) -> Arc<Map<String, Value>> {
        Arc::clone(self.schema.get_or_init(|| {
            let schema = SchemaSettings::draft2020_12()
                .into_generator()
                .into_root_schema_for::<A>();
            let Value::Object(mut schema) = schema.to_value() else {
                unreachable!("tool inputs must have an object schema");
            };
            assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
            schema.remove("title");
            schema.remove("description");
            Arc::new(schema)
        }))
    }

    /// Decodes only this tool's typed input, never raw domain commands.
    pub fn decode(&self, args: Map<String, Value>) -> Result<A, serde_json::Error> {
        serde_json::from_value(Value::Object(args))
    }
}

pub static LIST_COLLECTIONS: Tool<PageArgs> = Tool::new(
    "list_collections",
    "List the collections (groups of lists) you can see, in display order. Paginated: pass next_cursor back as cursor.",
);
pub static LIST_LISTS: Tool<PageArgs> = Tool::new(
    "list_lists",
    "List the lists you can see, with IDs, collections and features (workflow, timeline, default_view), in display order. Paginated: pass next_cursor back as cursor.",
);
pub static LIST_TAGS: Tool<PageArgs> = Tool::new(
    "list_tags",
    "List the tags you can see (with their IDs, names and colors). Without a default level of read you see only the tags on tasks you can read. Paginated: pass next_cursor back as cursor.",
);
pub static GET_TASK: Tool<GetTaskArgs> = Tool::new(
    "get_task",
    "Get one task with its workflow status (including Done), start date, steps, links, attachments, reminders and tags.",
);
pub static QUERY_TASKS: Tool<QueryArgs> = Tool::new(
    "query_tasks",
    "Find tasks with a smart-list query (see the query argument), across the lists you can see. Use it before create_task to avoid duplicates. Paginated: pass next_cursor back as cursor with the same query.",
);
pub static CREATE_TASK: Tool<CreateTaskArgs> = Tool::new(
    "create_task",
    "Create a task (capture level; plan level to make it repeat). Goes to the Inbox unless list_id is given. Optional due date, start date, open status, priority and repeat rule (which needs a due date). Returns task_id and the event. Query first to avoid duplicates; retry with the same command_id.",
);
pub static ADD_NOTE: Tool<AddNoteArgs> =
    Tool::new("add_note", "Append text to a task's notes (capture level).");
pub static ADD_LINK: Tool<AddLinkArgs> = Tool::new(
    "add_link",
    "Add a web link (http or https URL, optional title) to a task (capture level). Returns link_id.",
);
pub static UPDATE_TASK: Tool<UpdateTaskArgs> = Tool::new(
    "update_task",
    "Change a task's title, notes, open status, start date, due date, priority or repeat rule (plan level). Omitted fields stay as they are, except that a one-date edit can shift the other date to keep start on or before due. Null clears start_date, due or recurrence; a repeat rule needs a due date. Use complete_task for Done.",
);
pub static COMPLETE_TASK: Tool<CompleteTaskArgs> = Tool::new(
    "complete_task",
    "Complete a task, or reopen (uncomplete) it with completed: false (plan level). Completing a repeating task creates its next occurrence.",
);
pub static MOVE_TASK: Tool<MoveTaskArgs> = Tool::new(
    "move_task",
    "Move a task to another list (plan level on both lists).",
);
pub static ADD_STEP: Tool<AddStepArgs> = Tool::new(
    "add_step",
    "Add a checklist step to a task (plan level). Returns step_id.",
);
pub static UPDATE_STEP: Tool<UpdateStepArgs> = Tool::new(
    "update_step",
    "Retitle a step and/or tick it off (completed: true) or reopen it (completed: false) (plan level).",
);
pub static SET_MY_DAY: Tool<SetMyDayArgs> = Tool::new(
    "set_my_day",
    "Put a task in My Day (today by default) or take it out with in_my_day: false (plan level).",
);
pub static TAG_TASK: Tool<TagTaskArgs> = Tool::new(
    "tag_task",
    "Put a tag on a task (plan level). Get tag IDs from list_tags. Tagging a task that already has the tag changes nothing.",
);
pub static UNTAG_TASK: Tool<TagTaskArgs> = Tool::new(
    "untag_task",
    "Take a tag off a task (plan level). The tag itself stays. Untagging a task that doesn't have the tag changes nothing. If that was the last task through which you saw the tag, a retry gets not_found: confirm with get_task on the task instead.",
);
pub static CREATE_TAG: Tool<CreateTagArgs> = Tool::new(
    "create_tag",
    "Create a tag (needs plan level on at least one existing list and a default level of at least read). Returns tag_id. Check list_tags first: names are unique. Renaming and deleting tags is the owner's.",
);
pub static ADD_REMINDER: Tool<AddReminderArgs> = Tool::new(
    "add_reminder",
    "Add a reminder to a task at a given time (plan level). at is an RFC 3339 time with an offset, or a local time (2030-01-02T09:00) in the owner's time zone. Returns reminder_id.",
);
pub static UPDATE_REMINDER: Tool<UpdateReminderArgs> = Tool::new(
    "update_reminder",
    "Move a reminder to another time (plan level). at takes the same forms as in add_reminder.",
);
pub static MERGE_TASKS: Tool<MergeTasksArgs> = Tool::new(
    "merge_tasks",
    "Merge a duplicate task (source) into the one to keep (target) (plan level on both). The source's steps, links, attachments, tags and notes move over and the source is deleted.",
);
pub static ARCHIVE_TASK: Tool<ArchiveTaskArgs> = Tool::new(
    "archive_task",
    "Delete a task, softly (plan level). There is no separate archived state: this is the same delete as in the app. The task, its steps, links and reminders leave every list and query; get_task still finds it with include_deleted: true, and nothing is erased. The only way back is undo_event on this result's event (or the owner restoring it).",
);
pub static GET_ACTIVITY: Tool<ActivityArgs> = Tool::new(
    "get_activity",
    "Recent changes, newest first, with one-line summaries, who made them and whether they can be undone. Only changes in lists you can see. Paginated: pass next_before back as before.",
);
pub static UNDO_EVENT: Tool<UndoArgs> = Tool::new(
    "undo_event",
    "Undo one of your own changes by the seq and id of any of its events (undoing an undo redoes it). Refused with undo_conflict if later changes touched the same fields; look at them before retrying with force: true.",
);
