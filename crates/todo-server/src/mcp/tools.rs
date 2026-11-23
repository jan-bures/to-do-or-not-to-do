//! The MCP tools.
//!
//! Each tool is a thin adapter over a REST handler: it builds the same request the REST client would
//! send and calls the handler function directly, with the caller and the request's [`Cancel`] that the
//! HTTP middleware attached. So admission permits, the write quota, the access checks in `todo-core`,
//! the read visibility filters, the query caps and pagination are the REST ones, by construction.
//!
//! | Tool | Command or endpoint | Level needed (agents) |
//! |---|---|---|
//! | `list_collections` | `GET /collections` | read |
//! | `list_lists` | `GET /lists` | read |
//! | `list_tags` | `GET /tags` | read (tags on tasks you can read, or all with a default of read) |
//! | `get_task` | `GET /tasks/{id}` | read |
//! | `query_tasks` | `GET /tasks?query=` | read |
//! | `create_task` | `create_task` | capture; plan to set `recurrence` |
//! | `add_note` | `add_note` | capture |
//! | `add_link` | `add_link` (web links only) | capture |
//! | `update_task` | `update_task` | plan |
//! | `complete_task` | `complete_task` / `uncomplete_task` | plan |
//! | `move_task` | `move_task` | plan, on both lists |
//! | `add_step` | `create_step` | plan |
//! | `update_step` | `update_step` or `complete_step` / `uncomplete_step` (one per call) | plan |
//! | `set_my_day` | `set_my_day` | plan |
//! | `tag_task` / `untag_task` | `add_tag` / `remove_tag` | plan |
//! | `create_tag` | `create_tag` | plan on at least one list, and read by default |
//! | `add_reminder` | `add_reminder` | plan |
//! | `update_reminder` | `update_reminder` | plan |
//! | `merge_tasks` | `merge_tasks` | plan, on both tasks |
//! | `archive_task` | `delete` (a soft delete: a tombstone, which `undo_event` reverses) | plan |
//! | `get_activity` | `GET /activity` | read (events filtered) |
//! | `undo_event` | `POST /events/{seq}/undo` | the level of the change; own events only |
//!
//! # Results and errors
//!
//! A successful tool returns the REST response body as structured content (and the same JSON as
//! text). A write returns `command_id`, `events` (`[{id, seq}]`), `summary` (the activity-feed line),
//! `epoch` (the log it is in), `replayed`, and the IDs of anything it created. `undo_event` takes the
//! event's `seq` and `id` (and optionally `epoch`), like `POST /events/{seq}/undo`, so an undo can never
//! hit another event after a restore. A failure is a tool error (`isError: true`) whose
//! structured content is the REST error envelope, `{"error": {"code", "message", "details"}}`, with the
//! same codes (`forbidden`, `not_found`, `undo_conflict`, `quota_exceeded`, …) plus `retry_after` in
//! seconds where REST sends `Retry-After`.
//!
//! # Idempotency
//!
//! Every write takes an optional `command_id` (a UUID); without one, a fresh one is generated per call.
//! The IDs of entities a write creates are derived from the `command_id`, so retrying with the same
//! `command_id` and arguments replays the stored result (`replayed: true`) if the command committed.
//! Defaults a tool resolves itself (the Inbox for `create_task`, "today" for `set_my_day`) are taken
//! from the stored record on a retry, even if lists were renamed or the day changed in between.
//! Reusing a committed ID with different arguments gets `command_id_reused`.
//!
//! A rejected command is not stored. For example, after `move_source_changed`, the same ID can
//! succeed if the source precondition holds on a later retry.
//!
//! Hidden targets answer like missing ones (`not_found`, `event_not_found`), also on a retry; see
//! [`crate::api::hidden`].

