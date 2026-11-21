//! The server's own web pages: sign-in and setup, device pairing approval, the admin sections, share
//! pages, and their static assets. Everything is rendered on the server, with no framework and no
//! external resources.
//!
//! # Routes reachable without authentication
//!
//! The [`Gate::Public`] entries of [`routes`] (see [`crate::app::public_routes`] for the full list,
//! with `GET /health` and the two pairing API endpoints). `/setup` and its ceremony endpoints answer
//! 404 once the owner has a passkey. Every other page needs a session ([`OwnerSession`]); every other
//! API route needs a bearer token.
//!
//! # Protections
//!
//! - Non-GET requests must carry `Origin: <public_origin>` ([`origin_guard`]), and those made with a
//!   session must carry its CSRF token.
//! - The sign-in, setup and pairing routes are rate-limited per client IP ([`ip_limit`]); failed
//!   sign-ins lock an IP out for a while.
//! - Every HTML response gets a strict CSP, `X-Content-Type-Options: nosniff`, `Referrer-Policy:
//!   no-referrer` and `Cache-Control: no-store` ([`security_headers`], applied to the whole app).
//!   When `public_origin` is `https://`, every response (pages, API JSON, event streams, assets) also
//!   gets [`HSTS`].
//!
//! [`OwnerSession`]: crate::auth::session::OwnerSession

pub mod ceremony;
pub mod html;
pub mod pages;
pub mod share;

use crate::app::AppState;
use crate::auth::session::check_origin;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY,
    STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};

/// `Strict-Transport-Security` on every response when the public origin is HTTPS: one year. No
/// `includeSubDomains`: the server does not know what else runs under its domain.
pub const HSTS: &str = "max-age=31536000";

/// The content security policy of every HTML page (share pages use a stricter one).
pub const CSP: &str = "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";

const AUTH_JS: &str = include_str!("assets/auth.js");
const APP_CSS: &str = include_str!("assets/app.css");

/// `GET /assets/{file}`.
async fn asset(Path(file): Path<String>) -> Response {
    let (body, ty) = match file.as_str() {
        "auth.js" => (AUTH_JS, "text/javascript; charset=utf-8"),
        "app.css" => (APP_CSS, "text/css; charset=utf-8"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (CONTENT_TYPE, HeaderValue::from_static(ty)),
            (
                CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        body,
    )
        .into_response()
}

/// A same-site path to continue to after sign-in: must start with one `/` (no scheme, no `//host`,
/// no backslashes or control characters); anything else becomes `/admin`.
#[must_use]
pub fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n)
            if n.starts_with('/')
                && !n.starts_with("//")
                && !n.contains('\\')
                && !n.chars().any(char::is_control)
                && n.len() <= 512 =>
        {
            n.to_owned()
        }
        _ => "/admin".to_owned(),
    }
}

/// Rejects state-changing requests whose `Origin` is not the public origin.
pub async fn origin_guard(State(app): State<AppState>, req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD)
        && let Err(e) = check_origin(req.headers(), &app.authsvc)
    {
        return e.into_response();
    }
    next.run(req).await
}

/// Per-IP rate limit for the sign-in, setup and pairing routes.
pub async fn ip_limit(State(app): State<AppState>, req: Request, next: Next) -> Response {
    let ip = app
        .authsvc
        .client_ip(req.headers(), crate::auth::peer_ip(req.extensions()));
    if let Some(ip) = ip
        && let Err(wait) = app.authsvc.ip_limiter.check(&ip)
    {
        return crate::api::error::ApiError::too_many_requests(wait).into_response();
    }
    next.run(req).await
}

