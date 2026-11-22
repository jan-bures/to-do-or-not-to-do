//! Attachment upload and download (REST only; MCP has no attachment tools, see below).
//!
//! - `POST /api/v1/tasks/{id}/attachments?file_name=...[&attachment_id=...]`: the request body is the raw
//!   file (no multipart); `Content-Type` is its media type (default `application/octet-stream`). At most
//!   `attachments.max_upload_bytes` (25 MiB): a larger `Content-Length` is refused before anything is
//!   read, and a body without one is counted while it streams and cut off at the cap. Only this route
//!   has the larger body limit and its own, longer deadline (`attachments.upload_timeout_ms`).
//! - `GET /api/v1/attachments/{id}`: the content, decrypted while it streams, as a download. `HEAD` gives
//!   the same headers without fetching anything (nothing is charged): a cheap "is the content stored?".
//! - `PUT /api/v1/attachments/{id}/content?size_bytes=...&sha256=...`: **owner only.** Fills in the content
//!   of an imported attachment, whose reference (storage key `import/sha256/<hex>`) already exists
//!   without a stored object. Same body cap, deadline, sealing and budget as an upload. The size must be
//!   the reference's and the hash the one in its key; the body is hashed while it is sealed and must
//!   match both, or nothing is charged or stored (422 `content_mismatch`). The object is recorded for
//!   the existing reference, so no command is written. The same content again is 200
//!   `idempotent-replayed: true` without reading the body; other content for an attachment that has
//!   some is 409 `content_exists`. A replay is decided from the declared `size_bytes` and `sha256`
//!   alone (stored content was verified against them when it was stored); the body of a replay is never
//!   read, so a caller must check its own bytes against the hash it sends. Agents, share tokens and
//!   anonymous callers get exactly what a missing route gives (404 `no such endpoint`, or 401 / the
//!   global 413), before any parameter or announced size is looked at.
//!
//! **Visibility under the writer.** A HEAD or GET decides visibility on a snapshot, looks the object up
//! in the registry (which can block), and then, for agents, confirms the decision inside the writer, so
//! access revoked during the lookup gives the 404 of a missing attachment (the approach of
//! [`super::hidden`]). The fill route is owner-only and reads no visibility, so it has nothing to
//! confirm; an attachment deleted while its content is stored is collected like any unreferenced
//! object.
//!
//! **Timing.** A HEAD (or GET) for an attachment the caller cannot see and one that does not exist give
//! the same 404, but the hidden one also runs the visibility check, so it may answer measurably later.
//! That difference is accepted, as for writes (see [`super::hidden`]).
//!
//! **Access.** Uploading is `add_attachment`: the owner, or an agent with capture access on the task's
//! list, exactly as `todo-core` decides for the command; it counts as one write against an agent's quota.
//! Upload accepts bearer authentication or the owner's web session with its CSRF token and matching
//! `Origin`. Filling imported content still requires an owner bearer token.
//! Downloading needs read access on the task's list; a task or attachment the caller cannot see, a
//! deleted one, and a missing one all give the same 404. Share pages never show attachments.
//!
//! **Retries.** Pass your own `attachment_id` (a UUID) to make an upload retryable: once it has been
//! stored, the same `attachment_id` returns the stored attachment (`idempotent-replayed: true`) without
//! reading the body or touching storage again. While an upload with that ID is still in flight, a
//! concurrent retry gets 409 `upload_in_progress` (with `Retry-After`) without its body being read.
//!
//! **IDs.** A taken `attachment_id` is 409 `already_exists` for the owner and the generic
//! `id_unavailable` for agents, the same whoever holds the ID (see [`super::hidden`]).
//!
//! What remains: IDs are chosen by clients, so an agent can still confirm that an ID *it already knows*
//! is taken (it gets `id_unavailable`, or `command_id_reused` for the upload's derived command ID,
//! instead of success). It learns nothing else: not the task, the list, or whether the attachment is
//! live. That is acceptable only because IDs cannot be guessed:
//!
//! - an upload without `attachment_id` gets a UUIDv4 from the server: 122 random bits;
//! - clients built on `todo-client` make UUIDv7s: a 48-bit millisecond timestamp and 74 bits from the
//!   OS random number generator, so knowing roughly when it was made still leaves 2^74 candidates;
//! - imported attachments are UUIDv7-shaped with the original creation time and 74 bits of a SHA-256
//!   over a fixed namespace and the Microsoft To Do attachment ID, which never leaves the import bundle
//!   (the server stores no source IDs);
//! - the one server-made ID that used to be computable, the monthly budget warning task, is now
//!   derived with the master key (see [`crate::blobs`]).
//!
//! An agent that picks a predictable ID for its own upload only makes its own attachment probeable.
//!
//! **Costs.** Every upload and download is an R2 operation counted in the monthly budget (see
//! [`crate::blobs`]); downloads are also rate-limited per token, and both have concurrency caps.
//!
//! **MCP.** Not exposed: the MCP endpoint keeps the global 1 MiB body limit, which would cap base64
//! content well below a useful size, and a second upload path would double the surface to secure.