use crate::api::commands::{self, REPLAYED_HEADER};
use crate::api::dto::{
    ActivityParams, CommandRequest, IncludeDeleted, ReadParams, TaskQueryParams,
};
use crate::api::error::ApiError;
use crate::api::read::{self, MAX_PAGE_BYTES};
use crate::api::{ApiJson, ApiPath, ApiQuery, RequestCancel, Work, blocking};
use crate::app::AppState;
use crate::auth::Authenticated;
use crate::cancel::Cancel;
use crate::engine::CommandResult;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::Response;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::future::Future;
use todo_core::AccessLevel;
use todo_core::access::{Target, level_on};
use todo_core::command::Command;
use todo_core::event::EventPayload;
use todo_core::ids::{LinkId, ListId, ReminderId, StepId, TagId, TaskId};
use todo_core::model::Entity;
use todo_core::time::Timestamp;
use todo_tools::{
    args::*,
    command::{CommandError, reminder_time, resolve_inbox},
    descriptor as tools,
    read::{project_list, project_task},
};
use uuid::Uuid;

/// What the server tells agents when they connect (the full guide is `docs/agents/`).
pub const INSTRUCTIONS: &str = "\
A personal to-do list. You act with the levels the owner granted your token, per list: read, capture \
(create tasks, add notes and links) or plan (change, complete, move, merge and archive tasks; steps, \
My Day, tags, reminders and repeat rules). Lists you cannot read do not exist for you. Create tasks in the Inbox unless told otherwise. Before \
creating a task, query for an existing one (query_tasks) to avoid duplicates. Pass the same command_id \
when retrying a write, so it is not applied twice. Every write is attributed to you, shows in the \
owner's activity feed and can be undone. Check list_lists for features.workflow; with plan access, \
set status to in_progress when starting work on a workflow list. Use complete_task for Done.";

// ---- plumbing ---------------------------------------------------------------------------------------

/// The caller and the request's cancellation, attached by the HTTP middleware.
fn caller(ctx: &RequestContext<RoleServer>) -> Result<(Authenticated, Cancel), ApiError> {
    let parts = ctx
        .extensions
        .get::<Parts>()
        .ok_or_else(ApiError::unauthorized)?;
    let auth = parts
        .extensions
        .get::<Authenticated>()
        .cloned()
        .ok_or_else(ApiError::unauthorized)?;
    let cancel = parts
        .extensions
        .get::<Cancel>()
        .cloned()
        .unwrap_or_default();
    Ok((auth, cancel))
}

/// The REST error envelope, plus `retry_after`.
fn error_body(e: &ApiError) -> Value {
    let mut err = json!({ "code": e.code, "message": e.message });
    if let Some(d) = &e.details {
        err["details"] = d.clone();
    }
    if let Some(s) = e.retry_after {
        err["retry_after"] = json!(s);
    }
    json!({ "error": err })
}

fn tool_result(tool: &'static str, r: Result<Value, ApiError>) -> CallToolResult {
    match r {
        Ok(v) => {
            tracing::debug!(tool, "mcp tool call succeeded");
            let mut res = CallToolResult::success(vec![ContentBlock::text(v.to_string())]);
            res.structured_content = Some(v);
            res
        }
        Err(e) => {
            tracing::debug!(tool, code = e.code, "mcp tool call failed");
            let body = error_body(&e);
            let mut res = CallToolResult::error(vec![ContentBlock::text(body.to_string())]);
            res.structured_content = Some(body);
            res
        }
    }
}

impl From<CommandError> for ApiError {
    fn from(error: CommandError) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_body",
            error.to_string(),
        )
    }
}

fn to_value(v: &impl Serialize) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| {
        tracing::error!(error = %e, "cannot encode a tool result");
        ApiError::internal()
    })
}

async fn body_json(res: Response) -> Result<Value, ApiError> {
    // The handlers already cap their bodies at MAX_PAGE_BYTES.
    let bytes = axum::body::to_bytes(res.into_body(), MAX_PAGE_BYTES + 1024)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "cannot read a handler response");
            ApiError::internal()
        })?;
    serde_json::from_slice(&bytes).map_err(|e| {
        tracing::error!(error = %e, "a handler response is not JSON");
        ApiError::internal()
    })
}

