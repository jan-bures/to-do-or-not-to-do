//! Typed JSON errors: `{"error": {"code": "...", "message": "...", "details": {...}}}`.

use crate::db::StoreError;
use crate::engine::EngineError;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use todo_core::error::{DomainError, UndoError};
use todo_core::ids::EventId;
use todo_core::query::ParseError;

/// The error envelope.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    /// The error.
    pub error: ErrorDetail,
}

/// One error.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ErrorDetail {
    /// Stable machine-readable code, for example `invalid` or `undo_conflict`.
    pub code: String,
    /// Human-readable description.
    pub message: String,
    /// Structured details, depending on the code.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub details: Option<Value>,
}

/// An error response.
#[derive(Debug)]
pub struct ApiError {
    /// HTTP status.
    pub status: StatusCode,
    /// Error code.
    pub code: &'static str,
    /// Message.
    pub message: String,
    /// Details.
    pub details: Option<Value>,
    /// Seconds for a `Retry-After` header.
    pub retry_after: Option<u32>,
}

impl ApiError {
    /// Builds an error.
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
            retry_after: None,
        }
    }

    /// Adds a `Retry-After` header (seconds).
    #[must_use]
    pub fn with_retry_after(mut self, seconds: u32) -> Self {
        self.retry_after = Some(seconds.max(1));
        self
    }

    /// Adds details.
    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// 401.
    #[must_use]
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer token is required",
        )
    }

    /// 404 for a missing resource.
    pub fn not_found(what: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", what)
    }

    /// 500 without leaking internals.
    #[must_use]
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }

    /// 408 when a request took too long.
    #[must_use]
    pub fn timeout() -> Self {
        Self::new(
            StatusCode::REQUEST_TIMEOUT,
            "timeout",
            "the request took too long",
        )
    }

    /// 503 when the deadline passed while a write was committing and its outcome is not yet known. The
    /// write may or may not have been stored; retrying with the same `command_id` is safe and returns
    /// the stored result if there is one.
    #[must_use]
    pub fn outcome_unknown() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "outcome_unknown",
            "the write was committing when the deadline passed; retry with the same command_id",
        )
        .with_retry_after(1)
    }

    /// 400 when an undo request has no `command_id` (without one, a lost response could not be retried
    /// safely).
    #[must_use]
    pub fn command_id_required() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "command_id_required",
            "undo requests need a client-generated command_id so that they can be retried safely",
        )
    }

    /// 400 when an undo request has no `expected_event_id`.
    #[must_use]
    pub fn expected_event_id_required() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "expected_event_id_required",
            "undo requests need the expected_event_id of the event at seq",
        )
    }

    /// 409 when a request refers to another log epoch. `details.epoch` is the current one.
    #[must_use]
    pub fn epoch_mismatch(current: uuid::Uuid) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "epoch_mismatch",
            "the event log was replaced (restored); resync from seq 0",
        )
        .with_details(json!({ "epoch": current }))
    }

    /// 500 when a single item is larger than the response byte budget. `next` lets the client skip it
    /// (a `next_cursor` or `next_since`, depending on the route).
    #[must_use]
    pub fn item_too_large(field: &'static str, next: Value) -> Self {
        let mut details = serde_json::Map::new();
        details.insert(field.to_owned(), next);
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "item_too_large",
            "an item is larger than the response size limit",
        )
        .with_details(Value::Object(details))
    }

    /// 409 when an event-stream resume point is too far behind to replay, or the client just reconnected
    /// to the same point after a `catch_up`. The client must page `GET /events?since=` (JSON) up to
    /// `head_seq` and reconnect from where it got to.
    #[must_use]
    pub fn catch_up_required(since: u64, head_seq: u64, reason: &'static str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "catch_up_required",
            "too far behind to replay on the stream; page GET /events?since= up to head_seq, then reconnect",
        )
        .with_details(json!({ "since": since, "head_seq": head_seq, "reason": reason }))
    }

    /// 400 when a resume position is ahead of the log head.
    #[must_use]
    pub fn invalid_since(since: u64, head_seq: u64) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_since",
            "the resume position is ahead of the event log",
        )
        .with_details(json!({ "since": since, "head_seq": head_seq }))
    }

    /// 503 when the SSE connection limit is reached.
    #[must_use]
    pub fn too_many_streams() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "too_many_streams",
            "too many open event streams; retry later",
        )
    }

    /// 503 when too much blocking work is already admitted.
    #[must_use]
    pub fn overloaded() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "overloaded",
            "the server is busy; retry later",
        )
        .with_retry_after(1)
    }

    /// 503 when the engine is closed or poisoned and needs a restart.
    #[must_use]
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "the server cannot serve requests right now",
        )
        .with_retry_after(30)
    }

    /// 429 when a principal reconnects its event stream too often.
    #[must_use]
    pub fn rate_limited(retry_after: u32) -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many event-stream connects; retry later",
        )
        .with_retry_after(retry_after)
    }

    /// 400 for a malformed or unknown pagination cursor.
    #[must_use]
    pub fn invalid_cursor() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_cursor",
            "the cursor is malformed or belongs to a different query; restart from the first page",
        )
    }

    /// 422 for a smart-list query that does not parse.
    #[must_use]
    pub fn invalid_query(e: &ParseError) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_query",
            e.to_string(),
        )
        .with_details(json!({ "pos": e.pos, "len": e.len, "reason": e.kind.to_string() }))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let unauthorized = self.status == StatusCode::UNAUTHORIZED;
        let retry_after = self.retry_after;
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code.to_owned(),
                message: self.message,
                details: self.details,
            },
        };
        let mut res = (self.status, axum::Json(body)).into_response();
        if unauthorized {
            res.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        if let Some(secs) = retry_after {
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        res
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        tracing::error!(error = %e, "storage error");
        Self::internal()
    }
}

