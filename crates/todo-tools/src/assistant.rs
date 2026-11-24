//! The local assistant surface. Advertise only these restricted tools, never the MCP catalog.
//!
//! A host creates a projection from one confirmed snapshot and the list IDs the owner opted into
//! for the selected provider profile. The projection does not expose the source snapshot. It does
//! not track replica revisions or execute proposals; the host must discard it when scope changes.

pub mod access;
pub mod approval;
mod projection;
pub mod relay;

use crate::{args::*, descriptor};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

pub use projection::{
    CollectionSummary, ListSummary, Page, Projection, ReadError, StepSummary, TagSummary,
    TaskDetail, TaskSummary,
};

/// A strict subset of the canonical input type. The same field allowlist drives discovery and
/// decoding; even a null value for a non-advertised field is refused.
#[derive(Debug)]
pub struct Tool<A: 'static> {
    canonical: &'static descriptor::Tool<A>,
    pub description: &'static str,
    fields: &'static [&'static str],
}

impl<A: JsonSchema + DeserializeOwned> Tool<A> {
    const fn new(
        canonical: &'static descriptor::Tool<A>,
        description: &'static str,
        fields: &'static [&'static str],
    ) -> Self {
        Self {
            canonical,
            description,
            fields,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        self.canonical.name
    }

    #[must_use]
    pub fn input_schema(&self) -> Map<String, Value> {
        let mut schema = self.canonical.input_schema().as_ref().clone();
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.retain(|name, _| self.fields.contains(&name.as_str()));
        }
        // All subsets keep the canonical required fields. Drop definitions unreachable from the
        // exposed properties (e.g. recurrence on task writes).
        let mut refs = Vec::new();
        collect_refs(&schema["properties"], &mut refs);
        if let Some(defs) = schema.get("$defs").and_then(Value::as_object) {
            let mut retained = Map::new();
            while let Some(name) = refs.pop() {
                if !retained.contains_key(&name)
                    && let Some(def) = defs.get(&name)
                {
                    collect_refs(def, &mut refs);
                    retained.insert(name, def.clone());
                }
            }
            if retained.is_empty() {
                schema.remove("$defs");
            } else {
                schema.insert("$defs".into(), Value::Object(retained));
            }
        }
        if let Some(query) = schema
            .get_mut("properties")
            .and_then(|p| p.get_mut("query"))
        {
            query["description"] = json!(
                "Smart-list query, e.g. `list:inbox and not done`, `due<=today sort:due`, or words in titles and steps. Notes are never searched. Empty matches all visible tasks. At most 1024 bytes, 64 nodes and 32 terms."
            );
        }
        if let Some(list) = schema
            .get_mut("properties")
            .and_then(|p| p.get_mut("list_id"))
        {
            list["description"] = json!(
                "The visible list to propose creating it in. Omit to use the visible Inbox if available."
            );
        }
        if let Some(due) = schema.get_mut("properties").and_then(|p| p.get_mut("due")) {
            due["description"] = json!(
                "Due date: YYYY-MM-DD, today, tomorrow, yesterday or a day offset such as +3d. Relative dates use the owner's local date. For update_task, null clears the date."
            );
        }
        if let Some(completed) = schema
            .get_mut("properties")
            .and_then(|p| p.get_mut("completed"))
        {
            completed["description"] = json!(
                "Propose marking done with true (the default) or reopening with false. Only non-repeating tasks are supported."
            );
        }
        schema
    }

    pub fn decode(&self, args: Map<String, Value>) -> Result<A, ArgumentError> {
        if args.keys().any(|key| !self.fields.contains(&key.as_str())) {
            return Err(ArgumentError::UnsupportedField);
        }
        self.canonical
            .decode(args)
            .map_err(|_| ArgumentError::InvalidArguments)
    }
}

fn collect_refs(schema: &Value, out: &mut Vec<String>) {
    match schema {
        Value::Object(object) => {
            if let Some(name) = object
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/$defs/"))
            {
                out.push(name.to_owned());
            }
            object.values().for_each(|child| collect_refs(child, out));
        }
        Value::Array(items) => items.iter().for_each(|child| collect_refs(child, out)),
        _ => {}
    }
}

/// No model-provided argument contents are echoed in these decoding errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArgumentError {
    #[error("unsupported tool argument")]
    UnsupportedField,
    #[error("invalid tool arguments")]
    InvalidArguments,
}

pub static LIST_LISTS: Tool<PageArgs> = Tool::new(
    &descriptor::LIST_LISTS,
    "List the visible lists with their IDs, features, collections and live task counts. Pass next_cursor back as cursor.",
    &["limit", "cursor"],
);
pub static QUERY_TASKS: Tool<QueryArgs> = Tool::new(
    &descriptor::QUERY_TASKS,
    "Find visible task summaries with a smart-list query. Pass next_cursor back as cursor with the same query.",
    &["query", "limit", "cursor"],
);
pub static GET_TASK: Tool<GetTaskArgs> = Tool::new(
    &descriptor::GET_TASK,
    "Get one visible task with its workflow status, dates and checklist. Does not return notes, links, attachments or reminders.",
    &["task_id"],
);
pub static CREATE_TASK: Tool<CreateTaskArgs> = Tool::new(
    &descriptor::CREATE_TASK,
    "Propose creating a task in a visible list. Nothing changes until the owner approves.",
    &["title", "list_id", "due", "priority"],
);
pub static UPDATE_TASK: Tool<UpdateTaskArgs> = Tool::new(
    &descriptor::UPDATE_TASK,
    "Propose changing a visible task's title, due date or priority. Omit unchanged fields or use null to clear due. Nothing changes until the owner approves.",
    &["task_id", "title", "due", "priority"],
);
pub static COMPLETE_TASK: Tool<CompleteTaskArgs> = Tool::new(
    &descriptor::COMPLETE_TASK,
    "Propose completing a visible, non-repeating task, or reopening it with completed: false. Nothing changes until the owner approves.",
    &["task_id", "completed"],
);