use super::commands::REPLAYED_HEADER;
use super::error::{ApiError, ErrorBody};
use super::{ApiPath, ApiQuery, RequestCancel, Work, blocking, hidden, visibility};
use crate::app::AppState;
use crate::auth::Authenticated;
use crate::blobs::registry::{BlobRow, Refusal};
use crate::blobs::{
    AttachmentError, Attachments, InFlight, UploadSlot, seconds_to_next_month, store,
    upload_command_id,
};
use crate::engine::CommandResult;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{post, put};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use todo_core::access::{authorize, entity_target};
use todo_core::command::Command;
use todo_core::ids::{AttachmentId, EntityId, TaskId};
use todo_core::model::AttachmentRef;
use utoipa::{IntoParams, ToSchema};

/// The policy of every download: nothing in it may run, load or be framed, even if a browser ignored
/// `Content-Disposition`.
pub const DOWNLOAD_CSP: &str = "default-src 'none'; sandbox; frame-ancestors 'none'";
/// Longest accepted file name, in bytes.
pub const MAX_FILE_NAME: usize = 255;
/// Longest accepted media type, in bytes.
pub const MAX_MEDIA_TYPE: usize = 255;
const FALLBACK_MEDIA_TYPE: &str = "application/octet-stream";

/// Upload parameters.
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct UploadParams {
    /// The file name, without directories (at most 255 bytes of UTF-8).
    pub file_name: String,
    /// Your ID for the new attachment, to make retries safe. Default: a random one.
    #[serde(default)]
    #[param(value_type = Option<String>, format = Uuid)]
    pub attachment_id: Option<AttachmentId>,
}

/// The response to an upload.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UploadResponse {
    /// The new attachment.
    pub attachment: AttachmentRef,
    /// The stored (or replayed) `add_attachment` result.
    pub result: CommandResult,
    /// The log epoch of the result's sequence numbers.
    pub epoch: uuid::Uuid,
}

/// Maps attachment failures to responses. Budget refusals also file the month's warning task.
fn attachment_error(att: &Attachments, e: AttachmentError) -> ApiError {
    match e {
        AttachmentError::Refused(r) => {
            att.warn_refused(&r);
            refusal_error(&r, att)
        }
        AttachmentError::Store(store::StoreError::NotFound { .. }) => content_unavailable(),
        AttachmentError::Engine(e) => e.into(),
        AttachmentError::Store(e) => {
            tracing::error!(error = %e, "attachment store failed");
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "storage_unavailable",
                "attachment storage is unavailable; retry later",
            )
            .with_retry_after(30)
        }
        AttachmentError::Crypto(e) => {
            tracing::error!(error = %e, "attachment content failed to decrypt");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "content_corrupt",
                "the stored content failed its integrity check",
            )
        }
        e => {
            tracing::error!(error = %e, "attachment failure");
            ApiError::internal()
        }
    }
}

fn refusal_error(r: &Refusal, att: &Attachments) -> ApiError {
    match r {
        Refusal::Storage { used, wanted, cap } => ApiError::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "storage_budget_exhausted",
            "attachment storage is full; delete attachments to free space",
        )
        .with_details(json!({ "used_bytes": used, "needed_bytes": wanted, "cap_bytes": cap })),
        Refusal::Operations {
            class,
            month,
            used,
            cap,
        } => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "r2_budget_exhausted",
            "this month's attachment storage operations are used up",
        )
        .with_details(
            json!({ "class": format!("{class:?}"), "month": month, "used": used, "cap": cap }),
        )
        .with_retry_after(seconds_to_next_month(att.now())),
    }
}

fn content_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "content_unavailable",
        "this attachment has no stored content",
    )
}

fn too_large(max: u64) -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        format!("attachments are limited to {max} bytes"),
    )
    .with_details(json!({ "max_bytes": max }))
}

fn invalid(field: &'static str, reason: &str) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid",
        format!("{field}: {reason}"),
    )
    .with_details(json!({ "field": field, "reason": reason }))
}

fn is_tchar(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// A normalized media type (`type/subtype` lowercased, parameters kept), or `None` if `raw` is not one.
#[must_use]
pub fn media_type(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > MAX_MEDIA_TYPE || !raw.is_ascii() {
        return None;
    }
    let mut parts = raw.split(';');
    let essence = parts.next()?.trim();
    let (ty, sub) = essence.split_once('/')?;
    if ty.is_empty() || sub.is_empty() || !ty.chars().all(is_tchar) || !sub.chars().all(is_tchar) {
        return None;
    }
    let mut out = format!("{}/{}", ty.to_ascii_lowercase(), sub.to_ascii_lowercase());
    for p in parts {
        let (k, v) = p.trim().split_once('=')?;
        let v = v.trim();
        let quoted = v.len() >= 2
            && v.starts_with('"')
            && v.ends_with('"')
            && v[1..v.len() - 1]
                .chars()
                .all(|c| c != '"' && c != '\\' && (c == '\t' || !c.is_ascii_control()));
        if k.is_empty()
            || !k.chars().all(is_tchar)
            || !(quoted || (!v.is_empty() && v.chars().all(is_tchar)))
        {
            return None;
        }
        out.push_str("; ");
        out.push_str(&k.to_ascii_lowercase());
        out.push('=');
        out.push_str(v);
    }
    Some(out)
}

/// Checks a file name: non-empty, at most [`MAX_FILE_NAME`] bytes, no directories, no control
/// characters.
pub fn file_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() || name.len() > MAX_FILE_NAME {
        return Err(invalid("file_name", "empty or longer than 255 bytes"));
    }
    if name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
        || name == "."
        || name == ".."
    {
        return Err(invalid(
            "file_name",
            "must be a plain file name without directories or control characters",
        ));
    }
    Ok(name.to_owned())
}