/// Security headers on every response; the strict CSP on every HTML response that has none yet, and
/// [`HSTS`] on every response when `https` (the public origin is HTTPS).
pub async fn security_headers(State(https): State<bool>, req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if !h.contains_key(CACHE_CONTROL) {
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    let is_html = h
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if is_html {
        if !h.contains_key(CONTENT_SECURITY_POLICY) {
            h.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
        }
        h.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    if https {
        h.insert(STRICT_TRANSPORT_SECURITY, HeaderValue::from_static(HSTS));
    }
    res
}

/// Who may reach a web route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Anyone (sign-in, setup, the pairing page, assets, share pages).
    Public,
    /// A signed-in owner session; the handler takes [`OwnerSession`](crate::auth::session::OwnerSession).
    Session,
}

struct Route {
    method: Method,
    path: &'static str,
    gate: Gate,
    /// Rate-limited per client IP.
    limited: bool,
    handler: MethodRouter<AppState>,
}

fn table() -> Vec<Route> {
    use Gate::{Public, Session};
    let r = |method: Method, path, gate, limited, handler| Route {
        method,
        path,
        gate,
        limited,
        handler,
    };
    vec![
        // Sign-in, setup and pairing (rate-limited per IP).
        r(Method::GET, "/login", Public, true, get(pages::login)),
        r(Method::GET, "/setup", Public, true, get(pages::setup)),
        r(Method::GET, "/pair", Public, true, get(pages::pair)),
        r(
            Method::POST,
            "/auth/login/start",
            Public,
            true,
            post(ceremony::login_start),
        ),
        r(
            Method::POST,
            "/auth/login/finish",
            Public,
            true,
            post(ceremony::login_finish),
        ),
        r(
            Method::POST,
            "/auth/setup/start",
            Public,
            true,
            post(ceremony::setup_start),
        ),
        r(
            Method::POST,
            "/auth/setup/finish",
            Public,
            true,
            post(ceremony::setup_finish),
        ),
        r(
            Method::POST,
            "/auth/step-up/start",
            Session,
            true,
            post(ceremony::step_up_start),
        ),
        r(
            Method::POST,
            "/auth/step-up/finish",
            Session,
            true,
            post(ceremony::step_up_finish),
        ),
        r(
            Method::POST,
            "/auth/passkeys/start",
            Session,
            true,
            post(ceremony::passkey_start),
        ),
        r(
            Method::POST,
            "/auth/passkeys/finish",
            Session,
            true,
            post(ceremony::passkey_finish),
        ),
        r(
            Method::POST,
            "/pair/approve",
            Session,
            true,
            post(pages::pair_approve),
        ),
        r(
            Method::POST,
            "/pair/deny",
            Session,
            true,
            post(pages::pair_deny),
        ),
        // Admin.
        r(Method::GET, "/", Session, false, get(pages::root)),
        r(
            Method::POST,
            "/auth/logout",
            Session,
            false,
            post(pages::logout),
        ),
        r(Method::GET, "/admin", Session, false, get(pages::overview)),
        r(
            Method::GET,
            "/admin/devices",
            Session,
            false,
            get(pages::devices),
        ),
        r(
            Method::POST,
            "/admin/devices/{id}/revoke",
            Session,
            false,
            post(pages::device_revoke),
        ),
        r(
            Method::GET,
            "/admin/agents",
            Session,
            false,
            get(pages::agents),
        ),
        r(
            Method::POST,
            "/admin/agents",
            Session,
            false,
            post(pages::agent_create),
        ),
        r(
            Method::GET,
            "/admin/agents/{id}",
            Session,
            false,
            get(pages::agent_grants),
        ),
        r(
            Method::POST,
            "/admin/agents/{id}/grants",
            Session,
            false,
            post(pages::agent_grants_save),
        ),
        r(
            Method::POST,
            "/admin/agents/{id}/revoke",
            Session,
            false,
            post(pages::agent_revoke),
        ),
        r(
            Method::GET,
            "/admin/shares",
            Session,
            false,
            get(pages::shares),
        ),
        r(
            Method::POST,
            "/admin/shares",
            Session,
            false,
            post(pages::share_create),
        ),
        r(
            Method::POST,
            "/admin/shares/{id}/rotate",
            Session,
            false,
            post(pages::share_rotate),
        ),
        r(
            Method::POST,
            "/admin/shares/{id}/revoke",
            Session,
            false,
            post(pages::share_revoke),
        ),
        r(
            Method::GET,
            "/admin/audit",
            Session,
            false,
            get(pages::audit),
        ),
        // Static assets and share pages.
        r(Method::GET, "/assets/{file}", Public, false, get(asset)),
        r(Method::GET, "/s/{token}", Public, false, get(share::show)),
    ]
}

/// Every web route: `(method, path template, gate)`. The router is built from the same table.
#[must_use]
pub fn routes() -> Vec<(Method, &'static str, Gate)> {
    table()
        .into_iter()
        .map(|r| (r.method, r.path, r.gate))
        .collect()
}

/// The web router (pages, ceremonies, assets, share pages).
///
/// Every non-GET route checks `Origin`; the sign-in, setup and pairing routes are rate-limited per
/// client IP. Session routes check the session in their handlers (the [`OwnerSession`] extractor).
///
/// [`OwnerSession`]: crate::auth::session::OwnerSession
pub fn router(state: AppState) -> Router<AppState> {
    let mut by_path: Vec<(&'static str, bool, MethodRouter<AppState>)> = Vec::new();
    for r in table() {
        match by_path
            .iter_mut()
            .find(|(p, l, _)| *p == r.path && *l == r.limited)
        {
            Some((_, _, m)) => *m = std::mem::take(m).merge(r.handler),
            None => by_path.push((r.path, r.limited, r.handler)),
        }
    }
    let (mut limited, mut open) = (Router::new(), Router::new());
    for (path, is_limited, m) in by_path {
        if is_limited {
            limited = limited.route(path, m);
        } else {
            open = open.route(path, m);
        }
    }
    open.merge(limited.route_layer(from_fn_with_state(state.clone(), ip_limit)))
        .route_layer(from_fn_with_state(state, origin_guard))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_must_stay_on_this_site() {
        assert_eq!(safe_next(Some("/pair?code=AB")), "/pair?code=AB");
        for bad in [
            "//evil.example.com",
            "https://evil.example.com",
            "/\\evil",
            "x",
            "/a\nb",
        ] {
            assert_eq!(safe_next(Some(bad)), "/admin", "{bad}");
        }
        assert_eq!(safe_next(None), "/admin");
    }
}
