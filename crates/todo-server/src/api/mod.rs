//! The REST API under `/api/v1`.

pub mod attachments;
pub mod commands;
pub mod dto;
pub mod error;
pub mod events;
pub mod hidden;
pub(crate) mod invite;
pub mod openapi;
pub mod pair;
pub mod read;
pub mod visibility;

use crate::app::AppState;
use crate::auth;
use crate::cancel::Cancel;
use axum::Router;
use axum::extract::{FromRequest, FromRequestParts, MatchedPath, Request};
use axum::http::request::Parts;
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::Response;
use axum::routing::{delete, get, post};
use error::ApiError;

/// JSON body extractor whose rejections are typed [`ApiError`]s (413 for oversized bodies).
#[derive(Debug, FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct ApiJson<T>(pub T);

/// Query-string extractor with typed rejections.
#[derive(Debug, FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
pub struct ApiQuery<T>(pub T);

/// Path extractor with typed rejections.
#[derive(Debug, FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct ApiPath<T>(pub T);

/// The request's [`Cancel`], set by the deadline middleware in [`crate::app`]. Without the middleware
/// (handlers mounted on their own) it is a fresh, never-cancelled one.
#[derive(Clone, Debug)]
pub struct RequestCancel(pub Cancel);

impl<S: Send + Sync> FromRequestParts<S> for RequestCancel {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<Cancel>()
                .cloned()
                .unwrap_or_default(),
        ))
    }
}

/// Which admission pool a piece of blocking work draws from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// Reads of the state or the log (`limits.max_concurrent_reads`).
    Read,
    /// Commands and undos (`limits.max_concurrent_writes`).
    Write,
}

/// Runs blocking engine work on the blocking pool, after bounded admission.
///
/// The permit is taken *before* the work is spawned, without waiting: when the pool is full the
/// request gets 503 `overloaded` with `Retry-After`. The permit moves into the blocking closure, so it
/// is released when the work really ends, not when a timed-out handler gives up on it. Blocking
/// threads in use are therefore bounded by the two pool sizes, whatever the clients do.
pub async fn blocking<T, F>(app: &AppState, work: Work, f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    if !app.engine.is_healthy() {
        return Err(ApiError::unavailable());
    }
    let pool = match work {
        Work::Read => &app.reads,
        Work::Write => &app.writes,
    };
    let permit = pool
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::overloaded())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "blocking task failed");
        ApiError::internal()
    })
}

/// The route template a response was produced by (for logging), set by [`tag_route`].
#[derive(Clone, Debug)]
pub struct RouteLabel(pub String);

/// Route-level middleware that records the matched route template on the response, so the access log
/// can name the route without logging the raw path.
pub async fn tag_route(req: Request, next: Next) -> Response {
    let label = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned());
    let mut res = next.run(req).await;
    if let Some(l) = label {
        res.extensions_mut().insert(RouteLabel(l));
    }
    res
}

/// The answer for a path no route matches. Owner-only routes give agents exactly this, so they look
/// absent.
pub(crate) fn no_such_endpoint() -> ApiError {
    ApiError::not_found("no such endpoint")
}

async fn not_found() -> ApiError {
    no_such_endpoint()
}

/// The API router, to be nested at `/api/v1`. Everything needs a bearer token (or the web app's
/// session, see [`auth::require_auth_or_session`]) except the public pairing
/// endpoints and the email ingest endpoint (HMAC-signed).
pub fn router(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/pair/start", post(pair::start))
        .route("/pair/poll", post(pair::poll))
        .route("/pair/redeem", post(invite::redeem))
        // No bearer token: signed with HMAC instead, and 404 unless configured (see `crate::ingest`).
        .route("/ingest/email", post(crate::ingest::post_email));
    Router::new()
        .route("/openapi.json", get(openapi::document))
        .route("/pair/invites", post(invite::create))
        .route(
            "/pair/invites/{id}",
            get(invite::status).delete(invite::cancel),
        )
        .route("/pair/review", post(invite::review))
        .route("/pair/device", delete(pair::revoke_self))
        .route("/commands", post(commands::post_command))
        .route("/events", get(events::get_events))
        .route("/events/{seq}/undo", post(commands::post_undo))
        .route("/activity", get(read::activity))
        .route("/collections", get(read::collections))
        .route("/lists", get(read::lists))
        .route("/lists/{id}/tasks", get(read::list_tasks))
        .route("/tasks", get(read::query_tasks))
        .route("/tasks/{id}", get(read::task_detail))
        .route(
            "/attachments/{id}",
            get(attachments::download).head(attachments::head),
        )
        .route("/tags", get(read::tags))
        .route("/smart-lists", get(read::smart_lists))
        .route("/smart-lists/{id}/tasks", get(read::smart_list_tasks))
        .fallback(not_found)
        // Bearer tokens, or the owner's browser session for the web app (CSRF header required).
        .layer(from_fn_with_state(state, auth::require_auth_or_session))
        .merge(public)
        // Outside auth, so rejected requests are labelled too; routes only, so the fallback is not.
        .route_layer(from_fn(tag_route))
}
