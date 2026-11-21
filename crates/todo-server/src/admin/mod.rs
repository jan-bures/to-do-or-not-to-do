//! The local admin API: routine admin operations for an operator (a script or an AI agent on the
//! server's own machine) without the owner's passkey. See `docs/admin-api.md`.
//!
//! # Exposure
//!
//! It is served by a second listener, on its own socket, only when `[admin] bind` is set, and the
//! config refuses any address that isn't loopback. The public listener doesn't mount these routes at
//! all and answers 404 anywhere under `/api/v1/admin` ([`not_on_the_public_listener`]). A tunnel or
//! proxy must never be pointed at the admin port; reach it from elsewhere with an SSH port forward.
//!
//! # Credential
//!
//! One bearer token, `tdx_...`, made by `todo-server admin-token --out <file>`. The config holds only
//! its SHA-256 (`admin.token_sha256`); rotating it means a new token and a restart. Device, agent and
//! owner tokens are refused here, and the admin token is refused everywhere else (the public
//! listener doesn't know it).
//!
//! # Guard ([`guard`]), in order
//!
//! 1. **No browsers.** A request carrying `Origin`, `Sec-Fetch-Site`, `Sec-Fetch-Mode` or `Cookie` is
//!    refused with 403 `browser_refused`, so no web page can drive the API, even through DNS
//!    rebinding. The `Host` must name a loopback address (`localhost`, `127.0.0.1`, `[::1]`).
//! 2. **The token**, only from `Authorization: Bearer`, compared as SHA-256 digests in constant time.
//!    It is never logged.
//! 3. **Failures.** A missing or wrong token spends a small budget shared by all callers
//!    (`admin.failures_per_minute`, `admin.failure_burst`); once it is empty, rejected requests get
//!    429 instead of 401. The right token is never charged and never turned away by it.
//! 4. **Rate.** All authenticated requests share `admin.requests_per_minute` and `admin.request_burst`.
//!
//! Forwarding headers (`CF-Connecting-IP`, `X-Forwarded-For`) are ignored: the client IP is always
//! the TCP peer.
//!
//! # Scope
//!
//! Email ingest settings, agents (listing, grants and quota, revocation), devices and share links
//! (listing and revocation), the audit log, and the names of lists and collections. Never task data,
//! never a new credential of any kind (agent tokens, device pairing, share links), never passkeys:
//! those stay behind the owner's passkey on the admin pages. Every change is audited with the actor
//! [`ADMIN_API_ACTOR`](crate::auth::service::ADMIN_API_ACTOR).

pub mod handlers;
pub mod openapi;

use crate::api::error::ApiError;
use crate::api::tag_route;
use crate::app::AppState;
use crate::auth::limits::KeyedLimiter;
use crate::auth::secret::{self, ADMIN_TOKEN_PREFIX, RandomError};
use crate::auth::{bearer, peer_ip};
use crate::config::AdminConfig;
use axum::Router;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Request, State};
use axum::http::header::{COOKIE, HOST, ORIGIN};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{Next, from_fn, from_fn_with_state, map_response};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;

/// Every admin route starts with this.
pub const PREFIX: &str = "/api/v1/admin";

/// State of the admin listener: the app, the token's hash and its limiters.
#[derive(Clone)]
pub struct AdminState {
    /// The shared application state.
    pub app: AppState,
    inner: Arc<Inner>,
}

struct Inner {
    token_hash: [u8; 32],
    requests: KeyedLimiter<()>,
    failures: KeyedLimiter<()>,
}

impl std::fmt::Debug for AdminState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdminState(<redacted>)")
    }
}

impl AdminState {
    /// The state for a token hash and the configured rates.
    #[must_use]
    pub fn new(app: AppState, token_hash: [u8; 32], cfg: &AdminConfig) -> Self {
        Self {
            app,
            inner: Arc::new(Inner {
                token_hash,
                requests: KeyedLimiter::new(cfg.requests_per_minute, cfg.request_burst),
                failures: KeyedLimiter::new(cfg.failures_per_minute, cfg.failure_burst),
            }),
        }
    }