/// An ID derived from a command ID, so a retry recreates the same entity.
#[must_use]
pub fn derived_id(command_id: Uuid, what: &str) -> Uuid {
    let mut h = Sha256::new();
    h.update(b"todo-mcp\x1f");
    h.update(command_id.as_bytes());
    h.update([0x1f]);
    h.update(what.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    Uuid::new_v8(b)
}

/// A write's result: the stored command result, the log epoch, whether it was replayed, and `extra`
/// fields.
async fn outcome(res: Response, extra: Value) -> Result<Value, ApiError> {
    let replayed = res.headers().contains_key(REPLAYED_HEADER);
    let mut v = body_json(res).await?;
    let epoch = v.get_mut("epoch").map(Value::take).unwrap_or(Value::Null);
    let r: CommandResult = serde_json::from_value(v).map_err(|e| {
        tracing::error!(error = %e, "unexpected command result");
        ApiError::internal()
    })?;
    let mut out = json!({
        "command_id": r.command_id,
        "events": r.events,
        "summary": r.summary,
        "epoch": epoch,
        "replayed": replayed,
    });
    if let (Value::Object(o), Value::Object(e)) = (&mut out, extra) {
        o.extend(e);
    }
    Ok(out)
}

/// Runs one command through `POST /api/v1/commands`.
async fn command(
    app: &AppState,
    auth: Authenticated,
    cancel: Cancel,
    command_id: Uuid,
    command: Command,
    extra: Value,
) -> Result<Value, ApiError> {
    let res = commands::post_command(
        auth,
        State(app.clone()),
        RequestCancel(cancel),
        ApiJson(CommandRequest {
            command_id,
            command,
        }),
    )
    .await?;
    outcome(res, extra).await
}

/// The list named "Inbox" (case- and diacritic-insensitively) that the caller can capture into: a
/// top-level one first, then by position.
async fn inbox(app: &AppState, auth: &Authenticated) -> Result<ListId, ApiError> {
    let engine = app.engine.clone();
    let actor = auth.actor.clone();
    blocking(app, Work::Read, move || {
        let state = engine.state();
        resolve_inbox(&state, |l| {
            level_on(&state, &actor, Target::List(l.id)) >= AccessLevel::Capture
        })
    })
    .await?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "inbox_not_found",
            "no list named Inbox that you can capture into; pass list_id (see list_lists)",
        )
    })
}

/// The caller's stored command `command_id`, if this is a retry: its result and when it was handled.
async fn stored(
    app: &AppState,
    auth: &Authenticated,
    command_id: Option<Uuid>,
) -> Result<Option<(CommandResult, Timestamp)>, ApiError> {
    let Some(id) = command_id else {
        return Ok(None);
    };
    let engine = app.engine.clone();
    let actor = auth.actor.clone();
    Ok(blocking(app, Work::Read, move || engine.stored_command(&actor, id)).await??)
}

/// The list a stored `create_task` put `task` in (from its `Created` event).
async fn stored_list(
    app: &AppState,
    result: &CommandResult,
    task: TaskId,
) -> Result<Option<ListId>, ApiError> {
    let engine = app.engine.clone();
    let seqs: Vec<u64> = result.events.iter().map(|e| e.seq).collect();
    blocking(app, Work::Read, move || -> Result<_, ApiError> {
        for seq in seqs {
            for e in engine.events_since(seq.saturating_sub(1), 1)? {
                if let EventPayload::Created {
                    entity: Entity::Task(t),
                } = &e.payload
                    && t.id == task
                {
                    return Ok(Some(t.list));
                }
            }
        }
        Ok(None)
    })
    .await?
}

// ---- the server ---------------------------------------------------------------------------------------

/// The MCP server handler: one per request (the transport is stateless), cloned from a prototype.
#[derive(Clone)]
pub struct TodoMcp {
    app: AppState,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for TodoMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TodoMcp").finish_non_exhaustive()
    }
}

impl TodoMcp {
    /// A handler over `app`.
    #[must_use]
    pub fn new(app: AppState) -> Self {
        Self {
            app,
            tool_router: Self::tool_router(),
        }
    }

    /// The names of all tools.
    #[must_use]
    pub fn tool_names() -> Vec<String> {
        let mut v: Vec<String> = Self::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        v.sort();
        v
    }

