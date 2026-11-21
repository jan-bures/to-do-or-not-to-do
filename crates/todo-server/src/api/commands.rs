//! Writes: commands and undo.

use super::dto::{CommandRequest, UndoRequest};
use super::error::{ApiError, ErrorBody};
use super::{ApiJson, ApiPath, RequestCancel, Work, blocking};
use crate::app::AppState;
use crate::auth::Authenticated;
use crate::engine::{CommandResult, Outcome};
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Header set on a response that replays the stored result of a retried command.
pub const REPLAYED_HEADER: &str = "idempotent-replayed";

fn outcome_response(o: Outcome) -> Response {
    let mut res = Json(o.result).into_response();
    if o.replayed {
        res.headers_mut()
            .insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
    }
    res
}

/// Runs a command.
#[utoipa::path(
    post,
    path = "/api/v1/commands",
    tag = "commands",
    request_body = CommandRequest,
    responses(
        (status = 200, description = "Stored (or replayed: see the `idempotent-replayed` header)", body = CommandResult),
        (status = 401, description = "Missing or invalid token", body = ErrorBody),
        (status = 403, description = "Not allowed", body = ErrorBody),
        (status = 404, description = "A referenced entity does not exist", body = ErrorBody),
        (status = 409, description = "Conflicts with the current state, or the command ID was reused", body = ErrorBody),
        (status = 408, description = "The deadline passed before the write began committing; nothing was stored", body = ErrorBody),
        (status = 413, description = "Body too large", body = ErrorBody),
        (status = 422, description = "Invalid command", body = ErrorBody),
        (status = 503, description = "Too many writes in flight (`overloaded`), or the deadline passed while the write was committing (`outcome_unknown`: retry with the same `command_id`); see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn post_command(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiJson(req): ApiJson<CommandRequest>,
) -> Result<Response, ApiError> {
    let engine = app.engine.clone();
    // The deadline middleware owns `cancel`: it either cancels the write before it commits (408) or
    // waits for the commit (see `crate::app`).
    let outcome = blocking(&app, Work::Write, move || {
        engine.execute_cancellable(&auth.actor, req.command_id, &req.command, &cancel)
    })
    .await??;
    Ok(outcome_response(outcome))
}

/// Undoes the command that produced an event. Undoing a compensating event redoes the change.
#[utoipa::path(
    post,
    path = "/api/v1/events/{seq}/undo",
    tag = "commands",
    params(("seq" = u64, Path, description = "Sequence number of any event of the command to undo")),
    request_body(content = UndoRequest, description = "`command_id` is required; `force` defaults to false"),
    responses(
        (status = 200, description = "Compensating events stored (or replayed: see the `idempotent-replayed` header)", body = CommandResult),
        (status = 400, description = "No `command_id` (`command_id_required`)", body = ErrorBody),
        (status = 401, description = "Missing or invalid token", body = ErrorBody),
        (status = 404, description = "No such event", body = ErrorBody),
        (status = 409, description = "Later changes conflict (`undo_conflict`), or already undone", body = ErrorBody),
        (status = 408, description = "The deadline passed before the undo began committing; nothing was stored", body = ErrorBody),
        (status = 503, description = "Too many writes in flight (`overloaded`), or the outcome is unknown (`outcome_unknown`: retry with the same `command_id`); see `Retry-After`", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn post_undo(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiPath(seq): ApiPath<u64>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let req: UndoRequest = if body.iter().all(u8::is_ascii_whitespace) {
        UndoRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_body",
                e.to_string(),
            )
        })?
    };
    // Without a client-chosen ID, a lost response (time-out, `outcome_unknown`) could not be retried
    // without risking a second undo.
    let command_id = req.command_id.ok_or_else(ApiError::command_id_required)?;
    let engine = app.engine.clone();
    let outcome = blocking(&app, Work::Write, move || {
        engine.undo_cancellable(&auth.actor, seq, req.force, Some(command_id), &cancel)
    })
    .await??;
    Ok(outcome_response(outcome))
}
