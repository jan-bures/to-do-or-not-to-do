//! Browser sessions for the admin pages.
//!
//! The session cookie is `__Host-todo_session`: `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/`, no
//! `Domain`. Its value is 256 random bits; the store keeps only its SHA-256. A session ends after
//! `auth.session_idle_timeout_mins` without a request or `auth.session_absolute_timeout_hours` after
//! sign-in.
//!
//! **CSRF.** Every state-changing request to the web routes must carry an `Origin` header equal to
//! `public_origin` (checked by the web router for every non-GET request), and requests made with a
//! session must also carry the session's synchronizer token: a `csrf` form field for forms, an
//! `X-CSRF-Token` header for the JSON ceremony endpoints. `SameSite=Strict` is a third layer.

use super::service::AuthService;
use super::store::SessionRow;
use super::{peer_ip, secret};
use crate::api::error::ApiError;
use crate::app::AppState;
use axum::extract::FromRequestParts;
use axum::http::header::{COOKIE, LOCATION, ORIGIN};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use std::net::IpAddr;
use std::sync::Arc;

/// The session cookie's name.
pub const SESSION_COOKIE: &str = "__Host-todo_session";
/// The header carrying the CSRF token on JSON requests.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// `Set-Cookie` value for a new session.
#[must_use]
pub fn session_cookie(value: &str, max_age_secs: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={value}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("invalid=1"))
}

/// `Set-Cookie` value that removes the session cookie.
#[must_use]
pub fn clear_cookie() -> HeaderValue {
    HeaderValue::from_static(
        "__Host-todo_session=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
    )
}

/// The session cookie's value, if the request has one.
#[must_use]
pub fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v.trim().to_owned())
        .filter(|v| !v.is_empty() && v.len() <= 128)
}

/// Runs blocking auth work on the blocking pool.
pub async fn blocking<T: Send + 'static>(
    svc: &Arc<AuthService>,
    f: impl FnOnce(&AuthService) -> T + Send + 'static,
) -> Result<T, ApiError> {
    let svc = svc.clone();
    tokio::task::spawn_blocking(move || f(&svc))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "auth task failed");
            ApiError::internal()
        })
}

/// Checks a presented CSRF token against the session's.
pub fn check_csrf(session: &SessionRow, presented: Option<&str>) -> Result<(), ApiError> {
    match presented {
        Some(p) if secret::ct_eq(p.as_bytes(), session.csrf.as_bytes()) => Ok(()),
        _ => Err(csrf_failed()),
    }
}

/// 403 for a missing or wrong CSRF token or origin.
#[must_use]
pub fn csrf_failed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "csrf",
        "the request did not come from this site's own pages; reload the page and try again",
    )
}

/// Checks that a state-changing request's `Origin` is the public origin.
pub fn check_origin(headers: &HeaderMap, svc: &AuthService) -> Result<(), ApiError> {
    let Some(origin) = svc.origin() else {
        return Err(super::service::no_origin());
    };
    match headers.get(ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(o) if o.eq_ignore_ascii_case(origin.as_str()) => Ok(()),
        _ => Err(csrf_failed()),
    }
}

/// A signed-in owner's browser session. Extracting it from a request without a live session
/// redirects page loads to `/login` and answers anything else with 401.
#[derive(Debug, Clone)]
pub struct OwnerSession {
    /// The session.
    pub row: SessionRow,
    /// The client IP.
    pub ip: Option<IpAddr>,
}

/// The session, if the request has a live one (for pages that work either way).
#[derive(Debug, Clone)]
pub struct MaybeSession(pub Option<OwnerSession>);

/// The client IP of a request.
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub Option<IpAddr>);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            state
                .authsvc
                .client_ip(&parts.headers, peer_ip(&parts.extensions)),
        ))
    }
}

async fn lookup(parts: &Parts, state: &AppState) -> Result<Option<OwnerSession>, ApiError> {
    let ip = state
        .authsvc
        .client_ip(&parts.headers, peer_ip(&parts.extensions));
    let Some(cookie) = cookie_value(&parts.headers) else {
        return Ok(None);
    };
    let row = blocking(&state.authsvc, move |svc| svc.session(&cookie)).await??;
    Ok(row.map(|row| OwnerSession { row, ip }))
}

/// Percent-encodes a path and query for a `next=` parameter.
#[must_use]
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl FromRequestParts<AppState> for MaybeSession {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        lookup(parts, state)
            .await
            .map(Self)
            .map_err(IntoResponse::into_response)
    }
}

impl FromRequestParts<AppState> for OwnerSession {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match lookup(parts, state).await {
            Ok(Some(s)) => Ok(s),
            Ok(None) if parts.method == Method::GET => {
                let next = parts.uri.path_and_query().map_or("/admin", |p| p.as_str());
                let to = format!("/login?next={}", encode_component(next));
                Err((
                    StatusCode::SEE_OTHER,
                    [(
                        LOCATION,
                        HeaderValue::from_str(&to).unwrap_or(HeaderValue::from_static("/login")),
                    )],
                )
                    .into_response())
            }
            Ok(None) => Err(
                ApiError::new(StatusCode::UNAUTHORIZED, "signed_out", "sign in first")
                    .into_response(),
            ),
            Err(e) => Err(e.into_response()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_has_the_host_prefix_and_strict_flags() {
        let c = session_cookie("abc", 3600);
        let c = c.to_str().unwrap();
        assert!(c.starts_with("__Host-todo_session=abc;"));
        for flag in [
            "Path=/",
            "Secure",
            "HttpOnly",
            "SameSite=Strict",
            "Max-Age=3600",
        ] {
            assert!(c.contains(flag), "{flag}");
        }
        assert!(!c.contains("Domain"));
    }

    #[test]
    fn the_cookie_is_found_among_others() {
        let mut h = HeaderMap::new();
        h.insert(
            COOKIE,
            HeaderValue::from_static("a=1; __Host-todo_session=xyz; b=2"),
        );
        assert_eq!(cookie_value(&h).as_deref(), Some("xyz"));
        let mut h = HeaderMap::new();
        h.insert(COOKIE, HeaderValue::from_static("todo_session=xyz"));
        assert_eq!(cookie_value(&h), None);
    }

    #[test]
    fn next_parameters_are_encoded() {
        assert_eq!(encode_component("/pair?code=AB&x"), "/pair%3Fcode%3DAB%26x");
    }
}