    /// Runs a tool body with the caller and turns the outcome into a tool result.
    async fn run<F, Fut>(
        &self,
        tool: &'static str,
        ctx: &RequestContext<RoleServer>,
        f: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        F: FnOnce(AppState, Authenticated, Cancel) -> Fut,
        Fut: Future<Output = Result<Value, ApiError>>,
    {
        let r = match caller(ctx) {
            Ok((auth, cancel)) => f(self.app.clone(), auth, cancel).await,
            Err(e) => Err(e),
        };
        Ok(tool_result(tool, r))
    }
}

#[tool_router]
impl TodoMcp {
    #[tool(
        description = tools::LIST_COLLECTIONS.description,
        input_schema = tools::LIST_COLLECTIONS.input_schema(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_collections(
        &self,
        Parameters(a): Parameters<PageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::LIST_COLLECTIONS.name,
            &ctx,
            |app, auth, cancel| async move {
                let page = read::collections(
                    auth,
                    State(app),
                    RequestCancel(cancel),
                    ApiQuery(ReadParams {
                        include_deleted: a.include_deleted,
                        limit: a.limit,
                        cursor: a.cursor,
                    }),
                )
                .await?;
                to_value(&page.0)
            },
        )
        .await
    }

    #[tool(
        description = tools::LIST_LISTS.description,
        input_schema = tools::LIST_LISTS.input_schema(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_lists(
        &self,
        Parameters(a): Parameters<PageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::LIST_LISTS.name,
            &ctx,
            |app, auth, cancel| async move {
                let page = read::lists(
                    auth,
                    State(app),
                    RequestCancel(cancel),
                    ApiQuery(ReadParams {
                        include_deleted: a.include_deleted,
                        limit: a.limit,
                        cursor: a.cursor,
                    }),
                )
                .await?;
                let mut page = to_value(&page.0)?;
                if let Some(items) = page["items"].as_array_mut() {
                    items.iter_mut().for_each(project_list);
                }
                Ok(page)
            },
        )
        .await
    }

    #[tool(
        description = tools::LIST_TAGS.description,
        input_schema = tools::LIST_TAGS.input_schema(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_tags(
        &self,
        Parameters(a): Parameters<PageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::LIST_TAGS.name,
            &ctx,
            |app, auth, cancel| async move {
                let page = read::tags(
                    auth,
                    State(app),
                    RequestCancel(cancel),
                    ApiQuery(ReadParams {
                        include_deleted: a.include_deleted,
                        limit: a.limit,
                        cursor: a.cursor,
                    }),
                )
                .await?;
                to_value(&page.0)
            },
        )
        .await
    }

    #[tool(
        description = tools::GET_TASK.description,
        input_schema = tools::GET_TASK.input_schema(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_task(
        &self,
        Parameters(a): Parameters<GetTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(tools::GET_TASK.name, &ctx, |app, auth, cancel| async move {
            let res = read::task_detail(
                auth,
                State(app),
                RequestCancel(cancel),
                ApiPath(TaskId::from_uuid(a.task_id)),
                ApiQuery(IncludeDeleted {
                    include_deleted: a.include_deleted,
                }),
            )
            .await?;
            let mut detail = body_json(res).await?;
            if let Some(task) = detail.get_mut("task") {
                project_task(task);
            }
            Ok(detail)
        })
        .await
    }

    #[tool(
        description = tools::QUERY_TASKS.description,
        input_schema = tools::QUERY_TASKS.input_schema(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn query_tasks(
        &self,
        Parameters(a): Parameters<QueryArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::QUERY_TASKS.name,
            &ctx,
            |app, auth, cancel| async move {
                let page = read::query_tasks(
                    auth,
                    State(app),
                    RequestCancel(cancel),
                    ApiQuery(TaskQueryParams {
                        query: a.query,
                        limit: a.limit,
                        cursor: a.cursor,
                    }),
                )
                .await?;
                let mut page = to_value(&page.0)?;
                if let Some(items) = page["items"].as_array_mut() {
                    items.iter_mut().for_each(project_task);
                }
                Ok(page)
            },
        )
        .await
    }

    #[tool(
        description = tools::CREATE_TASK.description,
        input_schema = tools::CREATE_TASK.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_task(
        &self,
        Parameters(a): Parameters<CreateTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::CREATE_TASK.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                let task = TaskId::from_uuid(derived_id(command_id, "task"));
                let list = match a.list_id {
                    Some(l) => ListId::from_uuid(l),
                    // A retry uses the Inbox the first attempt resolved, even if lists changed since, so
                    // it sends the same command and replays.
                    None => match stored(&app, &auth, a.command_id).await? {
                        Some((r, _)) => match stored_list(&app, &r, task).await? {
                            Some(l) => l,
                            None => inbox(&app, &auth).await?,
                        },
                        None => inbox(&app, &auth).await?,
                    },
                };
                let cmd = a.into_command(task, list)?;
                let extra = json!({ "task_id": task, "list_id": list });
                command(&app, auth, cancel, command_id, cmd, extra).await
            },
        )
        .await
    }