/// Maps a validation error to a 4xx response.
#[must_use]
pub fn domain_error(e: &DomainError) -> ApiError {
    use DomainError as D;
    let msg = e.to_string();
    let (status, code, details) = match e {
        D::Denied(a) => (
            StatusCode::FORBIDDEN,
            "forbidden",
            Some(json!({ "needed": a.needed, "have": a.have, "owner_only": a.owner_only })),
        ),
        D::NotFound(id) => (
            StatusCode::NOT_FOUND,
            "not_found",
            Some(json!({ "entity": id })),
        ),
        D::Deleted(id) => (
            StatusCode::CONFLICT,
            "deleted",
            Some(json!({ "entity": id })),
        ),
        D::NotDeleted(id) => (
            StatusCode::CONFLICT,
            "not_deleted",
            Some(json!({ "entity": id })),
        ),
        D::AlreadyDeleted(id) => (
            StatusCode::CONFLICT,
            "already_deleted",
            Some(json!({ "entity": id })),
        ),
        D::AlreadyExists(id) => (
            StatusCode::CONFLICT,
            "already_exists",
            Some(json!({ "entity": id })),
        ),
        D::DuplicateName(_) => (StatusCode::CONFLICT, "duplicate_name", None),
        D::Invalid { field, reason } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            Some(json!({ "field": field, "reason": reason })),
        ),
        D::BadPlacement(id) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "bad_placement",
            Some(json!({ "sibling": id })),
        ),
        D::SameEntity => (StatusCode::UNPROCESSABLE_ENTITY, "same_entity", None),
        D::Recurrence(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_recurrence", None),
        D::Ordering(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_position", None),
    };
    let err = ApiError::new(status, code, msg);
    match details {
        Some(d) => err.with_details(d),
        None => err,
    }
}

fn event_ref(id: EventId, seqs: &BTreeMap<EventId, u64>) -> Value {
    json!({ "id": id, "seq": seqs.get(&id) })
}

