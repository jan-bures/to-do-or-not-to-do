//! JSON endpoints for the WebAuthn ceremonies, called by `/assets/auth.js`.
//!
//! Each ceremony has a `start` that returns `{ceremony, options}` (the options go to
//! `navigator.credentials.create()` or `.get()`) and a `finish` that takes `{ceremony, credential}`.
//! The server keeps the ceremony state in SQLite for five minutes and accepts each `ceremony` once.
//!
//! | Endpoint | Session | Does |
//! |---|---|---|
//! | `/auth/setup/{start,finish}` | none; the setup code | registers the first passkey, signs in |
//! | `/auth/login/{start,finish}` | none | signs in |
//! | `/auth/invite/{start,finish}` | none; the invite secret | registers a new passkey, signs in |
//! | `/auth/step-up/{start,finish}` | yes, `X-CSRF-Token` | marks the session freshly verified |
//! | `/auth/passkeys/{start,finish}` | yes, `X-CSRF-Token`, fresh | registers another passkey |
//!
//! Every one of them also needs `Origin: <public_origin>` (checked by the web router).

use super::safe_next;
use crate::api::error::ApiError;
use crate::app::AppState;
use crate::auth::session::{
    CSRF_HEADER, ClientIp, OwnerSession, blocking, check_csrf, session_cookie,
};
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

/// Most bytes of a ceremony request body.
const MAX_BODY: usize = 64 * 1024;

fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.len() > MAX_BODY {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "request body is too large",
        ));
    }
    let body: &[u8] = if body.iter().all(u8::is_ascii_whitespace) {
        b"{}"
    } else {
        body
    };
    serde_json::from_slice(body).map_err(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_body",
            e.to_string(),
        )
    })
}

fn csrf(headers: &HeaderMap, session: &OwnerSession) -> Result<(), ApiError> {
    check_csrf(
        &session.row,
        headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()),
    )
}

fn started(handle: String, options: impl serde::Serialize) -> Json<Value> {
    Json(json!({ "ceremony": handle, "options": options }))
}

fn signed_in(app: &AppState, cookie: &str, redirect: &str) -> Response {
    let mut res = Json(json!({ "redirect": redirect })).into_response();
    res.headers_mut().insert(
        SET_COOKIE,
        session_cookie(cookie, app.authsvc.session_max_age_secs()),
    );
    res
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupStart {
    code: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    ceremony: String,
    credential: RegisterPublicKeyCredential,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Assertion {
    ceremony: String,
    credential: PublicKeyCredential,
    #[serde(default)]
    next: Option<String>,
}

/// `POST /auth/setup/start`.
pub async fn setup_start(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let req: SetupStart = parse(&body)?;
    let (handle, options) =
        blocking(&app.authsvc, move |svc| svc.setup_start(&req.code, ip)).await??;
    Ok(started(handle, options))
}

/// `POST /auth/setup/finish`.
pub async fn setup_finish(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    body: Bytes,
) -> Result<Response, ApiError> {
    let req: Registration = parse(&body)?;
    let session = blocking(&app.authsvc, move |svc| {
        svc.setup_finish(&req.ceremony, &req.credential, ip)
    })
    .await??;
    Ok(signed_in(&app, &session.cookie, "/admin"))
}

/// `POST /auth/invite/start`. The invite, not any cookie sent with the request, authorizes this.
pub async fn invite_start(
    State(app): State<AppState>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let req: crate::api::invite::Redeem = parse(&body)?;
    let name = crate::api::invite::device_name(&req.device_name)?;
    let (handle, options) = blocking(&app.authsvc, move |svc| {
        svc.invite_passkey_start(&req.secret, name)
    })
    .await??;
    Ok(started(handle, options))
}

/// `POST /auth/invite/finish`.
pub async fn invite_finish(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    body: Bytes,
) -> Result<Response, ApiError> {
    let req: Registration = parse(&body)?;
    let session = blocking(&app.authsvc, move |svc| {
        svc.invite_passkey_finish(&req.ceremony, &req.credential, ip)
    })
    .await??;
    Ok(signed_in(&app, &session.cookie, "/"))
}

/// `POST /auth/login/start`.
pub async fn login_start(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
) -> Result<Json<Value>, ApiError> {
    let (handle, options) = blocking(&app.authsvc, move |svc| svc.login_start(ip)).await??;
    Ok(started(handle, options))
}

/// `POST /auth/login/finish`.
pub async fn login_finish(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    body: Bytes,
) -> Result<Response, ApiError> {
    let req: Assertion = parse(&body)?;
    let next = safe_next(req.next.as_deref());
    let session = blocking(&app.authsvc, move |svc| {
        svc.login_finish(&req.ceremony, &req.credential, ip)
    })
    .await??;
    Ok(signed_in(&app, &session.cookie, &next))
}

/// `POST /auth/step-up/start`.
pub async fn step_up_start(
    State(app): State<AppState>,
    session: OwnerSession,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    csrf(&headers, &session)?;
    let (handle, options) =
        blocking(&app.authsvc, move |svc| svc.step_up_start(&session.row)).await??;
    Ok(started(handle, options))
}

/// `POST /auth/step-up/finish`.
pub async fn step_up_finish(
    State(app): State<AppState>,
    session: OwnerSession,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    csrf(&headers, &session)?;
    let req: Assertion = parse(&body)?;
    blocking(&app.authsvc, move |svc| {
        svc.step_up_finish(&session.row, &req.ceremony, &req.credential, session.ip)
    })
    .await??;
    Ok(Json(json!({ "verified": true })))
}

/// `POST /auth/passkeys/start`.
pub async fn passkey_start(
    State(app): State<AppState>,
    session: OwnerSession,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    csrf(&headers, &session)?;
    let (handle, options) =
        blocking(&app.authsvc, move |svc| svc.passkey_add_start(&session.row)).await??;
    Ok(started(handle, options))
}

/// `POST /auth/passkeys/finish`.
pub async fn passkey_finish(
    State(app): State<AppState>,
    session: OwnerSession,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    csrf(&headers, &session)?;
    let req: Registration = parse(&body)?;
    blocking(&app.authsvc, move |svc| {
        svc.passkey_add_finish(
            &session.row,
            &req.ceremony,
            &req.credential,
            req.name.as_deref().unwrap_or_default(),
            session.ip,
        )
    })
    .await??;
    Ok(Json(json!({ "added": true })))
}