    /// True if `token` is the admin token (constant time over the digests).
    fn accepts(&self, token: &str) -> bool {
        // The prefix check only rejects tokens of other kinds early; it says nothing about the secret.
        token.starts_with(ADMIN_TOKEN_PREFIX)
            && secret::ct_eq(&secret::digest(token), &self.inner.token_hash)
    }
}

/// The caller of an admin route, set by [`guard`]: the TCP peer's address.
#[derive(Clone, Copy, Debug)]
pub struct AdminCaller {
    /// The client IP (the TCP peer; forwarding headers are ignored).
    pub ip: Option<IpAddr>,
}

impl<S: Send + Sync> FromRequestParts<S> for AdminCaller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Only the guard inserts it; a handler mounted without the guard refuses everything.
        parts
            .extensions
            .get::<Self>()
            .copied()
            .ok_or_else(ApiError::unauthorized)
    }
}

/// 403 for a request that looks like it comes from a browser.
fn browser_refused() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "browser_refused",
        "the admin API does not accept requests from browsers (Origin, Sec-Fetch-* or Cookie headers)",
    )
}

/// 403 for a `Host` that isn't a loopback name.
fn bad_host() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "bad_host",
        "the admin API answers only to localhost, 127.0.0.1 or [::1]",
    )
}

/// True if the request carries a header only browsers send (or a cookie).
fn from_browser(h: &HeaderMap) -> bool {
    h.contains_key(ORIGIN)
        || h.contains_key("sec-fetch-site")
        || h.contains_key("sec-fetch-mode")
        || h.contains_key(COOKIE)
}

/// True if `Host` names a loopback address (`localhost`, or a loopback IP literal), with any port.
#[must_use]
pub fn loopback_host(host: &str) -> bool {
    let host = host.trim();
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((ip, tail)) if tail.is_empty() || tail.starts_with(':') => ip,
            _ => return false,
        }
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
            Some(_) => return false,
            None => host,
        }
    };
    name.eq_ignore_ascii_case("localhost")
        || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The admin listener's gate (see the module docs).
pub async fn guard(State(st): State<AdminState>, mut req: Request, next: Next) -> Response {
    let headers = req.headers();
    if from_browser(headers) {
        return browser_refused().into_response();
    }
    let host_ok = headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            req.uri()
                .authority()
                .map(axum::http::uri::Authority::as_str)
        })
        .is_some_and(loopback_host);
    if !host_ok {
        return bad_host().into_response();
    }
    // The token is checked first, so wrong tokens from another local process can't lock out the
    // right one: only rejected requests spend the failure budget.
    let ok = bearer(headers).is_ok_and(|t| st.accepts(t));
    if !ok {
        tracing::debug!("rejected an admin API request");
        if let Err(wait) = st.inner.failures.check(&()) {
            return ApiError::too_many_requests(wait).into_response();
        }
        return ApiError::unauthorized().into_response();
    }
    if let Err(wait) = st.inner.requests.check(&()) {
        return ApiError::too_many_requests(wait).into_response();
    }
    let ip = peer_ip(req.extensions());
    req.extensions_mut().insert(AdminCaller { ip });
    next.run(req).await
}

async fn no_such_endpoint() -> ApiError {
    crate::api::no_such_endpoint()
}

