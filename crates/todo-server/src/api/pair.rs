//! Device pairing: `POST /api/v1/pair/start` and `POST /api/v1/pair/poll`. Both are reachable without
//! a token (that is their point) and are rate-limited per client IP.
//!
//! 1. The client starts a pairing with its device name and shows the owner the `user_code` and
//!    `verification_uri`.
//! 2. The owner opens the page, signs in with a passkey and approves (or denies) the request.
//! 3. The client polls every `interval` seconds with its `poll_token`. Once approved, the poll returns
//!    the device token, exactly once. Requests expire after five minutes.

use super::ApiJson;
use super::error::{ApiError, ErrorBody};
use crate::app::AppState;
use crate::auth::pairing::{POLL_INTERVAL_SECS, Poll, StartError};
use crate::auth::service::no_origin;
use crate::auth::session::{ClientIp, blocking};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use todo_core::DeviceId;
use utoipa::ToSchema;

/// `POST /api/v1/pair/start` body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PairStart {
    /// A name for the device, shown to the owner when approving (1–64 printable characters).
    pub device_name: String,
}

/// A started pairing.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PairStarted {
    /// Show this to the owner: they confirm it on the verification page.
    pub user_code: String,
    /// Secret; keep it to poll with. Never show it.
    pub poll_token: String,
    /// The page the owner opens (it carries the user code).
    pub verification_uri: String,
    /// Seconds until the request expires.
    pub expires_in: u32,
    /// Seconds to wait between polls.
    pub interval: u32,
}

/// `POST /api/v1/pair/poll` body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PairPoll {
    /// The `poll_token` from the start response.
    pub poll_token: String,
}

/// Where a pairing stands.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PairStatus {
    /// Not decided yet; poll again after `interval` seconds.
    Pending {
        /// Seconds to wait.
        interval: u32,
    },
    /// Approved. This is the only time the token is sent: store it securely.
    Approved {
        /// The device's ID (the actor recorded on its changes).
        #[schema(value_type = String)]
        device_id: DeviceId,
        /// The device's bearer token.
        token: String,
    },
    /// The owner denied the request.
    Denied,
}

fn limited(app: &AppState, ip: ClientIp, poll: bool) -> Result<(), ApiError> {
    let Some(ip) = ip.0 else { return Ok(()) };
    let limiter = if poll {
        &app.authsvc.poll_limiter
    } else {
        &app.authsvc.ip_limiter
    };
    limiter.check(&ip).map_err(ApiError::too_many_requests)
}

/// Starts pairing a device.
#[utoipa::path(
    post, path = "/api/v1/pair/start", tag = "pairing", request_body = PairStart,
    responses(
        (status = 200, body = PairStarted),
        (status = 422, description = "Bad device name", body = ErrorBody),
        (status = 429, description = "Too many requests from this address, or too many of its pairings pending (`too_many_pairings`); see `Retry-After`", body = ErrorBody),
        (status = 503, description = "Pairing is not configured, or too many requests are pending", body = ErrorBody),
    ),
    security(())
)]
pub async fn start(
    State(app): State<AppState>,
    ip: ClientIp,
    ApiJson(req): ApiJson<PairStart>,
) -> Result<Json<PairStarted>, ApiError> {
    limited(&app, ip, false)?;
    let origin = app
        .authsvc
        .origin()
        .ok_or_else(no_origin)?
        .as_str()
        .to_owned();
    let started = app
        .authsvc
        .pairings
        .start(
            &req.device_name,
            ip.0,
            app.authsvc.config().pairings_per_ip,
            app.authsvc.now(),
        )
        .map_err(|e| match e {
            StartError::BadName => {
                ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", e.to_string())
            }
            StartError::Full => {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "busy", e.to_string())
                    .with_retry_after(60)
            }
            StartError::TooManyFromSource => ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_pairings",
                e.to_string(),
            )
            .with_retry_after(60),
            StartError::Random(r) => r.into(),
        })?;
    let expires_in =
        u32::try_from((started.expires_at - app.authsvc.now()).max(0) / 1000).unwrap_or(0);
    Ok(Json(PairStarted {
        verification_uri: format!("{origin}/pair?code={}", started.user_code),
        user_code: started.user_code,
        poll_token: started.poll_token,
        expires_in,
        interval: POLL_INTERVAL_SECS,
    }))
}

/// Polls a pairing.
#[utoipa::path(
    post, path = "/api/v1/pair/poll", tag = "pairing", request_body = PairPoll,
    responses(
        (status = 200, body = PairStatus),
        (status = 404, description = "Unknown, expired or already finished (`pairing_not_found`)", body = ErrorBody),
        (status = 429, description = "Polling too fast; see `Retry-After`", body = ErrorBody),
    ),
    security(())
)]
pub async fn poll(
    State(app): State<AppState>,
    ip: ClientIp,
    ApiJson(req): ApiJson<PairPoll>,
) -> Result<Response, ApiError> {
    limited(&app, ip, true)?;
    if req.poll_token.len() > 128 {
        return Err(gone());
    }
    let status = match app
        .authsvc
        .pairings
        .poll(&req.poll_token, app.authsvc.now())
    {
        Poll::Pending => PairStatus::Pending {
            interval: POLL_INTERVAL_SECS,
        },
        Poll::Denied => PairStatus::Denied,
        Poll::Gone => return Err(gone()),
        Poll::Approved { device_name } => {
            let (device_id, token) = blocking(&app.authsvc, move |svc| {
                svc.create_device(&device_name, ip.0)
            })
            .await??;
            PairStatus::Approved { device_id, token }
        }
    };
    Ok(Json(status).into_response())
}

fn gone() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "pairing_not_found",
        "no such pairing request: it expired, was finished, or never existed; start again",
    )
}
