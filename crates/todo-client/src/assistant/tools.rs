use super::provider::{ToolCall, ToolDefinition};
use super::{AssistantError, LimitKind, ListRef, TaskRef};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, io};
use todo_core::{
    State,
    ids::{CollectionId, ListId, TaskId},
    query::{EvalContext, parse_date},
};
use todo_tools::assistant::{self, Projection, ReadError, approval::Batch};

const MAX_RESULT_BYTES: usize = 24 * 1024;

pub(super) fn catalog() -> Vec<ToolDefinition> {
    macro_rules! definitions {
        ($($tool:ident),+ $(,)?) => {vec![$(ToolDefinition {
            name: assistant::$tool.name(),
            description: assistant::$tool.description,
            arguments: assistant::$tool.input_schema(),
        }),+]};
    }
    definitions!(
        LIST_LISTS,
        QUERY_TASKS,
        GET_TASK,
        CREATE_TASK,
        UPDATE_TASK,
        COMPLETE_TASK
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Lists,
    Search,
    Task,
    Create,
    Update,
    Complete,
}

impl ToolKind {
    pub(super) fn parse(name: &str) -> Result<Self, AssistantError> {
        Ok(match name {
            n if n == assistant::LIST_LISTS.name() => Self::Lists,
            n if n == assistant::QUERY_TASKS.name() => Self::Search,
            n if n == assistant::GET_TASK.name() => Self::Task,
            n if n == assistant::CREATE_TASK.name() => Self::Create,
            n if n == assistant::UPDATE_TASK.name() => Self::Update,
            n if n == assistant::COMPLETE_TASK.name() => Self::Complete,
            _ => return Err(AssistantError::UnsupportedTool),
        })
    }
}

#[derive(Default)]
pub(super) struct References {
    pub collections: BTreeSet<CollectionId>,
    pub lists: BTreeSet<ListId>,
    pub tasks: BTreeSet<TaskId>,
}

impl References {
    pub fn visible(&self, state: &State, allowed: &BTreeSet<ListId>) -> bool {
        self.collections
            .iter()
            .all(|id| super::view_context::collection_visible(state, allowed, *id))
            && self
                .lists
                .iter()
                .all(|id| allowed.contains(id) && state.is_live((*id).into()))
            && self.tasks.iter().all(|id| {
                state
                    .tasks
                    .get(id)
                    .is_some_and(|task| allowed.contains(&task.list) && state.is_live((*id).into()))
            })
    }
}

pub(super) struct ToolResult {
    pub data: Value,
    pub references: References,
    pub links: Vec<TaskRef>,
    pub count: usize,
}

fn argument(error: assistant::ArgumentError) -> AssistantError {
    match error {
        assistant::ArgumentError::UnsupportedField => AssistantError::UnsupportedField,
        assistant::ArgumentError::InvalidArguments => AssistantError::InvalidArguments,
    }
}

fn read_error(error: ReadError) -> AssistantError {
    match error {
        ReadError::NotFound => AssistantError::NotFound,
        ReadError::TooLarge {
            required_bytes,
            max_bytes,
        } => LimitKind::ToolResult.exceeded(required_bytes, max_bytes),
        ReadError::Encode(_) => AssistantError::InvalidDraft,
        _ => AssistantError::InvalidArguments,
    }
}

pub(super) struct ReadScope<'a> {
    pub projection: &'a Projection,
    pub base: &'a State,
    pub context: &'a EvalContext,
}

fn resolve_due(due: &mut String, context: &EvalContext) -> Result<(), AssistantError> {
    let date = parse_date(due)
        .and_then(|expr| context.resolve(expr))
        .ok_or(AssistantError::InvalidArguments)?;
    *due = date.to_string();
    Ok(())
}