/// `Content-Disposition: attachment` with an ASCII fallback name and the exact UTF-8 name as an RFC 8187
/// `filename*`, so no file name can break out of the header or change its meaning.
#[must_use]
pub fn content_disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c == ' ' || (c.is_ascii_graphic() && !matches!(c, '"' | '\\' | '%' | ';' | ',')) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let fallback = if fallback.trim_matches(['_', ' ', '.']).is_empty() {
        "download".to_owned()
    } else {
        fallback
    };
    let mut encoded = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(char::from(b));
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

/// The upload routes, mounted outside the global body limit and request deadline: the handler enforces
/// `max_upload_bytes` itself while streaming, and the deadline is `upload_timeout_ms`.
pub fn upload_router(state: AppState) -> Router<AppState> {
    let timeout = state.attachments.config().upload_timeout();
    let max = state.attachments.config().max_upload_bytes;
    let upload = Router::new()
        .route("/api/v1/tasks/{id}/attachments", post(upload))
        // Route layers only: a `layer` would also wrap this router's fallback, which becomes the app's.
        .route_layer(from_fn_with_state(
            state.clone(),
            crate::auth::require_auth_or_session,
        ))
        .route_layer(from_fn(super::tag_route))
        .route_layer(from_fn_with_state(timeout, crate::app::deadline))
        // Like the global limit, an announced oversized body is refused before authentication.
        .route_layer(from_fn_with_state(max, refuse_announced_oversize));
    let fill = Router::new()
        .route("/api/v1/attachments/{id}/content", put(fill_content))
        .route_layer(from_fn_with_state(state.clone(), crate::auth::require_auth))
        .route_layer(from_fn(super::tag_route))
        .route_layer(from_fn_with_state(timeout, crate::app::deadline))
        // Owner only: the larger cap applies once the caller is known to be the owner.
        .route_layer(from_fn_with_state(state, owner_sized_body));
    upload.merge(fill)
}

fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// The body limit of the owner-only fill route. A body announced over the global limit gets exactly
/// what a route that does not exist gives (the global 413) unless the caller is the owner, who gets the
/// upload cap instead. So an agent, a share token or an anonymous caller learns nothing from the size
/// it announces. Checking the token here counts a failure against the caller's address like any bearer
/// check, so this is no cheaper a token oracle than any other route.
async fn owner_sized_body(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(declared) = declared_length(req.headers()) else {
        return next.run(req).await;
    };
    let global = u64::try_from(state.limits.max_body_bytes).unwrap_or(u64::MAX);
    if declared <= global {
        return next.run(req).await;
    }
    let svc = &state.authsvc;
    let ip = svc.client_ip(req.headers(), crate::auth::peer_ip(req.extensions()));
    if let Err(wait) = svc.bearer_admission(ip) {
        return ApiError::too_many_requests(wait).into_response();
    }
    match state.auth.authenticate(req.headers()) {
        Ok(a) if !a.is_agent() => {
            let max = state.attachments.config().max_upload_bytes;
            if declared > max {
                return too_large(max).into_response();
            }
            next.run(req).await
        }
        Ok(_) => crate::app::body_too_large().into_response(),
        Err(_) => {
            svc.bearer_failed(ip);
            crate::app::body_too_large().into_response()
        }
    }
}

/// 413 for a `Content-Length` over the upload cap, before anything else runs.
async fn refuse_announced_oversize(
    State(max): State<u64>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if declared_length(req.headers()).is_some_and(|n| n > max) {
        return too_large(max).into_response();
    }
    next.run(req).await
}

fn attachments(app: &AppState) -> Arc<Attachments> {
    app.attachments.clone()
}

/// What the dry run found.
enum Plan {
    /// The attachment is already stored (a retry): return it.
    Replay(Box<AttachmentRef>, CommandResult),
    /// Go ahead, holding the reservation of the ID.
    Upload(Option<UploadSlot>),
}

