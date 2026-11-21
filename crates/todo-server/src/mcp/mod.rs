//! The MCP server at `/mcp`: the agent tools over the streamable HTTP transport (`rmcp`).
//!
//! # Transport
//!
//! MCP is served **statelessly** (`rmcp`'s `legacy_session_mode: false`, JSON responses): every POST
//! carries one JSON-RPC message and gets its answer in the response body. No `Mcp-Session-Id` is issued
//! and nothing is kept between requests, so there are no idle sessions to expire and no per-session
//! memory to bound. GET (a server-to-client stream) and DELETE (ending a session) are answered with 405.
//! The tools never send notifications or requests to the client, so nothing needs a stream.
//!
//! # The same gates as `/api/v1`
//!
//! The route sits inside the app-wide middleware (access log, security headers, body limit, request
//! deadline, see [`crate::app`]) and behind, outermost first:
//!
//! 1. [`auth::require_auth`]: the bearer token (an agent token or an owner device token), the per-token
//!    request rate limit, 401 with `WWW-Authenticate: Bearer` otherwise;
//! 2. [`Inflight`]: at most `limits.max_mcp_requests_per_token` requests per token in flight (the
//!    stateless equivalent of a per-token session cap), 429 `too_many_mcp_requests` beyond that.
//!
//! Every tool then goes through the REST handlers themselves ([`tools`]), so admission permits, the
//! write quota, the engine's access checks and the read visibility filters are exactly the REST ones.
//!
//! `rmcp`'s DNS-rebinding guard (an allow-list of `Host` values, loopback only by default) is turned
//! off: the server sits behind a reverse proxy under its public hostname, which is not compiled in, and
//! every request needs a bearer token, which a rebinding page cannot supply. Requests that carry an
//! `Origin` header (that is, from a browser) are refused outright with 403: no browser client speaks
//! MCP to this server.

pub mod tools;

use crate::api::error::ApiError;
use crate::app::AppState;
use crate::auth::{self, Authenticated, Principal};
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

/// Where the MCP endpoint is mounted.
pub const PATH: &str = "/mcp";

/// The server's default log filter. `rmcp` logs rejected JSON-RPC requests and error responses at
/// `warn`, and their messages can quote tool arguments (task content), so it is held to `error`.
pub const DEFAULT_LOG_FILTER: &str = "info,rmcp=error";

/// Counts each token's MCP requests in flight and refuses those beyond the cap.
///
/// Entries exist only while a request is running, so the map holds at most one entry per principal
/// with a request in flight.
#[derive(Debug)]
pub struct Inflight {
    max: usize,
    counts: Mutex<HashMap<Principal, usize>>,
}

/// One admitted request; releases its slot when dropped.
#[derive(Debug)]
pub struct Slot {
    owner: Arc<Inflight>,
    who: Principal,
}

impl Inflight {
    /// At most `max` (at least 1) requests per principal.
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            counts: Mutex::new(HashMap::new()),
        }
    }

    /// Takes a slot for `who`, or `None` when it already has `max` requests in flight.
    pub fn enter(self: &Arc<Self>, who: &Principal) -> Option<Slot> {
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        let n = counts.entry(who.clone()).or_insert(0);
        if *n >= self.max {
            return None;
        }
        *n += 1;
        Some(Slot {
            owner: self.clone(),
            who: who.clone(),
        })
    }

    /// Requests `who` has in flight.
    #[must_use]
    pub fn in_flight(&self, who: &Principal) -> usize {
        self.counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(who)
            .copied()
            .unwrap_or(0)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut counts = self
            .owner
            .counts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = counts.get_mut(&self.who) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                counts.remove(&self.who);
            }
        }
    }
}

/// 429 when a token has too many MCP requests in flight.
#[must_use]
pub fn too_many_requests(max: usize) -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_mcp_requests",
        format!("this token already has {max} MCP requests in flight; retry when one finishes"),
    )
    .with_retry_after(1)
}

/// Middleware (after [`auth::require_auth`]) that holds one [`Slot`] for the whole request.
async fn admit(State(inflight): State<Arc<Inflight>>, req: Request, next: Next) -> Response {
    let Some(who) = req
        .extensions()
        .get::<Authenticated>()
        .map(|a| a.principal.clone())
    else {
        // `require_auth` always runs first; without a caller there is nothing to admit.
        return ApiError::unauthorized().into_response();
    };
    let Some(slot) = inflight.enter(&who) else {
        return too_many_requests(inflight.max).into_response();
    };
    let res = next.run(req).await;
    drop(slot);
    res
}

/// The `rmcp` transport configuration (see the module docs).
#[must_use]
pub fn transport_config(state: &AppState) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_sse_retry(None)
        .with_max_request_body_bytes(state.limits.max_body_bytes)
        .disable_allowed_hosts()
        .enforce_origin_validation()
}

/// The `/mcp` route, to be merged into the app router (inside its global middleware).
pub fn router(state: AppState) -> Router<AppState> {
    let inflight = Arc::new(Inflight::new(state.limits.max_mcp_requests_per_token));
    let handler = tools::TodoMcp::new(state.clone());
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(NeverSessionManager::default()),
        transport_config(&state),
    );
    Router::new()
        .route_service(PATH, service)
        .route_layer(from_fn_with_state(inflight, admit))
        .route_layer(from_fn_with_state(state, auth::require_auth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::DeviceId;

    #[test]
    fn slots_are_capped_per_principal_and_released_on_drop() {
        let inflight = Arc::new(Inflight::new(2));
        let a = Principal::Owner {
            device: DeviceId::new("a").unwrap(),
        };
        let b = Principal::Owner {
            device: DeviceId::new("b").unwrap(),
        };
        let s1 = inflight.enter(&a).unwrap();
        let s2 = inflight.enter(&a).unwrap();
        assert!(inflight.enter(&a).is_none());
        // Another token is not affected.
        let s3 = inflight.enter(&b).unwrap();
        drop(s1);
        assert_eq!(inflight.in_flight(&a), 1);
        let s4 = inflight.enter(&a).unwrap();
        drop((s2, s3, s4));
        assert!(inflight.counts.lock().unwrap().is_empty(), "entries leak");
    }
}
