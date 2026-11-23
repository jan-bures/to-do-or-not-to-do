//! Push settings and registration for a single paired bearer token.

use super::error::{ApiError, ErrorBody};
use super::{ApiJson, RequestCancel, Work, blocking, no_such_endpoint};
use crate::app::AppState;
use crate::auth::{Authenticated, Principal};
use crate::config::PushConfig;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use todo_core::DeviceId;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct FirebaseOptions<'a> {
    project_id: &'a str,
    application_id: &'a str,
    api_key: &'a str,
    gcm_sender_id: &'a str,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Registration {
    token: String,
}

fn paired(
    app: &AppState,
    caller: Authenticated,
    headers: &HeaderMap,
) -> Result<DeviceId, ApiError> {
    match caller.principal {
        Principal::Owner { device } if app.authsvc.paired_device(&device, headers) => Ok(device),
        _ => Err(no_such_endpoint()),
    }
}

fn options(config: &PushConfig) -> FirebaseOptions<'_> {
    FirebaseOptions {
        project_id: &config.project_id,
        application_id: &config.application_id,
        api_key: &config.api_key,
        gcm_sender_id: &config.gcm_sender_id,
    }
}

#[utoipa::path(get, path = "/api/v1/pair/push", tag = "pairing",
    responses((status = 200, body = FirebaseOptions), (status = 204, description = "Push not configured"),
        (status = 401, body = ErrorBody), (status = 404, body = ErrorBody)),
    security(("bearer" = [])))]
pub(crate) async fn config(
    State(app): State<AppState>,
    caller: Authenticated,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    paired(&app, caller, &headers)?;
    let response = app.push.as_deref().map_or_else(
        || StatusCode::NO_CONTENT.into_response(),
        |p| Json(options(p)).into_response(),
    );
    Ok(([(header::CACHE_CONTROL, "no-store")], response).into_response())
}

#[utoipa::path(put, path = "/api/v1/pair/push", tag = "pairing", request_body = Registration,
    responses((status = 204, description = "Registration replaced"), (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 422, body = ErrorBody)), security(("bearer" = [])))]
pub(crate) async fn register(
    State(app): State<AppState>,
    caller: Authenticated,
    headers: HeaderMap,
    RequestCancel(cancel): RequestCancel,
    ApiJson(body): ApiJson<Registration>,
) -> Result<StatusCode, ApiError> {
    let device = paired(&app, caller, &headers)?;
    let push = app.push.clone().ok_or_else(no_such_endpoint)?;
    if !crate::push::valid_registration(&body.token) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "invalid push registration",
        ));
    }
    let auth = app.authsvc.clone();
    blocking(&app, Work::Write, move || {
        if !cancel.begin_commit() {
            return Err(ApiError::timeout());
        }
        if !auth
            .store()
            .register_push(device.as_str(), &push.project_id, &body.token)?
        {
            return Err(no_such_endpoint());
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await?
}

#[utoipa::path(delete, path = "/api/v1/pair/push", tag = "pairing",
    responses((status = 204, description = "Registration removed"), (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody)), security(("bearer" = [])))]
pub(crate) async fn unregister(
    State(app): State<AppState>,
    caller: Authenticated,
    headers: HeaderMap,
    RequestCancel(cancel): RequestCancel,
) -> Result<StatusCode, ApiError> {
    let device = paired(&app, caller, &headers)?;
    let auth = app.authsvc.clone();
    blocking(&app, Work::Write, move || {
        if !cancel.begin_commit() {
            return Err(ApiError::timeout());
        }
        auth.store().unregister_push(device.as_str())?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await?
}