/// Uploads an attachment to a task.
///
/// Send a bearer token or, for the owner’s web session, its cookie and `X-CSRF-Token` with an
/// `Origin` that matches the server's public origin. An `Authorization` header takes precedence.
#[utoipa::path(
    post,
    path = "/api/v1/tasks/{id}/attachments",
    tag = "attachments",
    params(("id" = String, Path, format = Uuid, description = "Task ID"), UploadParams),
    request_body(
        content = Vec<u8>,
        content_type = "application/octet-stream",
        description = "The raw file. `Content-Type` is stored as its media type (any type; default `application/octet-stream`). At most 25 MiB."
    ),
    responses(
        (status = 201, description = "Stored", body = UploadResponse),
        (status = 200, description = "Already stored under this `attachment_id` (header `idempotent-replayed: true`); the body was not read", body = UploadResponse),
        (status = 401, description = "Missing or invalid token or session", body = ErrorBody),
        (status = 403, description = "No capture access on the task's list, or the session's CSRF or Origin check failed", body = ErrorBody),
        (status = 404, description = "No such task (or one you cannot see)", body = ErrorBody),
        (status = 408, description = "The deadline passed before the attachment was recorded", body = ErrorBody),
        (status = 409, description = "The task is deleted; `attachment_id` is taken (`already_exists`, or `id_unavailable` for agents, whoever holds it); or an upload with this `attachment_id` is still in progress (`upload_in_progress`, see `Retry-After`)", body = ErrorBody),
        (status = 413, description = "Larger than the limit (`payload_too_large`)", body = ErrorBody),
        (status = 422, description = "Bad file name or media type", body = ErrorBody),
        (status = 429, description = "Rate limit, write quota (`quota_exceeded`) or this month's storage operations (`r2_budget_exhausted`); see `Retry-After`", body = ErrorBody),
        (status = 502, description = "Attachment storage failed (`storage_unavailable`)", body = ErrorBody),
        (status = 503, description = "Too many uploads at once (`overloaded`)", body = ErrorBody),
        (status = 507, description = "Attachment storage is full (`storage_budget_exhausted`)", body = ErrorBody),
    ),
    security(("bearer" = []), ("web_session" = [], "csrf" = []))
)]
#[allow(clippy::too_many_lines)]
pub async fn upload(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    ApiPath(task): ApiPath<TaskId>,
    ApiQuery(q): ApiQuery<UploadParams>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let att = attachments(&app);
    let max = att.config().max_upload_bytes;
    let name = file_name(&q.file_name)?;
    let media = match headers.get(CONTENT_TYPE) {
        None => FALLBACK_MEDIA_TYPE.to_owned(),
        Some(v) => v
            .to_str()
            .ok()
            .and_then(media_type)
            .ok_or_else(|| invalid("media_type", "Content-Type is not a valid media type"))?,
    };
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > max) {
        return Err(too_large(max));
    }
    let id = q
        .attachment_id
        .unwrap_or_else(|| AttachmentId::from_uuid(uuid::Uuid::new_v4()));
    let object_key = store::new_object_key().map_err(|_| ApiError::internal())?;
    let command = |size_bytes: u64| Command::AddAttachment {
        id,
        task,
        file_name: name.clone(),
        media_type: media.clone(),
        size_bytes,
        storage_key: object_key.clone(),
    };

    // Dry run: refuse what the command would refuse before reading a byte or touching storage.
    let (engine, actor, probe, a) = (
        app.engine.clone(),
        auth.actor.clone(),
        command(0),
        att.clone(),
    );
    let plan = blocking(&app, Work::Read, move || -> Result<Plan, ApiError> {
        let dry_run = |state: &todo_core::State| -> Result<Plan, ApiError> {
            // On the same snapshot as the decision below. Only a pre-check: the write re-checks
            // inside the writer.
            hidden::check_command(state, &actor, &probe)?;
            // A retry of an upload this caller completed: same attachment, same task, stored content.
            if let Some(existing) = state.attachments.get(&id)
                && existing.task == task
                && let Some((stored, _)) = engine.stored_command(&actor, upload_command_id(id))?
                && a.find(id, &existing.storage_key)
                    .map_err(|e| attachment_error(&a, e))?
                    .is_some()
                && replay_still_allowed(&engine, &actor, &probe, existing)?
            {
                return Ok(Plan::Replay(Box::new(existing.clone()), stored));
            }
            authorize(state, &actor, &probe)
                .map_err(|d| super::error::domain_error(&todo_core::DomainError::Denied(d)))?;
            todo_core::decide(state, &actor, &probe, engine.now())
                .map_err(|e| hidden::domain_write_error(&actor, &e))?;
            Ok(Plan::Upload(None))
        };
        let first = dry_run(&engine.state())?;
        if let Plan::Replay(..) = first {
            return Ok(first);
        }
        // One upload per ID at a time, so concurrent retries cannot each store (and pay for) a copy.
        // Taken after the access checks, so it answers nothing an agent could not learn anyway.
        let slot = a.reserve_id(id, &actor).map_err(|busy| match busy {
            InFlight::SameCaller => ApiError::new(
                StatusCode::CONFLICT,
                "upload_in_progress",
                "an upload with this attachment_id is in progress; retry when it has finished",
            )
            .with_retry_after(5),
            InFlight::OtherCaller if visibility::sees_all(&actor) => {
                super::error::domain_error(&todo_core::DomainError::AlreadyExists(id.into()))
            }
            InFlight::OtherCaller => hidden::id_unavailable(id.into()),
        })?;
        // Again, holding the reservation: an upload that finished meanwhile is now a replay (or taken).
        match dry_run(&engine.state())? {
            Plan::Upload(_) => Ok(Plan::Upload(Some(slot))),
            replay => Ok(replay),
        }
    })
    .await??;
    let slot = match plan {
        Plan::Upload(slot) => slot,
        Plan::Replay(attachment, result) => {
            return Ok(replayed(attachment, result, app.engine.epoch()));
        }
    };

    auth.take_write(&app)?;
    let permit = att
        .uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::overloaded())?;
    let a = att.clone();
    let declared_size = declared.unwrap_or(0);
    tokio::task::spawn_blocking(move || a.precheck_upload(declared_size))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|e| attachment_error(&att, e))?;

    // Seal while reading: the body is counted and encrypted chunk by chunk into the spool file, never
    // held in memory.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(8);
    let (a, key) = (att.clone(), object_key.clone());
    let sealer = tokio::task::spawn_blocking(move || {
        let mut spool = a.spool(&key, id)?;
        while let Some(chunk) = rx.blocking_recv() {
            spool.push(&chunk)?;
        }
        spool.finish()
    });
    let mut stream = body.into_data_stream();
    let mut total: u64 = 0;
    let mut failed = None;
    while let Some(frame) = stream.next().await {
        let Ok(bytes) = frame else {
            failed = Some(ApiError::new(
                StatusCode::BAD_REQUEST,
                "body_read_failed",
                "the upload was interrupted",
            ));
            break;
        };
        total = total.saturating_add(bytes.len() as u64);
        if total > max {
            failed = Some(too_large(max));
            break;
        }
        if tx.send(bytes).await.is_err() {
            break;
        }
    }
    drop(tx);
    if let Some(e) = failed {
        // The sealer ends with the sender; dropping its result removes the spool file before we answer.
        drop(sealer.await);
        return Err(e);
    }
    let sealed = sealer
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|e| attachment_error(&att, e))?;

    let (a, key, claim) = (att.clone(), object_key.clone(), cancel.clone());
    // The reservation moves with the work: if the request is abandoned (its deadline), the blocking
    // tasks still finish, and the ID stays reserved until they have.
    //
    // Charging and the PUT start this request's commit: it is claimed first, so an upload past its
    // deadline (already answered 408) charges and sends nothing, and one that claimed in time is
    // awaited. The command below then commits under the same claim.
    let stored = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        a.before_store_commit();
        if !claim.begin_commit() {
            return Err(ApiError::timeout());
        }
        a.store(&key, id, &key, &sealed)
            .map(|_| (sealed.plain, slot))
            .map_err(|e| attachment_error(&a, e))
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let (stored, slot) = stored?;

    let (engine, actor, cmd) = (app.engine.clone(), auth.actor.clone(), command(stored));
    let (outcome, attachment) = blocking(&app, Work::Write, move || {
        // Held until the reference is recorded (or refused).
        let _slot = slot;
        hidden::check_command(&engine.state(), &actor, &cmd)?;
        let o = engine
            .execute_checked(
                &actor,
                upload_command_id(id),
                &cmd,
                &cancel,
                hidden::command_check(&actor, &cmd),
            )
            .map_err(|e| hidden::write_error(&actor, e))?;
        let a = engine.state().attachments.get(&id).cloned();
        Ok::<_, ApiError>((o, a))
    })
    .await??;
    let attachment = attachment.ok_or_else(ApiError::internal)?;
    let mut res = (
        StatusCode::CREATED,
        axum::Json(UploadResponse {
            attachment,
            result: outcome.result,
            epoch: app.engine.epoch(),
        }),
    )
        .into_response();
    if outcome.replayed {
        res.headers_mut()
            .insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
    }
    Ok(res)
}

