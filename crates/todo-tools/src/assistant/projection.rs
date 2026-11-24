use crate::args::{GetTaskArgs, PageArgs, QueryArgs};
use crate::command::resolve_inbox;
use crate::read::{
    Binding, InvalidCursor, MAX_DETAIL_CHILDREN, QueryError, decode_cursor, encode_cursor,
    list_key, page_limit, parse_query, select, step_key,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use todo_core::ids::{CollectionId, ListId, StepId, TagId, TaskId};
use todo_core::model::{List, ListFeatures, Priority, Task};
use todo_core::query::{EvalContext, KeyPart, KeyValue, OrderKey, QueryStatus};
use todo_core::state::State;
use todo_core::time::Date;

/// A page containing only visible data. Local cursors are not valid REST cursors.
#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListSummary<'a> {
    pub id: ListId,
    pub name: &'a str,
    pub collection: Option<CollectionSummary<'a>>,
    pub features: ListFeatures,
    pub open_count: usize,
    pub completed_count: usize,
}

#[derive(Debug, Serialize)]
pub struct CollectionSummary<'a> {
    pub id: CollectionId,
    pub name: &'a str,
}

#[derive(Debug, Serialize)]
pub struct TagSummary<'a> {
    pub id: TagId,
    pub name: &'a str,
}

#[derive(Debug, Serialize)]
pub struct TaskSummary<'a> {
    pub id: TaskId,
    pub list: ListId,
    pub title: &'a str,
    pub status: QueryStatus,
    pub start_date: Option<Date>,
    pub due: Option<Date>,
    pub priority: Priority,
    pub in_my_day: bool,
    pub repeats: bool,
}

#[derive(Debug, Serialize)]
pub struct StepSummary<'a> {
    pub id: StepId,
    pub title: &'a str,
    pub completed: bool,
}

#[derive(Debug, Serialize)]
pub struct TaskDetail<'a> {
    pub task: TaskSummary<'a>,
    pub steps: Vec<StepSummary<'a>>,
    pub tags: Vec<TagSummary<'a>>,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("task not found")]
    NotFound,
    #[error("deleted lists and tasks are not available")]
    IncludeDeleted,
    #[error("the read needs {required_bytes} bytes but the response budget is {max_bytes} bytes")]
    TooLarge {
        required_bytes: usize,
        max_bytes: usize,
    },
    #[error("the read could not be encoded")]
    Encode(#[from] serde_json::Error),
    #[error(transparent)]
    InvalidCursor(#[from] InvalidCursor),
    #[error(transparent)]
    InvalidQuery(#[from] QueryError),
}

/// A snapshot of opted-in, live lists and their live tasks. Only the small summaries returned by
/// its methods are intended as model input. Filtering and removing notes before constructing the
/// core query matcher also prevents full-text queries from probing excluded content.
pub struct Projection {
    state: State,
    ctx: EvalContext,
    list_counts: BTreeMap<ListId, (usize, usize)>,
    scope_key: String,
}

impl fmt::Debug for Projection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Projection").finish_non_exhaustive()
    }
}

impl Projection {
    /// `allowed` is an explicit allowlist; no newly added list is visible by default.
    #[must_use]
    pub fn new(confirmed: &State, allowed: &BTreeSet<ListId>, ctx: EvalContext) -> Self {
        let mut state = State::new();
        for id in allowed {
            let Some(list) = confirmed.lists.get(id).filter(|l| l.deleted.is_none()) else {
                continue;
            };
            if let Some(parent) = list.collection {
                let Some(collection) = confirmed
                    .collections
                    .get(&parent)
                    .filter(|c| c.deleted.is_none())
                else {
                    continue;
                };
                state
                    .collections
                    .entry(parent)
                    .or_insert_with(|| collection.clone());
            }
            state.lists.insert(*id, list.clone());
        }
        let mut list_counts: BTreeMap<ListId, (usize, usize)> = BTreeMap::new();
        for task in confirmed
            .tasks
            .values()
            .filter(|t| t.deleted.is_none() && state.lists.contains_key(&t.list))
        {
            let counts = list_counts.entry(task.list).or_default();
            if task.is_done() {
                counts.1 += 1;
            } else {
                counts.0 += 1;
            }
            let tags = task
                .tags
                .iter()
                .filter_map(|id| {
                    let tag = confirmed.tags.get(id).filter(|t| t.deleted.is_none())?;
                    state.tags.entry(*id).or_insert_with(|| tag.clone());
                    Some(*id)
                })
                .collect();
            // Copy only what evaluation and the summaries need. Never copy notes, a merge
            // destination, links, attachments, reminders, grants or the undo journal.
            state.tasks.insert(
                task.id,
                Task {
                    id: task.id,
                    list: task.list,
                    title: task.title.clone(),
                    notes: String::new(),
                    due: task.due,
                    status: task.status,
                    start_date: task.start_date,
                    priority: task.priority,
                    my_day: task.my_day,
                    completed_at: task.completed_at,
                    recurrence: task.recurrence.clone(),
                    tags,
                    position: task.position.clone(),
                    created: task.created.clone(),
                    updated: task.updated.clone(),
                    deleted: None,
                    merged_into: None,
                },
            );
        }
        state.steps.extend(
            confirmed
                .steps
                .iter()
                .filter(|(_, s)| s.deleted.is_none() && state.tasks.contains_key(&s.task))
                .map(|(id, step)| (*id, step.clone())),
        );
        // Only live, visible IDs affect the cursor binding, never hidden list choices or content.
        let ids: Vec<_> = state.lists.keys().map(ToString::to_string).collect();
        let scope_key = ids.join(":");
        Self {
            state,
            ctx,
            list_counts,
            scope_key,
        }
    }