    #[tool(
        description = tools::ADD_NOTE.description,
        input_schema = tools::ADD_NOTE.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn add_note(
        &self,
        Parameters(a): Parameters<AddNoteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(tools::ADD_NOTE.name, &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            command(&app, auth, cancel, command_id, a.into_command(), json!({})).await
        })
        .await
    }

    #[tool(
        description = tools::ADD_LINK.description,
        input_schema = tools::ADD_LINK.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn add_link(
        &self,
        Parameters(a): Parameters<AddLinkArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(tools::ADD_LINK.name, &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let link = derived_id(command_id, "link");
            command(
                &app,
                auth,
                cancel,
                command_id,
                a.into_command(LinkId::from_uuid(link)),
                json!({ "link_id": link }),
            )
            .await
        })
        .await
    }

    #[tool(
        description = tools::UPDATE_TASK.description,
        input_schema = tools::UPDATE_TASK.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn update_task(
        &self,
        Parameters(a): Parameters<UpdateTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::UPDATE_TASK.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                command(&app, auth, cancel, command_id, a.into_command()?, json!({})).await
            },
        )
        .await
    }

    #[tool(
        description = tools::COMPLETE_TASK.description,
        input_schema = tools::COMPLETE_TASK.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn complete_task(
        &self,
        Parameters(a): Parameters<CompleteTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::COMPLETE_TASK.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                command(&app, auth, cancel, command_id, a.into_command(), json!({})).await
            },
        )
        .await
    }

    #[tool(
        description = tools::MOVE_TASK.description,
        input_schema = tools::MOVE_TASK.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn move_task(
        &self,
        Parameters(a): Parameters<MoveTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::MOVE_TASK.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                command(&app, auth, cancel, command_id, a.into_command(), json!({})).await
            },
        )
        .await
    }

    #[tool(
        description = tools::ADD_STEP.description,
        input_schema = tools::ADD_STEP.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn add_step(
        &self,
        Parameters(a): Parameters<AddStepArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(tools::ADD_STEP.name, &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            let step = derived_id(command_id, "step");
            command(
                &app,
                auth,
                cancel,
                command_id,
                a.into_command(StepId::from_uuid(step)),
                json!({ "step_id": step }),
            )
            .await
        })
        .await
    }

    #[tool(
        description = tools::UPDATE_STEP.description,
        input_schema = tools::UPDATE_STEP.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn update_step(
        &self,
        Parameters(a): Parameters<UpdateStepArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::UPDATE_STEP.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                command(&app, auth, cancel, command_id, a.into_command()?, json!({})).await
            },
        )
        .await
    }

    #[tool(
        description = tools::SET_MY_DAY.description,
        input_schema = tools::SET_MY_DAY.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn set_my_day(
        &self,
        Parameters(a): Parameters<SetMyDayArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::SET_MY_DAY.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                // A retry without a date uses "today" as of its first attempt.
                let stored_at = if a.in_my_day && a.date.is_none() {
                    stored(&app, &auth, a.command_id).await?.map(|(_, at)| at)
                } else {
                    None
                };
                let explicit = a.date.clone().filter(|_| a.in_my_day);
                let cmd = a.into_command(|| {
                    let now = stored_at.unwrap_or_else(|| app.engine.now());
                    jiff::ZonedDateTime::new(now, app.zone.clone()).date()
                })?;
                let Command::SetMyDay { date, .. } = &cmd else {
                    unreachable!("My Day mapper must return SetMyDay");
                };
                let date = explicit.map_or_else(|| json!(date), Value::String);
                command(&app, auth, cancel, command_id, cmd, json!({ "date": date })).await
            },
        )
        .await
    }

    #[tool(
        description = tools::TAG_TASK.description,
        input_schema = tools::TAG_TASK.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn tag_task(
        &self,
        Parameters(a): Parameters<TagTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(tools::TAG_TASK.name, &ctx, |app, auth, cancel| async move {
            let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
            command(
                &app,
                auth,
                cancel,
                command_id,
                a.into_command(true),
                json!({}),
            )
            .await
        })
        .await
    }

    #[tool(
        description = tools::UNTAG_TASK.description,
        input_schema = tools::UNTAG_TASK.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn untag_task(
        &self,
        Parameters(a): Parameters<TagTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::UNTAG_TASK.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                command(
                    &app,
                    auth,
                    cancel,
                    command_id,
                    a.into_command(false),
                    json!({}),
                )
                .await
            },
        )
        .await
    }

    #[tool(
        description = tools::CREATE_TAG.description,
        input_schema = tools::CREATE_TAG.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_tag(
        &self,
        Parameters(a): Parameters<CreateTagArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::CREATE_TAG.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                let tag = derived_id(command_id, "tag");
                command(
                    &app,
                    auth,
                    cancel,
                    command_id,
                    a.into_command(TagId::from_uuid(tag)),
                    json!({ "tag_id": tag }),
                )
                .await
            },
        )
        .await
    }

    #[tool(
        description = tools::ADD_REMINDER.description,
        input_schema = tools::ADD_REMINDER.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn add_reminder(
        &self,
        Parameters(a): Parameters<AddReminderArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::ADD_REMINDER.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                let at = reminder_time(&a.at, &app.zone)?;
                let reminder = derived_id(command_id, "reminder");
                command(
                    &app,
                    auth,
                    cancel,
                    command_id,
                    a.into_command(ReminderId::from_uuid(reminder), at),
                    json!({ "reminder_id": reminder, "at": at }),
                )
                .await
            },
        )
        .await
    }

    #[tool(
        description = tools::UPDATE_REMINDER.description,
        input_schema = tools::UPDATE_REMINDER.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn update_reminder(
        &self,
        Parameters(a): Parameters<UpdateReminderArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::UPDATE_REMINDER.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                let at = reminder_time(&a.at, &app.zone)?;
                command(
                    &app,
                    auth,
                    cancel,
                    command_id,
                    a.into_command(at),
                    json!({ "at": at }),
                )
                .await
            },
        )
        .await
    }

    #[tool(
        description = tools::MERGE_TASKS.description,
        input_schema = tools::MERGE_TASKS.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = false
        )
    )]
    async fn merge_tasks(
        &self,
        Parameters(a): Parameters<MergeTasksArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::MERGE_TASKS.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                command(&app, auth, cancel, command_id, a.into_command(), json!({})).await
            },
        )
        .await
    }

    #[tool(
        description = tools::ARCHIVE_TASK.description,
        input_schema = tools::ARCHIVE_TASK.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = false
        )
    )]
    async fn archive_task(
        &self,
        Parameters(a): Parameters<ArchiveTaskArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::ARCHIVE_TASK.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                command(&app, auth, cancel, command_id, a.into_command(), json!({})).await
            },
        )
        .await
    }

    #[tool(
        description = tools::GET_ACTIVITY.description,
        input_schema = tools::GET_ACTIVITY.input_schema(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_activity(
        &self,
        Parameters(a): Parameters<ActivityArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::GET_ACTIVITY.name,
            &ctx,
            |app, auth, _cancel| async move {
                let page = read::activity(
                    auth,
                    State(app),
                    ApiQuery(ActivityParams {
                        before: a.before,
                        limit: a.limit,
                        agent: None,
                        workflow_fields: true,
                    }),
                )
                .await?;
                to_value(&page.0)
            },
        )
        .await
    }

    #[tool(
        description = tools::UNDO_EVENT.description,
        input_schema = tools::UNDO_EVENT.input_schema(),
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            open_world_hint = false
        )
    )]
    async fn undo_event(
        &self,
        Parameters(a): Parameters<UndoArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(
            tools::UNDO_EVENT.name,
            &ctx,
            |app, auth, cancel| async move {
                let command_id = a.command_id.unwrap_or_else(Uuid::new_v4);
                let body = serde_json::to_vec(&json!({
                    "command_id": command_id,
                    "force": a.force,
                    "expected_event_id": a.event_id,
                    "epoch": a.epoch,
                }))
                .map_err(|_| ApiError::internal())?;
                let res = commands::post_undo(
                    auth,
                    State(app),
                    RequestCancel(cancel),
                    ApiPath(a.seq),
                    Bytes::from(body),
                )
                .await?;
                outcome(res, json!({})).await
            },
        )
        .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TodoMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("todo", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_are_stable_and_distinct() {
        let c = Uuid::new_v4();
        assert_eq!(derived_id(c, "task"), derived_id(c, "task"));
        assert_ne!(derived_id(c, "task"), derived_id(c, "step"));
        assert_ne!(derived_id(c, "task"), derived_id(Uuid::new_v4(), "task"));
    }

    #[test]
    fn errors_carry_the_rest_envelope() {
        let e = ApiError::quota_exceeded(7);
        let r = tool_result("t", Err(e));
        assert_eq!(r.is_error, Some(true));
        let body = r.structured_content.unwrap();
        assert_eq!(body["error"]["code"], "quota_exceeded");
        assert_eq!(body["error"]["retry_after"], 7);
    }

    #[test]
    fn recurrence_args_are_the_domain_shape() {
        use todo_core::recurrence::Recurrence;
        for (arg, rrule) in [
            (
                json!({ "rule": { "freq": "daily" } }),
                "FREQ=DAILY;INTERVAL=1",
            ),
            (
                json!({ "rule": { "freq": "weekly", "interval": 2, "days": ["mon", "fri"] } }),
                "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,FR",
            ),
            (
                json!({ "rule": { "freq": "monthly", "day": 31 }, "until": "2031-12-31" }),
                "FREQ=MONTHLY;INTERVAL=1;BYMONTHDAY=31;UNTIL=20311231",
            ),
            (
                json!({ "rule": { "freq": "yearly", "interval": 3 } }),
                "FREQ=YEARLY;INTERVAL=3",
            ),
        ] {
            let a: RecurrenceArg = serde_json::from_value(arg.clone()).unwrap();
            let r: Recurrence = serde_json::from_value(json!(a)).unwrap();
            r.validate().unwrap();
            assert_eq!(r.to_rrule(), rrule, "{arg}");
        }
        // Validation is the domain's: the tool passes out-of-range values through.
        let a: RecurrenceArg =
            serde_json::from_value(json!({ "rule": { "freq": "weekly", "days": [] } })).unwrap();
        let r: Recurrence = serde_json::from_value(json!(a)).unwrap();
        assert!(r.validate().is_err());
        // Misspelt parts are refused rather than dropped.
        for bad in [
            json!({ "rule": { "freq": "weekly", "byday": ["mon"] } }),
            json!({ "rule": { "freq": "daily" }, "count": 3 }),
            json!({ "rule": { "freq": "hourly" } }),
        ] {
            assert!(
                serde_json::from_value::<RecurrenceArg>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn every_tool_has_a_description_and_an_object_schema() {
        let names = TodoMcp::tool_names();
        assert_eq!(names.len(), 23, "{names:?}");
        for t in TodoMcp::tool_router().list_all() {
            assert!(
                t.description.as_deref().is_some_and(|d| d.len() > 20),
                "{}",
                t.name
            );
            assert_eq!(
                t.input_schema.get("type"),
                Some(&json!("object")),
                "{}",
                t.name
            );
        }
    }
}