/// Whether a replay found on an earlier snapshot may still be returned: decided inside the writer, so
/// on the state no grant change or write can move until it returns. The lookups before it (the stored
/// result, the blob) can block, and access may have been revoked meanwhile; a task that became hidden
/// then gets `not_found`, as if it were missing (see [`hidden`]). The blob I/O stays outside the
/// writer. Owners see everything, so they skip it.
fn replay_still_allowed(
    engine: &crate::engine::Engine,
    actor: &todo_core::Actor,
    probe: &Command,
    existing: &AttachmentRef,
) -> Result<bool, ApiError> {
    if visibility::sees_all(actor) {
        return Ok(true);
    }
    engine.with_writer(|now| -> Result<bool, ApiError> {
        hidden::check_command(now, actor, probe)?;
        Ok(now
            .attachments
            .get(&existing.id)
            .is_some_and(|a| a.task == existing.task && a.storage_key == existing.storage_key))
    })?
}

fn replayed(attachment: Box<AttachmentRef>, result: CommandResult, epoch: uuid::Uuid) -> Response {
    let mut res = Json200(UploadResponse {
        attachment: *attachment,
        result,
        epoch,
    })
    .into_response();
    res.headers_mut()
        .insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
    res
}

struct Json200<T>(T);

impl<T: Serialize> IntoResponse for Json200<T> {
    fn into_response(self) -> Response {
        (StatusCode::OK, axum::Json(self.0)).into_response()
    }
}

/// The storage key prefix of imported references: `import/sha256/<hex SHA-256 of the content>`.
pub const IMPORT_KEY_PREFIX: &str = "import/sha256/";

/// Parameters of [`fill_content`].
#[derive(Debug, Clone, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct ContentParams {
    /// The content's size in bytes. It must equal the attachment's `size_bytes`.
    pub size_bytes: u64,
    /// The content's SHA-256, as 64 hex digits. It must be the one in the attachment's storage key.
    pub sha256: String,
}

/// The response to [`fill_content`].
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ContentResponse {
    /// The attachment, unchanged.
    pub attachment: AttachmentRef,
    /// The stored content's size.
    pub size_bytes: u64,
    /// The stored content's SHA-256 (hex).
    pub sha256: String,
}

/// What the checks of [`fill_content`] found.
enum Fill {
    /// The same content is stored already.
    Replay(Box<AttachmentRef>),
    /// Go ahead.
    Store(Box<AttachmentRef>),
}

fn mismatch(reason: &str, details: serde_json::Value) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "content_mismatch",
        format!("the content does not match: {reason}"),
    )
    .with_details(details)
}