    #[must_use]
    pub fn contains_list(&self, id: ListId) -> bool {
        self.state.lists.contains_key(&id)
    }

    /// Resolve the Inbox only within the visible projection. The host still validates writes.
    #[must_use]
    pub fn inbox(&self) -> Option<ListId> {
        resolve_inbox(&self.state, |_| true)
    }

    pub fn list_lists(&self, args: &PageArgs) -> Result<Page<ListSummary<'_>>, ReadError> {
        self.list_lists_within(args, usize::MAX)
    }

    pub fn list_lists_within(
        &self,
        args: &PageArgs,
        max_bytes: usize,
    ) -> Result<Page<ListSummary<'_>>, ReadError> {
        if args.include_deleted {
            return Err(ReadError::IncludeDeleted);
        }
        let binding = Binding::new(&["assistant_lists", &self.scope_key]);
        let after = cursor_id(args.cursor.as_deref(), &binding)?
            .map(|id| {
                self.state
                    .lists
                    .get(&id)
                    .map(|l| list_key(&self.state, l))
                    .ok_or(InvalidCursor)
            })
            .transpose()?;
        let limit = page_limit(args.limit);
        let items = self.state.lists.values();
        let selected = select(
            items,
            |_| true,
            |l| list_key(&self.state, l),
            after.as_ref(),
            limit + 1,
            || false,
        )
        .expect("local read without a cancellation check");
        page(
            selected,
            limit,
            &binding,
            |l| l.id,
            |l| self.list_summary(l),
            max_bytes,
        )
    }

    pub fn query_tasks(&self, args: &QueryArgs) -> Result<Page<TaskSummary<'_>>, ReadError> {
        self.query_tasks_within(args, usize::MAX)
    }

    pub fn query_tasks_within(
        &self,
        args: &QueryArgs,
        max_bytes: usize,
    ) -> Result<Page<TaskSummary<'_>>, ReadError> {
        let query = parse_query(args.query.as_deref().unwrap_or_default())?;
        let binding = Binding::new(&[
            "assistant_tasks",
            &self.scope_key,
            &query.to_string(),
            &self.ctx.today.to_string(),
            &self.ctx.utc_offset_seconds().to_string(),
        ]);
        let after = cursor_id(args.cursor.as_deref(), &binding)?
            .map(|id| {
                self.state
                    .tasks
                    .get(&id)
                    .map(|t| query.order_key(&self.state, t))
                    .ok_or(InvalidCursor)
            })
            .transpose()?;
        let limit = page_limit(args.limit);
        let matcher = query.matcher(&self.state, &self.ctx);
        let selected = select(
            self.state.tasks.values(),
            |t| matcher.matches_fields(t),
            |t| query.order_key(&self.state, t),
            after.as_ref(),
            limit + 1,
            || false,
        )
        .expect("local read without a cancellation check");
        page(
            selected,
            limit,
            &binding,
            |t| t.id,
            |t| self.task_summary(t),
            max_bytes,
        )
    }

    pub fn get_task(&self, args: &GetTaskArgs) -> Result<TaskDetail<'_>, ReadError> {
        if args.include_deleted {
            return Err(ReadError::IncludeDeleted);
        }
        let id = TaskId::from_uuid(args.task_id);
        let task = self.state.tasks.get(&id).ok_or(ReadError::NotFound)?;
        let steps = select(
            self.state.steps.values(),
            |s| s.task == id,
            step_key,
            None,
            MAX_DETAIL_CHILDREN + 1,
            || false,
        )
        .expect("local read without a cancellation check");
        let truncated = steps.len() > MAX_DETAIL_CHILDREN || task.tags.len() > MAX_DETAIL_CHILDREN;
        Ok(TaskDetail {
            task: self.task_summary(task),
            steps: steps
                .into_iter()
                .take(MAX_DETAIL_CHILDREN)
                .map(|(_, s)| StepSummary {
                    id: s.id,
                    title: &s.title,
                    completed: s.completed_at.is_some(),
                })
                .collect(),
            tags: task
                .tags
                .iter()
                .take(MAX_DETAIL_CHILDREN)
                .filter_map(|id| self.state.tags.get(id))
                .map(|t| TagSummary {
                    id: t.id,
                    name: &t.name,
                })
                .collect(),
            truncated,
        })
    }

    pub fn get_task_within(
        &self,
        args: &GetTaskArgs,
        max_bytes: usize,
    ) -> Result<TaskDetail<'_>, ReadError> {
        let mut detail = self.get_task(args)?;
        if json_len(&detail)? <= max_bytes {
            return Ok(detail);
        }
        let steps = std::mem::take(&mut detail.steps);
        let tags = std::mem::take(&mut detail.tags);
        detail.truncated = true;
        let required_bytes = json_len(&detail)?;
        let mut remaining = max_bytes
            .checked_sub(required_bytes)
            .ok_or(ReadError::TooLarge {
                required_bytes,
                max_bytes,
            })?;
        detail.steps = fit_children(steps, &mut remaining)?;
        detail.tags = fit_children(tags, &mut remaining)?;
        Ok(detail)
    }

    fn list_summary<'a>(&'a self, list: &'a List) -> ListSummary<'a> {
        let (open_count, completed_count) =
            self.list_counts.get(&list.id).copied().unwrap_or_default();
        ListSummary {
            id: list.id,
            name: &list.name,
            features: list.features,
            open_count,
            completed_count,
            collection: list
                .collection
                .and_then(|c| self.state.collections.get(&c))
                .map(|c| CollectionSummary {
                    id: c.id,
                    name: &c.name,
                }),
        }
    }

    fn task_summary<'a>(&self, t: &'a Task) -> TaskSummary<'a> {
        TaskSummary {
            id: t.id,
            list: t.list,
            title: &t.title,
            status: QueryStatus::of(t),
            start_date: t.start_date,
            due: t.due,
            priority: t.priority,
            in_my_day: t.my_day == Some(self.ctx.today),
            repeats: t.recurrence.is_some(),
        }
    }
}