/// Maps an undo refusal to a 4xx response.
#[must_use]
pub fn undo_error(e: &UndoError, seqs: &BTreeMap<EventId, u64>) -> ApiError {
    use UndoError as U;
    let msg = e.to_string();
    match e {
        U::Denied(a) => ApiError::new(StatusCode::FORBIDDEN, "forbidden", msg).with_details(
            json!({ "needed": a.needed, "have": a.have, "owner_only": a.owner_only }),
        ),
        U::NotOwnEvent => ApiError::new(StatusCode::FORBIDDEN, "not_own_event", msg),
        U::UnknownEvent(id) => ApiError::new(StatusCode::NOT_FOUND, "event_not_found", msg)
            .with_details(json!({ "event": event_ref(*id, seqs) })),
        U::EventMismatch(id) => {
            // The server always passes the stored event, so this means the log and the in-memory
            // journal disagree.
            tracing::warn!(event = %id, "undo target does not match the applied event");
            ApiError::new(StatusCode::CONFLICT, "event_mismatch", msg)
                .with_details(json!({ "event": event_ref(*id, seqs) }))
        }
        U::Invariant { target, source } => {
            let inner = domain_error(source);
            ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "undo_invariant", msg).with_details(
                json!({
                    "entity": target,
                    "reason": inner.code,
                    "reason_details": inner.details,
                    "hint": "undo the dependent change first; force does not override this",
                }),
            )
        }
        U::AlreadyUndone(id) => ApiError::new(StatusCode::CONFLICT, "already_undone", msg)
            .with_details(json!({ "event": event_ref(*id, seqs) })),
        U::Conflict(c) => {
            let conflicts: Vec<Value> = c
                .conflicts
                .iter()
                .map(|f| {
                    json!({
                        "target": f.target,
                        "field": f.field,
                        "later_events": f.later_events.iter().map(|id| event_ref(*id, seqs)).collect::<Vec<_>>(),
                    })
                })
                .collect();
            ApiError::new(StatusCode::CONFLICT, "undo_conflict", msg).with_details(json!({
                "event": event_ref(c.event, seqs),
                "conflicts": conflicts,
                "hint": "retry with force: true to override",
            }))
        }
        U::HasDependents(id) => ApiError::new(StatusCode::CONFLICT, "has_dependents", msg)
            .with_details(json!({ "entity": id })),
        U::BrokenReference(id) => ApiError::new(StatusCode::CONFLICT, "broken_reference", msg)
            .with_details(json!({ "entity": id })),
        U::Apply(err) => {
            tracing::error!(error = %err, "undo produced an event that does not apply");
            ApiError::internal()
        }
    }
}

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::Domain(d) => domain_error(&d),
            EngineError::Undo { error, seqs } => undo_error(&error, &seqs),
            EngineError::EventNotFound(seq) => Self::new(
                StatusCode::NOT_FOUND,
                "event_not_found",
                format!("no event with seq {seq}"),
            ),
            EngineError::CommandIdReused(id) => Self::new(
                StatusCode::CONFLICT,
                "command_id_reused",
                format!("command id {id} was already used for a different request"),
            ),
            EngineError::EpochMismatch { current } => Self::epoch_mismatch(current),
            EngineError::UndoTargetMismatch {
                seq,
                expected,
                found,
            } => Self::new(
                StatusCode::CONFLICT,
                "event_mismatch",
                format!("event {seq} is not the expected event"),
            )
            .with_details(json!({ "seq": seq, "expected": expected, "found": found })),
            EngineError::Cancelled => Self::timeout(),
            EngineError::Unavailable => Self::unavailable(),
            EngineError::Store(s) => s.into(),
        }
    }
}

fn rejection(status: StatusCode, text: String) -> ApiError {
    let code = match status {
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        StatusCode::UNPROCESSABLE_ENTITY => "invalid_body",
        s if s.is_server_error() => return ApiError::internal(),
        _ => "bad_request",
    };
    ApiError::new(status, code, text)
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        rejection(r.status(), r.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(r: QueryRejection) -> Self {
        rejection(StatusCode::BAD_REQUEST, r.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(r: PathRejection) -> Self {
        rejection(StatusCode::BAD_REQUEST, r.body_text())
    }
}