/// Decides whether the content of `id` may be filled in with `size` bytes hashing to `sha`.
fn plan_fill(
    state: &todo_core::State,
    att: &Attachments,
    id: AttachmentId,
    size: u64,
    sha: &str,
) -> Result<Fill, ApiError> {
    let meta = state
        .attachments
        .get(&id)
        .filter(|_| state.is_live(EntityId::Attachment(id)))
        .ok_or_else(|| ApiError::not_found(format!("attachment {id} not found")))?;
    let stored = att
        .find(id, &meta.storage_key)
        .map_err(|e| attachment_error(att, e))?
        .is_some();
    let referenced = meta.storage_key.strip_prefix(IMPORT_KEY_PREFIX);
    if referenced == Some(sha) {
        if size != meta.size_bytes {
            return Err(mismatch(
                "size_bytes is not the attachment's size",
                json!({ "expected_bytes": meta.size_bytes, "size_bytes": size }),
            ));
        }
        return Ok(if stored {
            Fill::Replay(Box::new(meta.clone()))
        } else {
            Fill::Store(Box::new(meta.clone()))
        });
    }
    if stored {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "content_exists",
            "this attachment already has other content",
        ));
    }
    if referenced.is_some() {
        return Err(mismatch(
            "sha256 is not the one the attachment was imported with",
            json!({ "field": "sha256" }),
        ));
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "not_imported",
        "only imported attachments (storage key import/sha256/...) can have their content filled in",
    ))
}