// Local cursors expose only the last visible ID, not raw sort keys (e.g. private ranks). Recompute
// the key from the projection, so a supplied hidden ID is treated exactly like a missing ID.
fn cursor_id<T: FromStr>(raw: Option<&str>, binding: &Binding) -> Result<Option<T>, InvalidCursor> {
    decode_cursor(raw, binding)?
        .map(|key| match key.0.as_slice() {
            [KeyPart(KeyValue::Str(id), false)] => id.parse().map_err(|_| InvalidCursor),
            _ => Err(InvalidCursor),
        })
        .transpose()
}

fn page<'a, T, U: Serialize, Id: ToString>(
    selected: Vec<(OrderKey, &'a T)>,
    limit: usize,
    binding: &Binding,
    id: impl Fn(&T) -> Id,
    project: impl Fn(&'a T) -> U,
    max_bytes: usize,
) -> Result<Page<U>, ReadError> {
    let cursor_after = |taken: usize| {
        (selected.len() > taken).then(|| {
            let mut key = OrderKey::new();
            key.push_str(id(selected[taken - 1].1).to_string(), false);
            encode_cursor(binding, &key)
        })
    };
    let len = selected.len().min(limit);
    if max_bytes == usize::MAX {
        return Ok(Page {
            items: selected
                .iter()
                .take(len)
                .map(|(_, item)| project(item))
                .collect(),
            next_cursor: (len > 0).then(|| cursor_after(len)).flatten(),
        });
    }

    let empty = Page::<()> {
        items: Vec::new(),
        next_cursor: None,
    };
    let envelope = json_len(&empty)?;
    let mut items = Vec::new();
    let mut next_cursor = None;
    let mut fitted = 0;
    let mut required_bytes = envelope;
    let mut bytes = envelope;
    for (index, (_, item)) in selected.iter().take(len).enumerate() {
        let item = project(item);
        bytes += json_len(&item)? + usize::from(index > 0);
        let taken = index + 1;
        let cursor = cursor_after(taken);
        // Replace the envelope's null cursor with the actual JSON string when there is more data.
        let size = bytes - "null".len() + json_len(&cursor)?;
        if index == 0 {
            required_bytes = size;
        }
        if size <= max_bytes {
            fitted = taken;
            next_cursor = cursor;
        }
        if bytes > max_bytes {
            break;
        }
        items.push(item);
    }
    if (len > 0 && fitted == 0) || (len == 0 && envelope > max_bytes) {
        return Err(ReadError::TooLarge {
            required_bytes,
            max_bytes,
        });
    }
    items.truncate(fitted);
    Ok(Page { items, next_cursor })
}

fn json_len(value: &impl Serialize) -> Result<usize, serde_json::Error> {
    serde_json::to_vec(value).map(|bytes| bytes.len())
}

fn fit_children<T: Serialize>(
    mut children: Vec<T>,
    remaining: &mut usize,
) -> Result<Vec<T>, serde_json::Error> {
    let mut fitted = 0;
    for (index, child) in children.iter().enumerate() {
        let size = json_len(child)? + usize::from(index > 0);
        let Some(left) = remaining.checked_sub(size) else {
            break;
        };
        *remaining = left;
        fitted += 1;
    }
    children.truncate(fitted);
    Ok(children)
}
