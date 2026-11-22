//! Writes: commands and undo.

use super::dto::{CommandRequest, CommandResponse, UndoRequest};
use super::error::{ApiError, ErrorBody, undo_error};
use super::{ApiJson, ApiPath, RequestCancel, Work, blocking, hidden};
use crate::app::AppState;
use crate::auth::Authenticated;
use crate::engine::{Engine, EngineError, Outcome, UndoExpect};
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::collections::BTreeMap;
use todo_core::Actor;
use todo_core::error::{UndoConflict, UndoError};
use todo_core::ids::EventId;

/// Header set on a response that replays the stored result of a retried command.
pub const REPLAYED_HEADER: &str = "idempotent-replayed";

fn outcome_response(o: Outcome, epoch: uuid::Uuid) -> Response {
    let mut res = Json(CommandResponse {
        result: o.result,
        epoch,
    })
    .into_response();
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
        (status = 200, description = "Stored (or replayed: see the `idempotent-replayed` header)", body = CommandResponse),
        (status = 401, description = "Missing or invalid token", body = ErrorBody),
        (status = 403, description = "Not allowed", body = ErrorBody),
        (status = 404, description = "A referenced entity does not exist", body = ErrorBody),
        (status = 409, description = "Conflicts with the current state (e.g. `move_source_changed` for a stale guarded move, `id_unavailable` when an agent chooses a taken ID), or the command ID was reused", body = ErrorBody),
        (status = 408, description = "The deadline passed before the write began committing; nothing was stored", body = ErrorBody),
        (status = 413, description = "Body too large", body = ErrorBody),
        (status = 422, description = "Invalid command", body = ErrorBody),
        (status = 429, description = "Agent write quota used up (`quota_exceeded`) or too many requests; see `Retry-After`", body = ErrorBody),
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
    auth.take_write(&app)?;
    let engine = app.engine.clone();
    // The deadline middleware owns `cancel`: it either cancels the write before it commits (408) or
    // waits for the commit (see `crate::app`).
    let outcome = blocking(&app, Work::Write, move || {
        // Hidden and missing targets look the same (see `hidden`), also for a retry. The snapshot
        // check refuses early; the one inside the writer decides.
        hidden::check_command(&engine.state(), &auth.actor, &req.command)?;
        engine
            .execute_checked(
                &auth.actor,
                req.command_id,
                &req.command,
                &cancel,
                hidden::command_check(&auth.actor, &req.command),
            )
            .map_err(|e| hidden::write_error(&auth.actor, e))
    })
    .await??;
    Ok(outcome_response(outcome, app.engine.epoch()))
}

/// Undoes the command that produced an event. Undoing a compensating event redoes the change.
#[utoipa::path(
    post,
    path = "/api/v1/events/{seq}/undo",
    tag = "commands",
    params(("seq" = u64, Path, description = "Sequence number of any event of the command to undo")),
    request_body(content = UndoRequest, description = "`command_id` and `expected_event_id` are required; `force` defaults to false"),
    responses(
        (status = 200, description = "Compensating events stored (or replayed: see the `idempotent-replayed` header)", body = CommandResponse),
        (status = 400, description = "No `command_id` (`command_id_required`) or `expected_event_id` (`expected_event_id_required`)", body = ErrorBody),
        (status = 401, description = "Missing or invalid token", body = ErrorBody),
        (status = 404, description = "No such event", body = ErrorBody),
        (status = 409, description = "Later changes conflict (`undo_conflict`), or already undone; another event is at `seq` (`event_mismatch`); the log has another epoch (`epoch_mismatch`)", body = ErrorBody),
        (status = 429, description = "Agent write quota used up (`quota_exceeded`) or too many requests; see `Retry-After`", body = ErrorBody),
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
    // Without the expected event, an undo queued before a restore could undo whatever event now has
    // that sequence number.
    let expect = UndoExpect {
        event_id: req
            .expected_event_id
            .ok_or_else(ApiError::expected_event_id_required)?,
        epoch: req.epoch,
    };
    auth.take_write(&app)?;
    // The event ID found at `seq` is checked before access is: an agent must not learn it (it would
    // date events in lists the agent cannot read).
    let agent = matches!(auth.actor, todo_core::Actor::Agent { .. });
    let engine = app.engine.clone();
    let outcome = blocking(&app, Work::Write, move || {
        // Invisible and missing events look the same (see `hidden`), also for a retry: checked inside
        // the writer, before the idempotency lookup and the expected-event check.
        let done = engine.undo_checked(
            &auth.actor,
            seq,
            req.force,
            Some(command_id),
            Some(expect),
            &cancel,
            hidden::undo_check(&auth.actor, seq),
        );
        done.map_err(|e| match e {
            EngineError::UndoTargetMismatch { seq, .. } if agent => ApiError::new(
                StatusCode::CONFLICT,
                "event_mismatch",
                format!("event {seq} is not the expected event"),
            )
            .with_details(serde_json::json!({ "seq": seq })),
            EngineError::Undo {
                error: UndoError::Conflict(c),
                seqs,
            } if agent => visible_conflict(&engine, &auth.actor, c, &seqs),
            e => e.into(),
        })
    })
    .await??;
    Ok(outcome_response(outcome, app.engine.epoch()))
}

/// An undo conflict as an agent may see it: only the later events it could read in the event log.
/// If it could read none, the conflict is reported without any references.
fn visible_conflict(
    engine: &Engine,
    actor: &Actor,
    mut conflict: UndoConflict,
    seqs: &BTreeMap<EventId, u64>,
) -> ApiError {
    let state = engine.state();
    let gone = engine.whereabouts();
    let visible = |id: &EventId| {
        let Some(&seq) = seqs.get(id) else {
            return false;
        };
        match engine.event_with_scope(seq) {
            Ok(Some((event, scope))) => {
                super::visibility::event_visible(&state, &gone, actor, &event, scope.as_ref())
            }
            _ => false,
        }
    };
    for f in &mut conflict.conflicts {
        f.later_events.retain(&visible);
    }
    conflict.conflicts.retain(|f| !f.later_events.is_empty());
    if conflict.conflicts.is_empty() {
        return ApiError::new(
            StatusCode::CONFLICT,
            "undo_conflict",
            "later changes conflict with this undo".to_owned(),
        )
        .with_details(serde_json::json!({ "hint": "retry with force: true to override" }));
    }
    undo_error(&UndoError::Conflict(conflict), seqs)
}