/// Fills in the content of an imported attachment.
#[utoipa::path(
    put,
    path = "/api/v1/attachments/{id}/content",
    tag = "attachments",
    params(("id" = String, Path, format = Uuid, description = "Attachment ID"), ContentParams),
    request_body(
        content = Vec<u8>,
        content_type = "application/octet-stream",
        description = "The raw content: exactly `size_bytes` bytes hashing to `sha256`. At most 25 MiB."
    ),
    responses(
        (status = 201, description = "Verified and stored", body = ContentResponse),
        (status = 200, description = "This content is stored already (header `idempotent-replayed: true`). Decided from `size_bytes` and `sha256` alone: the body was not read, so check your own bytes against the hash you send.", body = ContentResponse),
        (status = 401, description = "Missing or invalid token", body = ErrorBody),
        (status = 404, description = "No such attachment (or a deleted one). Agents get the answer of a missing route: owner only.", body = ErrorBody),
        (status = 408, description = "The deadline passed before the content was stored", body = ErrorBody),
        (status = 409, description = "The attachment already has other content (`content_exists`), was not imported (`not_imported`), or a fill of it is in progress (`upload_in_progress`, see `Retry-After`)", body = ErrorBody),
        (status = 413, description = "Larger than the limit (`payload_too_large`)", body = ErrorBody),
        (status = 422, description = "Bad parameters (`invalid`), or the size or hash does not match the attachment or the body (`content_mismatch`); nothing is stored", body = ErrorBody),
        (status = 429, description = "Rate limit, or this month's storage operations (`r2_budget_exhausted`); see `Retry-After`", body = ErrorBody),
        (status = 502, description = "Attachment storage failed (`storage_unavailable`)", body = ErrorBody),
        (status = 503, description = "Too many uploads at once (`overloaded`)", body = ErrorBody),
        (status = 507, description = "Attachment storage is full (`storage_budget_exhausted`)", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
#[allow(clippy::too_many_lines)]
pub async fn fill_content(
    auth: Authenticated,
    State(app): State<AppState>,
    RequestCancel(cancel): RequestCancel,
    path: Result<ApiPath<AttachmentId>, ApiError>,
    query: Result<ApiQuery<ContentParams>, ApiError>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    // Owner only. Before anything else (even parameter errors), agents get what a missing route gives.
    if auth.is_agent() {
        return Err(super::no_such_endpoint());
    }
    let ApiPath(id) = path?;
    let ApiQuery(q) = query?;
    let att = attachments(&app);
    let max = att.config().max_upload_bytes;
    let sha = q.sha256.trim().to_ascii_lowercase();
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("sha256", "must be 64 hex digits"));
    }
    let size = q.size_bytes;
    if size > max {
        return Err(too_large(max));
    }
    let declared = declared_length(&headers);
    if declared.is_some_and(|n| n > max) {
        return Err(too_large(max));
    }
    if let Some(n) = declared.filter(|n| *n != size) {
        return Err(mismatch(
            "Content-Length is not size_bytes",
            json!({ "expected_bytes": size, "content_length": n }),
        ));
    }

    let (engine, actor, a, s) = (
        app.engine.clone(),
        auth.actor.clone(),
        att.clone(),
        sha.clone(),
    );
    let plan = blocking(&app, Work::Read, move || -> Result<_, ApiError> {
        let first = plan_fill(&engine.state(), &a, id, size, &s)?;
        if let Fill::Replay(_) = first {
            return Ok((first, None));
        }
        // One fill (or upload) per ID at a time, so concurrent retries cannot each store a copy.
        let slot = a.reserve_id(id, &actor).map_err(|_| {
            ApiError::new(
                StatusCode::CONFLICT,
                "upload_in_progress",
                "content for this attachment is being stored; retry when it has finished",
            )
            .with_retry_after(5)
        })?;
        // Again, holding the reservation: a fill that finished meanwhile is now a replay.
        match plan_fill(&engine.state(), &a, id, size, &s)? {
            Fill::Store(m) => Ok((Fill::Store(m), Some(slot))),
            replay => Ok((replay, None)),
        }
    })
    .await??;
    let (meta, slot) = match plan {
        (Fill::Replay(meta), _) => {
            let mut res = Json200(ContentResponse {
                attachment: *meta,
                size_bytes: size,
                sha256: sha,
            })
            .into_response();
            res.headers_mut()
                .insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
            return Ok(res);
        }
        (Fill::Store(meta), slot) => (meta, slot),
    };

    auth.take_write(&app)?;
    let permit = att
        .uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::overloaded())?;
    let a = att.clone();
    tokio::task::spawn_blocking(move || a.precheck_upload(size))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|e| attachment_error(&att, e))?;

    // Seal and hash while reading, chunk by chunk, as an upload does; verified before anything is
    // charged or stored.
    let object_key = store::new_object_key().map_err(|_| ApiError::internal())?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(8);
    let (a, key) = (att.clone(), object_key.clone());
    let sealer = tokio::task::spawn_blocking(move || {
        use sha2::Digest;
        let mut hash = sha2::Sha256::new();
        let mut spool = a.spool(&key, id)?;
        while let Some(chunk) = rx.blocking_recv() {
            hash.update(&chunk);
            spool.push(&chunk)?;
        }
        Ok::<_, AttachmentError>((spool.finish()?, hex::encode(hash.finalize())))
    });
    let mut stream = body.into_data_stream();
    let mut total: u64 = 0;
    let mut failed = None;
    while let Some(frame) = stream.next().await {
        let Ok(bytes) = frame else {
            failed = Some(ApiError::new(
                StatusCode::BAD_REQUEST,
                "body_read_failed",
                "the upload was interrupted",
            ));
            break;
        };
        total = total.saturating_add(bytes.len() as u64);
        if total > max {
            failed = Some(too_large(max));
            break;
        }
        if total > size {
            failed = Some(mismatch(
                "the body is longer than size_bytes",
                json!({ "expected_bytes": size }),
            ));
            break;
        }
        if tx.send(bytes).await.is_err() {
            break;
        }
    }
    drop(tx);
    if failed.is_none() && total != size {
        failed = Some(mismatch(
            "the body is shorter than size_bytes",
            json!({ "expected_bytes": size, "received_bytes": total }),
        ));
    }
    if let Some(e) = failed {
        // The sealer ends with the sender; dropping its result removes the spool file before we answer.
        drop(sealer.await);
        return Err(e);
    }
    let (sealed, digest) = sealer
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|e| attachment_error(&att, e))?;
    if digest != sha || sealed.plain != size {
        // Dropping `sealed` removes the spool file: nothing was charged or stored.
        drop(sealed);
        return Err(mismatch(
            "the body's SHA-256 is not sha256",
            json!({ "field": "sha256" }),
        ));
    }

    let (a, ref_key) = (att.clone(), meta.storage_key.clone());
    // Recorded as the object of the existing reference (its `storage_key`), so downloads and the
    // collector find it like any upload. No command: the attachment itself does not change.
    // Charging and the PUT are this request's commit: claimed first, so a request past its deadline
    // (already answered 408) charges and sends nothing, and one that claimed in time is awaited.
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _slot = slot;
        a.before_store_commit();
        if !cancel.begin_commit() {
            return Err(ApiError::timeout());
        }
        a.store(&object_key, id, &ref_key, &sealed)
            .map_err(|e| attachment_error(&a, e))
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok((
        StatusCode::CREATED,
        axum::Json(ContentResponse {
            attachment: *meta,
            size_bytes: size,
            sha256: sha,
        }),
    )
        .into_response())
}