/// The admin listener's router, with the same body limit, deadline, security headers and access log
/// as the public one.
pub fn router(st: AdminState) -> Router {
    use handlers as h;
    let limits = st.app.limits.clone();
    Router::new()
        .route("/api/v1/admin/openapi.json", get(openapi::document))
        .route("/api/v1/admin/lists", get(h::lists))
        .route("/api/v1/admin/ingest", get(h::ingest))
        .route("/api/v1/admin/ingest/inbox", put(h::inbox_set))
        .route(
            "/api/v1/admin/ingest/senders",
            get(h::senders).post(h::sender_add),
        )
        .route(
            "/api/v1/admin/ingest/senders/{address}",
            delete(h::sender_remove),
        )
        .route("/api/v1/admin/ingest/keys", get(h::keys).post(h::key_add))
        .route("/api/v1/admin/ingest/keys/{key}", delete(h::key_remove))
        .route("/api/v1/admin/agents", get(h::agents))
        .route("/api/v1/admin/agents/{id}", get(h::agent))
        .route("/api/v1/admin/agents/{id}/access", put(h::agent_access))
        .route("/api/v1/admin/agents/{id}/revoke", post(h::agent_revoke))
        .route("/api/v1/admin/devices", get(h::devices))
        .route("/api/v1/admin/devices/{id}/revoke", post(h::device_revoke))
        .route("/api/v1/admin/shares", get(h::shares))
        .route("/api/v1/admin/shares/{id}/revoke", post(h::share_revoke))
        .route("/api/v1/admin/audit", get(h::audit))
        .fallback(no_such_endpoint)
        .route_layer(from_fn(tag_route))
        .layer(from_fn_with_state(st.clone(), guard))
        .layer(from_fn_with_state(
            limits.request_timeout(),
            crate::app::deadline,
        ))
        .layer(DefaultBodyLimit::max(limits.max_body_bytes))
        .layer(RequestBodyLimitLayer::new(limits.max_body_bytes))
        .layer(map_response(crate::app::json_413))
        .layer(SetSensitiveRequestHeadersLayer::new([
            axum::http::header::AUTHORIZATION,
            axum::http::header::PROXY_AUTHORIZATION,
            COOKIE,
        ]))
        .layer(from_fn_with_state(false, crate::web::security_headers))
        .layer(from_fn(crate::app::access_log))
        .with_state(st)
}

/// Middleware for the public listener: 404 for anything under [`PREFIX`], whatever the method or
/// credentials, so the admin API is absent there (it is never mounted on it either).
pub async fn not_on_the_public_listener(req: Request, next: Next) -> Response {
    let p = req.uri().path();
    if p == PREFIX || p.starts_with("/api/v1/admin/") {
        return crate::api::no_such_endpoint().into_response();
    }
    next.run(req).await
}

// ---- the token ------------------------------------------------------------------------------------

/// A new admin token: `tdx_` and 256 random bits.
pub fn new_token() -> Result<String, RandomError> {
    Ok(format!(
        "{ADMIN_TOKEN_PREFIX}{}",
        secret::random_token::<32>()?
    ))
}

/// Why [`write_token_file`] failed.
#[derive(Debug, thiserror::Error)]
pub enum TokenFileError {
    /// No randomness.
    #[error(transparent)]
    Random(#[from] RandomError),
    /// The file could not be created or written (it must not exist yet).
    #[error("cannot write {}: {source}", path.display())]
    Io {
        /// The file.
        path: std::path::PathBuf,
        /// Why.
        source: std::io::Error,
    },
}

/// Makes a new admin token, writes it to a new owner-only file at `path` (refusing to overwrite an
/// existing one), and returns the hex SHA-256 for `admin.token_sha256`. Touches neither the database
/// nor the config, so it works while the server runs.
pub fn write_token_file(path: &Path) -> Result<String, TokenFileError> {
    use std::io::Write as _;
    let token = new_token()?;
    let io = |source| TokenFileError::Io {
        path: path.to_owned(),
        source,
    };
    let mut f = todo_fs::create_private_file(path).map_err(io)?;
    writeln!(f, "{token}").map_err(io)?;
    f.sync_all().map_err(io)?;
    Ok(crate::auth::hash_token(&token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_are_accepted() {
        for ok in [
            "localhost",
            "LOCALHOST:47321",
            "127.0.0.1",
            "127.0.0.1:47321",
            "127.1.2.3:1",
            "[::1]",
            "[::1]:47321",
        ] {
            assert!(loopback_host(ok), "{ok}");
        }
        for bad in [
            "",
            "todo.example.com",
            "todo.example.com:47321",
            "localhost.example.com",
            "10.0.0.1:47321",
            "[::]:1",
            "[::1]x",
            "127.0.0.1:abc",
            "0.0.0.0",
        ] {
            assert!(!loopback_host(bad), "{bad}");
        }
    }

    #[test]
    fn tokens_are_prefixed_long_and_distinct() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert!(a.starts_with("tdx_"));
        assert_eq!(a.len(), 4 + 43);
        assert_ne!(a, b);
    }
}
