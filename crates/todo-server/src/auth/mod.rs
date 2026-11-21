//! Authentication and authorization.
//!
//! Two kinds of callers reach the server:
//!
//! - **API clients** send `Authorization: Bearer <token>` to `/api/v1`: a paired device token
//!   ([`Principal::Owner`]), an agent token ([`Principal::Agent`]), or, only with
//!   `auth.allow_config_owner_tokens`, a static owner token from the config ([`OwnerTokens`]). The API
//!   router runs [`require_auth`] as a layer, so unknown paths and handlers that do not take the
//!   extractor are protected too; handlers take [`Authenticated`] to learn who is calling.
//! - **The owner's browser** signs in with a passkey and gets a session cookie for the admin pages
//!   ([`session`]). Bearer tokens never open the admin pages, so agents cannot manage tokens, share
//!   links, devices or grants.
//! - **The web app** (`/app`) uses that same session for the REST API under `/api/v1`, and only
//!   there ([`require_auth_or_session`]): every request must carry the session's CSRF token in
//!   `X-CSRF-Token`, and every write the public `Origin`. It acts as the owner on the device
//!   [`WEB_DEVICE`]. `/mcp` and the attachment upload stay bearer-only.
//! - **A local operator** may use the admin API on its own loopback listener with the `tdx_` admin
//!   token (see [`crate::admin`]). That token is unknown here: every route of the public listener
//!   refuses it.
//!
//! [`service::AuthService`] holds the state; [`store`] persists it.

mod owner_token;
mod principal;

pub mod limits;
pub mod pairing;
pub mod secret;
pub mod service;
pub mod session;
pub mod store;

pub use owner_token::{OwnerTokens, hash_token};
pub use principal::{Access, Principal};

use crate::api::error::ApiError;
use crate::app::AppState;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use todo_core::{Actor, DeviceId};

/// Longest bearer token accepted; anything longer is rejected before hashing.
pub const MAX_TOKEN_LEN: usize = 512;

/// Why a request could not be authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// No credentials were sent.
    #[error("missing credentials")]
    Missing,
    /// The credentials are malformed, unknown, revoked or expired.
    #[error("invalid credentials")]
    Invalid,
}

/// Turns request credentials into an authenticated caller.
pub trait Authenticator: Send + Sync + std::fmt::Debug + 'static {
    /// Authenticates a request from its headers.
    fn authenticate(&self, headers: &HeaderMap) -> Result<Authenticated, AuthError>;
}

/// The bearer token of a request.
pub(crate) fn bearer(headers: &HeaderMap) -> Result<&str, AuthError> {
    let value = headers.get(AUTHORIZATION).ok_or(AuthError::Missing)?;
    let value = value.to_str().map_err(|_| AuthError::Invalid)?;
    let (scheme, token) = value.split_once(' ').ok_or(AuthError::Invalid)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(AuthError::Invalid);
    }
    let token = token.trim();
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return Err(AuthError::Invalid);
    }
    Ok(token)
}

/// An authenticated API caller: the principal, the actor its events are recorded under, and its
/// access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authenticated {
    /// Who is calling.
    pub principal: Principal,
    /// The `todo-core` actor for commands, undo and read filtering.
    pub actor: Actor,
    /// What the caller may do.
    pub access: Access,
}

impl Authenticated {
    /// The owner on `device`, with full access.
    #[must_use]
    pub fn owner(device: DeviceId) -> Self {
        let principal = Principal::Owner { device };
        Self {
            actor: principal.actor(),
            principal,
            access: Access::Full,
        }
    }

    /// True for an agent.
    #[must_use]
    pub fn is_agent(&self) -> bool {
        self.principal.is_agent()
    }

    /// Counts a write against an agent's quota (owners are not limited). 429 when exhausted.
    pub fn take_write(&self, app: &AppState) -> Result<(), ApiError> {
        match (&self.principal, &self.access) {
            (Principal::Agent { token, .. }, Access::Agent { writes_per_hour }) => app
                .authsvc
                .quotas
                .take(*token, *writes_per_hour)
                .map_err(ApiError::quota_exceeded),
            _ => Ok(()),
        }
    }
}

/// The TCP peer of a request (set by [`crate::conn`]).
#[must_use]
pub fn peer_ip(parts_ext: &axum::http::Extensions) -> Option<std::net::IpAddr> {
    parts_ext.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip())
}

fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<Authenticated, ApiError> {
    state.auth.authenticate(headers).map_err(|e| {
        tracing::debug!(reason = %e, "rejected request");
        ApiError::unauthorized()
    })
}

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(a) = parts.extensions.get::<Self>() {
            return Ok(a.clone());
        }
        let a = authenticate(state, &parts.headers)?;
        parts.extensions.insert(a.clone());
        Ok(a)
    }
}

/// The device the owner's browser sessions act as on the API (the web app).
pub const WEB_DEVICE: &str = "web";

/// Like [`require_auth`], but a request **without** an `Authorization` header may instead present the
/// owner's browser session (the web app at `/app`). Mounted on the REST API only.
///
/// A session request must carry, on every method, the session's CSRF token in `X-CSRF-Token`: a
/// custom header no other site can make a browser send (it needs a CORS preflight, which this server
/// never grants), and that only this origin's pages know. A write must also carry `Origin:
/// <public_origin>`. `SameSite=Strict` on the cookie is a third layer. An unknown or expired session
/// is a 401 like a wrong token (and counts against the address in the same way); a live session with
/// a missing or wrong CSRF token or origin is 403 `csrf`.
pub async fn require_auth_or_session(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if req.headers().contains_key(AUTHORIZATION) {
        return require_auth(State(state), req, next).await;
    }
    let Some(cookie) = session::cookie_value(req.headers()) else {
        return require_auth(State(state), req, next).await;
    };
    let ip = state
        .authsvc
        .client_ip(req.headers(), peer_ip(req.extensions()));
    if let Err(wait) = state.authsvc.bearer_admission(ip) {
        return ApiError::too_many_requests(wait).into_response();
    }
    // Looked up without recording activity: only a request that passes the CSRF and origin checks
    // keeps the session alive.
    let row = match session::blocking(&state.authsvc, move |svc| svc.peek_session(&cookie)).await {
        Ok(Ok(Some(row))) => row,
        Ok(Ok(None)) => {
            tracing::debug!("rejected request: unknown or expired session");
            state.authsvc.bearer_failed(ip);
            return ApiError::unauthorized().into_response();
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "session lookup failed");
            return ApiError::internal().into_response();
        }
        Err(e) => return e.into_response(),
    };
    let presented = req
        .headers()
        .get(session::CSRF_HEADER)
        .and_then(|v| v.to_str().ok());
    if let Err(e) = session::check_csrf(&row, presented) {
        return e.into_response();
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD)
        && let Err(e) = session::check_origin(req.headers(), &state.authsvc)
    {
        return e.into_response();
    }
    let touched = row.clone();
    match session::blocking(&state.authsvc, move |svc| svc.touch_session(&touched)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::error!(error = %e, "session touch failed");
            return ApiError::internal().into_response();
        }
        Err(e) => return e.into_response(),
    }
    let Ok(device) = DeviceId::new(WEB_DEVICE) else {
        return ApiError::internal().into_response();
    };
    let principal = Principal::Session {
        session: row.id_hash,
        device,
    };
    let a = Authenticated {
        actor: principal.actor(),
        principal,
        access: Access::Full,
    };
    if let Err(wait) = state.authsvc.api_limiter.check(&a.principal) {
        return ApiError::too_many_requests(wait).into_response();
    }
    req.extensions_mut().insert(a);
    next.run(req).await
}

/// Middleware that turns away addresses that failed authentication too often (429, before looking
/// at the token), rejects unauthenticated requests with 401, applies the per-token request limit, and
/// caches the caller for the extractor.
pub async fn require_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let ip = state
        .authsvc
        .client_ip(req.headers(), peer_ip(req.extensions()));
    if let Err(wait) = state.authsvc.bearer_admission(ip) {
        return ApiError::too_many_requests(wait).into_response();
    }
    match authenticate(&state, req.headers()) {
        Ok(a) => {
            if let Err(wait) = state.authsvc.api_limiter.check(&a.principal) {
                return ApiError::too_many_requests(wait).into_response();
            }
            state.authsvc.note_use(&a.principal);
            req.extensions_mut().insert(a);
            next.run(req).await
        }
        Err(e) => {
            state.authsvc.bearer_failed(ip);
            e.into_response()
        }
    }
}