pub(super) fn execute(
    call: ToolCall,
    kind: ToolKind,
    ReadScope {
        projection,
        base,
        context,
    }: ReadScope<'_>,
    batch: &mut Batch,
    index: usize,
    new_id: TaskId,
) -> Result<ToolResult, AssistantError> {
    let mut references = References::default();
    let mut links = Vec::new();
    let mut count = 1;
    let args = call.arguments;
    let mut record_task = |task: &assistant::TaskSummary<'_>| {
        references.lists.insert(task.list);
        references.tasks.insert(task.id);
        if let Some(list) = base.lists.get(&task.list) {
            links.push(TaskRef {
                id: task.id,
                title: task.title.to_owned(),
                due: task.due,
                list: ListRef {
                    id: list.id,
                    name: list.name.clone(),
                },
            });
        }
    };
    let data = match kind {
        ToolKind::Lists => {
            let args = assistant::LIST_LISTS.decode(args).map_err(argument)?;
            let page = projection
                .list_lists_within(&args, MAX_RESULT_BYTES)
                .map_err(read_error)?;
            references
                .lists
                .extend(page.items.iter().map(|item| item.id));
            count = page.items.len();
            result_json(&page)?
        }
        ToolKind::Search => {
            let args = assistant::QUERY_TASKS.decode(args).map_err(argument)?;
            let page = projection
                .query_tasks_within(&args, MAX_RESULT_BYTES)
                .map_err(read_error)?;
            page.items.iter().for_each(&mut record_task);
            count = page.items.len();
            result_json(&page)?
        }
        ToolKind::Task => {
            let args = assistant::GET_TASK.decode(args).map_err(argument)?;
            let detail = projection
                .get_task_within(&args, MAX_RESULT_BYTES)
                .map_err(read_error)?;
            record_task(&detail.task);
            result_json(&detail)?
        }
        ToolKind::Create => {
            let mut args = assistant::CREATE_TASK.decode(args).map_err(argument)?;
            if let Some(due) = &mut args.due {
                resolve_due(due, context)?;
            }
            let list = args
                .list_id
                .map(ListId::from_uuid)
                .or_else(|| projection.inbox())
                .filter(|id| projection.contains_list(*id))
                .ok_or(AssistantError::NotFound)?;
            references.lists.insert(list);
            let command = args
                .into_command(new_id, list)
                .map_err(|_| AssistantError::InvalidArguments)?;
            let changed = batch.push(command, index)?;
            json!({"status": if changed { "drafted" } else { "no_change" }, "task_id": new_id})
        }
        ToolKind::Update | ToolKind::Complete => {
            let (id, command) = if kind == ToolKind::Update {
                let mut args = assistant::UPDATE_TASK.decode(args).map_err(argument)?;
                if let Some(Some(due)) = &mut args.due {
                    resolve_due(due, context)?;
                }
                let id = TaskId::from_uuid(args.task_id);
                let command = args
                    .into_command()
                    .map_err(|_| AssistantError::InvalidArguments)?;
                (id, command)
            } else {
                let args = assistant::COMPLETE_TASK.decode(args).map_err(argument)?;
                (TaskId::from_uuid(args.task_id), args.into_command())
            };
            references.lists.insert(batch.task(id)?.list);
            // Draft-created IDs are valid for later commands in this batch, but are not references
            // to the confirmed replica and must not pass the live-visibility check as such.
            if base.tasks.contains_key(&id) {
                references.tasks.insert(id);
            }
            let changed = batch.push(command, index)?;
            json!({"status": if changed { "drafted" } else { "no_change" }, "task_id": id})
        }
    };
    Ok(ToolResult {
        data,
        references,
        links,
        count,
    })
}

fn result_json(value: &impl Serialize) -> Result<Value, AssistantError> {
    let mut writer = Limited {
        left: MAX_RESULT_BYTES,
        bytes: Some(Vec::new()),
        overflow: None,
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| writer.error(LimitKind::ToolResult, MAX_RESULT_BYTES))?;
    serde_json::from_slice(&writer.bytes.unwrap_or_default())
        .map_err(|_| AssistantError::InvalidDraft)
}

pub(super) fn check_size(
    value: &impl Serialize,
    max: usize,
    kind: LimitKind,
) -> Result<(), AssistantError> {
    let mut writer = Limited {
        left: max,
        bytes: None,
        overflow: None,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| writer.error(kind, max))
}

struct Limited {
    left: usize,
    bytes: Option<Vec<u8>>,
    overflow: Option<usize>,
}

impl Limited {
    fn error(&self, kind: LimitKind, max: usize) -> AssistantError {
        self.overflow.map_or(AssistantError::InvalidDraft, |extra| {
            kind.exceeded(max.saturating_add(extra), max)
        })
    }
}

impl io::Write for Limited {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(left) = self.left.checked_sub(buf.len()) else {
            self.overflow = Some(buf.len() - self.left);
            return Err(io::Error::other("assistant size limit"));
        };
        self.left = left;
        if let Some(bytes) = &mut self.bytes {
            bytes.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