/// Downloads an attachment's content.
#[utoipa::path(
    get,
    path = "/api/v1/attachments/{id}",
    tag = "attachments",
    params(("id" = String, Path, format = Uuid, description = "Attachment ID")),
    responses(
        (status = 200, description = "The content, always as a download (`Content-Disposition: attachment`, `X-Content-Type-Options: nosniff`, a sandboxing CSP). `Content-Type` is the stored media type.", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 401, description = "Missing or invalid token", body = ErrorBody),
        (status = 404, description = "No such attachment, one you cannot see, a deleted one (`not_found`), or one without stored content (`content_unavailable`)", body = ErrorBody),
        (status = 429, description = "Download rate limit, or this month's storage operations (`r2_budget_exhausted`); see `Retry-After`", body = ErrorBody),
        (status = 500, description = "The stored content failed its integrity check (`content_corrupt`)", body = ErrorBody),
        (status = 502, description = "Attachment storage failed (`storage_unavailable`)", body = ErrorBody),
        (status = 503, description = "Too many downloads at once (`overloaded`)", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn download(
    auth: Authenticated,
    State(app): State<AppState>,
    ApiPath(id): ApiPath<AttachmentId>,
) -> Result<Response, ApiError> {
    let att = attachments(&app);
    att.download_limiter
        .check(&auth.principal)
        .map_err(ApiError::too_many_requests)?;
    let (meta, row) = stored_content(&app, &auth, id).await?;
    let plain_size = row.plain_size;
    let permit = att
        .downloads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::overloaded())?;

    // The first chunk is opened and checked before the response head is sent, so a refused, missing or
    // corrupt object still gets a proper error. A later failure aborts the body.
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    let a = att.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut opener = match a.open_object(&row) {
            Ok(o) => o,
            Err(e) => {
                let _ = first_tx.send(Err(attachment_error(&a, e)));
                return;
            }
        };
        match opener.next_chunk() {
            Ok(chunk) => {
                if first_tx
                    .send(Ok(Bytes::from(chunk.unwrap_or_default())))
                    .is_err()
                {
                    return;
                }
            }
            Err(e) => {
                let _ = first_tx.send(Err(attachment_error(&a, e.into())));
                return;
            }
        }
        loop {
            let next = match opener.next_chunk() {
                Ok(Some(c)) => Ok(Bytes::from(c)),
                Ok(None) => return,
                Err(e) => {
                    tracing::error!(error = %e, "attachment content failed to decrypt mid-stream");
                    Err(std::io::Error::other(
                        "attachment content failed its integrity check",
                    ))
                }
            };
            let stop = next.is_err();
            if tx.blocking_send(next).is_err() || stop {
                return;
            }
        }
    });
    let first = first_rx.await.map_err(|_| ApiError::internal())??;
    let rest = tokio_stream::wrappers::ReceiverStream::new(rx);
    let body = Body::from_stream(
        futures_util::stream::once(async move { Ok::<_, std::io::Error>(first) }).chain(rest),
    );
    let mut res = Response::new(body);
    download_headers(&mut res, &meta, plain_size);
    Ok(res)
}

/// The visible, live attachment `id` and its stored object: 404 `not_found` for one the caller cannot
/// see (or a deleted or missing one), 404 `content_unavailable` without stored content.
async fn stored_content(
    app: &AppState,
    auth: &Authenticated,
    id: AttachmentId,
) -> Result<(AttachmentRef, BlobRow), ApiError> {
    let (engine, actor, a) = (
        app.engine.clone(),
        auth.actor.clone(),
        app.attachments.clone(),
    );
    blocking(app, Work::Read, move || {
        let state = engine.state();
        let missing = || ApiError::not_found(format!("attachment {id} not found"));
        let meta = state.attachments.get(&id).ok_or_else(missing)?;
        let target = entity_target(&state, EntityId::Attachment(id));
        if !visibility::can_read(&state, &actor, target) || !state.is_live(EntityId::Attachment(id))
        {
            return Err(missing());
        }
        let row = a
            .find(id, &meta.storage_key)
            .map_err(|e| attachment_error(&a, e))?
            .ok_or_else(content_unavailable)?;
        // The registry lookup can block, and access may have been revoked meanwhile: for agents the
        // decision is confirmed inside the writer, on a state no grant change can move (as for upload
        // replays, see [`hidden`]). Owners see everything, so they skip it.
        if !visibility::sees_all(&actor) {
            let still = engine.with_writer(|now| {
                visibility::can_read(now, &actor, entity_target(now, EntityId::Attachment(id)))
                    && now.is_live(EntityId::Attachment(id))
                    && now
                        .attachments
                        .get(&id)
                        .is_some_and(|m| m.storage_key == meta.storage_key)
            })?;
            if !still {
                return Err(missing());
            }
        }
        Ok((meta.clone(), row))
    })
    .await?
}

/// Whether an attachment's content is stored, without fetching it.
#[utoipa::path(
    head,
    path = "/api/v1/attachments/{id}",
    tag = "attachments",
    params(("id" = String, Path, format = Uuid, description = "Attachment ID")),
    responses(
        (status = 200, description = "The content is stored. The headers are those of a download (`Content-Length` is the content's size), with no body. Nothing is fetched from storage or charged to the budget."),
        (status = 401, description = "Missing or invalid token"),
        (status = 404, description = "No such attachment, one you cannot see, a deleted one, or one without stored content"),
        (status = 429, description = "Rate limit (the download allowance, shared with `GET`); see `Retry-After`"),
    ),
    security(("bearer" = []))
)]
pub async fn head(
    auth: Authenticated,
    State(app): State<AppState>,
    ApiPath(id): ApiPath<AttachmentId>,
) -> Result<Response, ApiError> {
    // The same allowance as downloads, though nothing is fetched: a HEAD is not a free probe.
    app.attachments
        .download_limiter
        .check(&auth.principal)
        .map_err(ApiError::too_many_requests)?;
    let (meta, row) = stored_content(&app, &auth, id).await?;
    let mut res = Response::new(Body::empty());
    download_headers(&mut res, &meta, row.plain_size);
    Ok(res)
}

fn download_headers(res: &mut Response, meta: &AttachmentRef, plain_size: u64) {
    let media = media_type(&meta.media_type).unwrap_or_else(|| FALLBACK_MEDIA_TYPE.to_owned());
    let h = res.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&media)
            .unwrap_or_else(|_| HeaderValue::from_static(FALLBACK_MEDIA_TYPE)),
    );
    h.insert(CONTENT_LENGTH, HeaderValue::from(plain_size));
    h.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(&meta.file_name))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(DOWNLOAD_CSP),
    );
    // Private content: never stored, and never transformed (compressed) by a proxy on the way.
    h.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, no-transform"),
    );
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
}
