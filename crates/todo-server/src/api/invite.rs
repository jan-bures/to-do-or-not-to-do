//! Owner-only invite management and pending-device approval; anonymous native redemption.

use super::error::{ApiError, ErrorBody};
use super::{ApiJson, no_such_endpoint};
use crate::app::AppState;
use crate::auth::invite::{Issuer, Status};
use crate::auth::pairing::{self, StartError};
use crate::auth::service::{audit, no_origin};
use crate::auth::session::{ClientIp, blocking};
use crate::auth::{Authenticated, Principal};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use todo_core::DeviceId;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct Invitation {
    id: String,
    code: String,
    /// The 256-bit secret is in the URL fragment only.
    link: String,
    expires_in: u32,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InviteStatus {
    Pending,
    Used { device_name: String },
    Cancelled,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Redeem {
    /// A typed code or the secret from the invite link's fragment, not the URL itself.
    pub(crate) secret: String,
    pub(crate) device_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct DeviceCredential {
    #[schema(value_type = String)]
    device_id: DeviceId,
    token: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Review {
    code: String,
    /// Omit to preview; set to `created_at` from the preview to confirm that exact request.
    approve_at: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct PendingDevice {
    device_name: String,
    created_at: i64,
    /// Address seen by the server for the original pairing request, if known.
    source: Option<String>,
}

pub(crate) fn invite_gone() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "invite_not_found",
        "the invite is invalid, expired or already used",
    )
}

pub(crate) fn device_name(name: &str) -> Result<String, ApiError> {
    pairing::valid_device_name(name).map_err(|e| match e {
        StartError::BadName => {
            ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", e.to_string())
        }
        _ => ApiError::internal(),
    })
}

fn limited(app: &AppState, ip: ClientIp, poll: bool) -> Result<(), ApiError> {
    let ip =
        ip.0.unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let limiter = if poll {
        &app.authsvc.poll_limiter
    } else {
        &app.authsvc.ip_limiter
    };
    limiter.check(&ip).map_err(ApiError::too_many_requests)
}

async fn issuer(
    app: &AppState,
    caller: Authenticated,
    headers: &HeaderMap,
    fresh: bool,
) -> Result<Issuer, ApiError> {
    match caller.principal {
        Principal::Owner { device } if app.authsvc.paired_device(&device, headers) => {
            Ok(Issuer::Device(device))
        }
        Principal::Session { session, .. } => {
            // The API middleware already checked the cookie, CSRF token and Origin. Re-read the
            // row to enforce step-up and ensure it was not deleted in the meantime.
            blocking(&app.authsvc, move |svc| {
                let row = svc
                    .store()
                    .session(&session)?
                    .ok_or_else(ApiError::unauthorized)?;
                if fresh {
                    svc.require_step_up(&row)?;
                }
                Ok(Issuer::Session(session))
            })
            .await?
        }
        _ => Err(no_such_endpoint()),
    }
}

/// Creates a one-time invite. Requires a paired owner device or fresh owner session.
#[utoipa::path(post, path = "/api/v1/pair/invites", tag = "pairing",
    responses((status = 200, body = Invitation), (status = 403, body = ErrorBody),
        (status = 429, body = ErrorBody), (status = 503, body = ErrorBody)),
    security(("bearer" = []), ("web_session" = [], "csrf" = [])))]
pub async fn create(
    State(app): State<AppState>,
    ip: ClientIp,
    caller: Authenticated,
    headers: HeaderMap,
) -> Result<Json<Invitation>, ApiError> {
    let issuer = issuer(&app, caller, &headers, true).await?;
    limited(&app, ip, false)?;
    let origin = app.authsvc.origin().ok_or_else(no_origin)?.as_str();
    let created = app
        .authsvc
        .invites
        .create(issuer, app.authsvc.now())?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "busy",
                "too many invites; wait for them to expire",
            )
        })?;
    Ok(Json(Invitation {
        id: created.id,
        code: created.code,
        link: format!("{origin}/pair/invite#{}", created.secret),
        expires_in: created.expires_in,
    }))
}

