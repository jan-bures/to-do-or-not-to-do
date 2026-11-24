//! Web Push registrations are owned by the current passkey session, never by a bearer token.

use super::error::ApiError;
use super::{ApiJson, ApiPath, RequestCancel, Work, blocking, no_such_endpoint};
use crate::app::AppState;
use crate::auth::store::web_push::Registered;
use crate::auth::{Authenticated, Principal};
use crate::web_push::{Delivery, Registration, Sender, endpoint};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use todo_core::ids::TaskId;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
struct PublicKey<'a> {
    public_key: &'a str,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Endpoint {
    endpoint: String,
}

impl Endpoint {
    fn validate(self) -> Result<String, ApiError> {
        endpoint(&self.endpoint).map(Into::into).ok_or_else(invalid)
    }
}

#[derive(Serialize, ToSchema)]
pub(crate) struct SubscriptionStatus {
    active: bool,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct Title {
    title: String,
}

fn session(caller: Authenticated) -> Result<String, ApiError> {
    match caller.principal {
        Principal::Session { session, .. } => Ok(session),
        _ => Err(no_such_endpoint()),
    }
}

fn sender(app: &AppState) -> Result<Arc<Sender>, ApiError> {
    app.web_push.clone().ok_or_else(no_such_endpoint)
}

fn invalid() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid",
        "invalid Web Push subscription",
    )
}

#[utoipa::path(get, path = "/api/v1/web-push", tag = "web-push",
    responses((status = 200, body = PublicKey), (status = 204, description = "Web Push not configured")),
    security(("web_session" = [], "csrf" = [])))]
pub(crate) async fn config(
    State(app): State<AppState>,
    caller: Authenticated,
) -> Result<Response, ApiError> {
    session(caller)?;
    Ok(match app.web_push.as_deref() {
        Some(sender) => Json(PublicKey {
            public_key: sender.public_key(),
        })
        .into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

#[utoipa::path(put, path = "/api/v1/web-push", tag = "web-push", request_body = Registration,
    responses((status = 204, description = "Registered"), (status = 409, description = "Session subscription cap reached")),
    security(("web_session" = [], "csrf" = [])))]
pub(crate) async fn register(
    State(app): State<AppState>,
    caller: Authenticated,
    RequestCancel(cancel): RequestCancel,
    ApiJson(body): ApiJson<Registration>,
) -> Result<StatusCode, ApiError> {
    let session = session(caller)?;
    let sender = sender(&app)?;
    let target = body.validate().ok_or_else(invalid)?;
    let auth = app.authsvc.clone();
    blocking(&app, Work::Write, move || {
        if !cancel.begin_commit() {
            return Err(ApiError::timeout());
        }
        match auth
            .store()
            .register_web_push(&session, sender.public_key(), &target, auth.now())?
        {
            Registered::Stored => Ok(StatusCode::NO_CONTENT),
            Registered::MissingSession => Err(ApiError::unauthorized()),
            Registered::AtCapacity => Err(ApiError::new(
                StatusCode::CONFLICT,
                "subscription_cap",
                "this session has too many Web Push subscriptions",
            )),
        }
    })
    .await?
}

#[utoipa::path(delete, path = "/api/v1/web-push", tag = "web-push", request_body = Endpoint,
    responses((status = 204, description = "Unregistered")), security(("web_session" = [], "csrf" = [])))]
pub(crate) async fn unregister(
    State(app): State<AppState>,
    caller: Authenticated,
    RequestCancel(cancel): RequestCancel,
    ApiJson(body): ApiJson<Endpoint>,
) -> Result<StatusCode, ApiError> {
    let session = session(caller)?;
    let endpoint = body.validate()?;
    let auth = app.authsvc.clone();
    blocking(&app, Work::Write, move || {
        if !cancel.begin_commit() {
            return Err(ApiError::timeout());
        }
        auth.store().unregister_web_push(&session, &endpoint)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await?
}

#[utoipa::path(post, path = "/api/v1/web-push/status", tag = "web-push", request_body = Endpoint,
    responses((status = 200, body = SubscriptionStatus)), security(("web_session" = [], "csrf" = [])))]
pub(crate) async fn status(
    State(app): State<AppState>,
    caller: Authenticated,
    ApiJson(body): ApiJson<Endpoint>,
) -> Result<Json<SubscriptionStatus>, ApiError> {
    let session = session(caller)?;
    let sender = sender(&app)?;
    let endpoint = body.validate()?;
    let auth = app.authsvc.clone();
    blocking(&app, Work::Read, move || {
        let active = auth
            .store()
            .has_web_push(&session, &endpoint, sender.public_key())?;
        Ok(Json(SubscriptionStatus { active }))
    })
    .await?
}

#[utoipa::path(post, path = "/api/v1/web-push/test", tag = "web-push", request_body = Endpoint,
    responses((status = 204, description = "Accepted by the push service")), security(("web_session" = [], "csrf" = [])))]
pub(crate) async fn test(
    State(app): State<AppState>,
    caller: Authenticated,
    RequestCancel(cancel): RequestCancel,
    ApiJson(body): ApiJson<Endpoint>,
) -> Result<StatusCode, ApiError> {
    let session = session(caller)?;
    let sender = sender(&app)?;
    let endpoint = body.validate()?;
    let (auth, worker) = (app.authsvc.clone(), sender.clone());
    let target = blocking(&app, Work::Write, move || {
        if !cancel.begin_commit() {
            return Err(ApiError::timeout());
        }
        match auth.store().claim_web_push_test(
            &session,
            &endpoint,
            worker.public_key(),
            auth.now(),
        )? {
            Some(target) => Ok(target),
            None if auth
                .store()
                .has_web_push(&session, &endpoint, worker.public_key())? =>
            {
                Err(ApiError::too_many_requests(30))
            }
            None => Err(no_such_endpoint()),
        }
    })
    .await??;
    match sender.deliver(&app.authsvc, target, b"null", 60).await {
        Delivery::Accepted => Ok(StatusCode::NO_CONTENT),
        Delivery::Gone => Err(ApiError::new(
            StatusCode::GONE,
            "subscription_expired",
            "enable Web Push again",
        )),
        Delivery::Failed => Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "push_failed",
            "the push service did not accept the notification",
        )),
    }
}

#[utoipa::path(get, path = "/api/v1/web-push/tasks/{id}", tag = "web-push",
    params(("id" = TaskId, Path, description = "Task ID")),
    responses((status = 200, body = Title)), security(("web_session" = [], "csrf" = [])))]
pub(crate) async fn title(
    State(app): State<AppState>,
    caller: Authenticated,
    ApiPath(id): ApiPath<TaskId>,
) -> Result<Json<Title>, ApiError> {
    session(caller)?;
    sender(&app)?;
    let state = app.engine.state();
    let task = state
        .tasks
        .get(&id)
        .filter(|task| task.completed_at.is_none() && state.is_live(id.into()))
        .ok_or_else(|| ApiError::not_found("task is not open"))?;
    Ok(Json(Title {
        title: task.title.clone(),
    }))
}