/// Reads the issuer's own invitation status without returning its credentials.
#[utoipa::path(get, path = "/api/v1/pair/invites/{id}", tag = "pairing",
    params(("id" = String, Path)),
    responses((status = 200, body = InviteStatus), (status = 404, body = ErrorBody), (status = 429, body = ErrorBody)),
    security(("bearer" = []), ("web_session" = [], "csrf" = [])))]
pub async fn status(
    State(app): State<AppState>,
    ip: ClientIp,
    caller: Authenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<InviteStatus>, ApiError> {
    let issuer = issuer(&app, caller, &headers, false).await?;
    limited(&app, ip, true)?;
    let status = app
        .authsvc
        .invites
        .status(&issuer, &id, app.authsvc.now())
        .ok_or_else(invite_gone)?;
    Ok(Json(match status {
        Status::Pending => InviteStatus::Pending,
        Status::Cancelled => InviteStatus::Cancelled,
        Status::Used { device_name } => InviteStatus::Used { device_name },
    }))
}

/// Cancels an unused invite. Only the issuer may cancel it.
#[utoipa::path(delete, path = "/api/v1/pair/invites/{id}", tag = "pairing",
    params(("id" = String, Path)),
    responses((status = 204), (status = 404, body = ErrorBody), (status = 429, body = ErrorBody)),
    security(("bearer" = []), ("web_session" = [], "csrf" = [])))]
pub async fn cancel(
    State(app): State<AppState>,
    ip: ClientIp,
    caller: Authenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let issuer = issuer(&app, caller, &headers, false).await?;
    limited(&app, ip, false)?;
    if !app.authsvc.invites.cancel(&issuer, &id, app.authsvc.now()) {
        return Err(invite_gone());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Exchanges an invite for a new native device token. Only one redemption can succeed.
#[utoipa::path(post, path = "/api/v1/pair/redeem", tag = "pairing", request_body = Redeem,
    responses((status = 200, body = DeviceCredential), (status = 404, body = ErrorBody),
        (status = 422, body = ErrorBody), (status = 429, body = ErrorBody)), security(()))]
pub async fn redeem(
    State(app): State<AppState>,
    ip: ClientIp,
    ApiJson(req): ApiJson<Redeem>,
) -> Result<Json<DeviceCredential>, ApiError> {
    limited(&app, ip, false)?;
    let name = device_name(&req.device_name)?;
    let credential = blocking(&app.authsvc, move |svc| {
        let claim = svc
            .invites
            .redeem(&req.secret, svc.now())
            .ok_or_else(invite_gone)?;
        let result = svc
            .create_invited_device(&name, ip.0, &claim.issuer)
            .map(|(device_id, token)| DeviceCredential { device_id, token });
        svc.invites
            .finish(&claim, result.as_ref().ok().map(|_| name), svc.now());
        result
    })
    .await??;
    Ok(Json(credential))
}

/// Previews or approves a pending pairing. Requires a paired owner device or fresh owner session.
#[utoipa::path(post, path = "/api/v1/pair/review", tag = "pairing", request_body = Review,
    responses((status = 200, body = PendingDevice), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 429, body = ErrorBody)),
    security(("bearer" = []), ("web_session" = [], "csrf" = [])))]
pub async fn review(
    State(app): State<AppState>,
    ip: ClientIp,
    caller: Authenticated,
    headers: HeaderMap,
    ApiJson(req): ApiJson<Review>,
) -> Result<Json<PendingDevice>, ApiError> {
    let issuer = issuer(&app, caller, &headers, true).await?;
    limited(&app, ip, false)?;
    let svc = &app.authsvc;
    let p = svc
        .look_up_pairing_code(issuer.clone(), || {
            if req.approve_at.is_some() {
                svc.pairings
                    .approve_reviewed(&req.code, req.approve_at, &issuer, svc.now())
            } else {
                svc.pairings.pending(&req.code, svc.now())
            }
        })?
        .ok_or_else(super::pair::gone)?;
    if req.approve_at.is_some() {
        let name = p.device_name.clone();
        blocking(&app.authsvc, move |svc| {
            svc.audit(
                audit::PAIRING_APPROVED,
                ip.0,
                json!({ "device_name": name, "issuer": issuer }),
            );
        })
        .await?;
    }
    Ok(Json(PendingDevice {
        device_name: p.device_name,
        created_at: p.created_at,
        source: p.source.map(|ip| ip.to_string()),
    }))
}
