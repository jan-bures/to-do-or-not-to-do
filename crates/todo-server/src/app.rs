//! Application wiring: shared state, the router with its middleware, and the server lifecycle.
//!
//! Middleware, outermost first: access log, security headers, sensitive-header marking, JSON 413s,
//! body limits, the request deadline; then per route: the route label, authentication (bearer tokens
//! for `/api/v1` and `/mcp`, sessions for the admin pages), the handler.
//!
//! # Routes reachable without authentication
//!
//! Exactly [`public_routes`]; the integration tests check that list and that every other route
//! rejects anonymous requests.

use crate::api;
use crate::api::RouteLabel;
use crate::api::error::ApiError;
use crate::auth::Authenticator;
use crate::auth::service::{AuthInitError, AuthService, SetupCodeSink};
use crate::cancel::Cancel;
use crate::config::{Config, ConfigError, Limits};
use crate::db::StoreError;
use crate::engine::{Clock, Engine, system_clock};
use crate::ratelimit::{CatchUpMarks, ConnectLimiter};
use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, PROXY_AUTHORIZATION};
use axum::middleware::{Next, from_fn, from_fn_with_state, map_response};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use jiff::tz::TimeZone;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use todo_core::query::EvalContext;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;

/// State shared by all handlers.
#[derive(Clone, Debug)]
pub struct AppState {
    /// The single writer and the in-memory state.
    pub engine: Arc<Engine>,
    /// Bearer authentication for the API.
    pub auth: Arc<dyn Authenticator>,
    /// Passkeys, sessions, tokens, pairing, share links, rate limits and the audit log.
    pub authsvc: Arc<AuthService>,
    /// The owner's time zone.
    pub zone: TimeZone,
    /// Limits.
    pub limits: Limits,
    /// Permits for concurrent SSE connections.
    pub sse_slots: Arc<Semaphore>,
    /// Admission permits for blocking reads (see [`api::blocking`]).
    pub reads: Arc<Semaphore>,
    /// Admission permits for blocking writes.
    pub writes: Arc<Semaphore>,
    /// Per-principal limit on event-stream (re)connects.
    pub sse_connects: Arc<ConnectLimiter>,
    /// Where each principal's last stream told it to catch up.
    pub catch_up_marks: Arc<CatchUpMarks>,
    /// Becomes `true` when the server shuts down (ends event streams).
    pub shutdown: watch::Receiver<bool>,
}

impl AppState {
    /// "Now" and the owner's "today" for smart-list evaluation.
    #[must_use]
    pub fn eval_context(&self) -> EvalContext {
        let now = self.engine.now();
        let zoned = jiff::ZonedDateTime::new(now, self.zone.clone());
        EvalContext::new(now, zoned.date(), zoned.offset().seconds())
    }
}

/// Server start-up errors.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Bad configuration.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Storage failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Authentication could not be set up.
    #[error(transparent)]
    Auth(#[from] AuthInitError),
    /// I/O failed (binding, creating the data directory).
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: String,
        /// The cause.
        source: std::io::Error,
    },
}

async fn health() -> StatusCode {
    StatusCode::OK
}

/// Gives 413 responses from the body-limit layer the same JSON shape as every other error.
async fn json_413(res: Response) -> Response {
    let is_json = res
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if res.status() == StatusCode::PAYLOAD_TOO_LARGE && !is_json {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "request body is too large",
        )
        .into_response()
    } else {
        res
    }
}

/// The request deadline. It covers everything up to the response head, including authentication and
/// reading the request body, on every route. An event stream returns its head at once and is then
/// bounded by its own heartbeat, stall deadline and shutdown handling, so it needs no exemption (and the
/// `Accept` header can never be used to escape the deadline).
///
/// The middleware puts a [`Cancel`] with the absolute deadline into the request extensions (handlers
/// take it with [`api::RequestCancel`]). When the deadline passes it tries to cancel the work:
///
/// - **Cancelled** (the work had not begun committing): 408 `timeout`; nothing was or will be stored.
/// - **Already committing**: the outcome is not the middleware's to discard. It waits up to one more
///   `request_timeout` for the response; if even that passes, 503 `outcome_unknown` tells the client to
///   retry with the same `command_id` (which returns the stored result if the commit landed).
///
/// If the request future is dropped (the client went away), the guard cancels the work too.
async fn deadline(State(limit): State<Duration>, mut req: Request, next: Next) -> Response {
    let at = tokio::time::Instant::now() + limit;
    let cancel = Cancel::with_deadline(at.into_std());
    req.extensions_mut().insert(cancel.clone());
    let guard = cancel.on_drop();
    let fut = next.run(req);
    tokio::pin!(fut);
    let res = match tokio::time::timeout_at(at, &mut fut).await {
        Ok(res) => res,
        Err(_) if cancel.cancel() => ApiError::timeout().into_response(),
        Err(_) => match tokio::time::timeout(limit, &mut fut).await {
            Ok(res) => res,
            Err(_) => {
                tracing::warn!("a commit outlived its grace period; reporting outcome_unknown");
                ApiError::outcome_unknown().into_response()
            }
        },
    };
    guard.disarm();
    res
}

/// One line per request: method, matched route template (never the raw path or the query string),
/// status and duration. Requests that were rejected by authentication or matched no route can come
/// from anyone on the internet, so they are logged at `debug` only.
async fn access_log(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let started = Instant::now();
    let res = next.run(req).await;
    let status = res.status().as_u16();
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match res.extensions().get::<RouteLabel>() {
        Some(RouteLabel(route)) if status != StatusCode::UNAUTHORIZED.as_u16() => {
            if res.status().is_server_error() {
                tracing::warn!(%method, route = %route, status, ms, "request failed");
            } else {
                tracing::info!(%method, route = %route, status, ms, "request");
            }
        }
        label => tracing::debug!(
            %method,
            route = %label.map_or("unmatched", |l| l.0.as_str()),
            status,
            ms,
            "rejected request"
        ),
    }
    res
}

/// Every route reachable without credentials: `(method, path template)`. Everything else needs a
/// bearer token (`/api/v1`) or an owner session (the web pages).
#[must_use]
pub fn public_routes() -> Vec<(String, String)> {
    let mut v = vec![
        ("GET".to_owned(), "/health".to_owned()),
        ("POST".to_owned(), "/api/v1/pair/start".to_owned()),
        ("POST".to_owned(), "/api/v1/pair/poll".to_owned()),
    ];
    v.extend(
        crate::web::routes()
            .into_iter()
            .filter(|(_, _, g)| *g == crate::web::Gate::Public)
            .map(|(m, p, _)| (m.to_string(), p.to_owned())),
    );
    v
}

/// Builds the router with all middleware.
pub fn router(state: AppState) -> Router {
    let limits = state.limits.clone();
    let https = state
        .authsvc
        .origin()
        .is_some_and(crate::config::PublicOrigin::is_https);
    Router::new()
        .route("/health", get(health))
        .merge(crate::web::router(state.clone()))
        .merge(crate::mcp::router(state.clone()))
        .route_layer(from_fn(api::tag_route))
        .nest("/api/v1", api::router(state.clone()))
        .layer(from_fn_with_state(limits.request_timeout(), deadline))
        .layer(DefaultBodyLimit::max(limits.max_body_bytes))
        .layer(RequestBodyLimitLayer::new(limits.max_body_bytes))
        .layer(map_response(json_413))
        .layer(SetSensitiveRequestHeadersLayer::new([
            AUTHORIZATION,
            PROXY_AUTHORIZATION,
            COOKIE,
        ]))
        .layer(from_fn_with_state(https, crate::web::security_headers))
        .layer(from_fn(access_log))
        .with_state(state)
}

/// Extra knobs for embedding the server (tests).
#[derive(Clone)]
pub struct Options {
    /// The clock for event times, "today", and every authentication expiry.
    pub clock: Clock,
    /// API authentication; defaults to the auth service (paired devices, agent tokens, and the
    /// config owner tokens when allowed).
    pub auth: Option<Arc<dyn Authenticator>>,
    /// Also receives the one-time setup code (which is otherwise only in the owner-only setup code
    /// file, never in the log).
    pub setup_code: Option<SetupCodeSink>,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            clock: system_clock(),
            auth: None,
            setup_code: None,
        }
    }
}

/// A bound, not yet running server.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    router: Router,
    engine: Arc<Engine>,
    authsvc: Arc<AuthService>,
    limits: Limits,
    shutdown: watch::Sender<bool>,
}

impl Server {
    /// Opens the database and binds the listener.
    pub async fn bind(config: &Config, options: Options) -> Result<Self, ServerError> {
        config.validate()?;
        let zone = config.zone()?;
        std::fs::create_dir_all(&config.data_dir).map_err(|source| ServerError::Io {
            context: format!("cannot create data dir {}", config.data_dir.display()),
            source,
        })?;
        let path = config.database_path();
        let clock = options.clock.clone();
        let engine = tokio::task::spawn_blocking(move || Engine::open(&path, clock))
            .await
            .map_err(|e| ServerError::Io {
                context: "opening the database".into(),
                source: std::io::Error::other(e.to_string()),
            })??;
        let engine = Arc::new(engine);
        let authsvc = {
            let (config, engine, sink) = (config.clone(), engine.clone(), options.setup_code);
            tokio::task::spawn_blocking(move || AuthService::new(&config, engine, sink))
                .await
                .map_err(|e| ServerError::Io {
                    context: "starting authentication".into(),
                    source: std::io::Error::other(e.to_string()),
                })??
        };
        let authsvc = Arc::new(authsvc);
        let auth: Arc<dyn Authenticator> = match options.auth {
            Some(a) => a,
            None => authsvc.clone(),
        };
        let listener = TcpListener::bind(config.bind)
            .await
            .map_err(|source| ServerError::Io {
                context: format!("cannot bind {}", config.bind),
                source,
            })?;
        let (shutdown, shutdown_rx) = watch::channel(false);
        let state = AppState {
            engine: engine.clone(),
            auth,
            authsvc: authsvc.clone(),
            zone,
            limits: config.limits.clone(),
            sse_slots: Arc::new(Semaphore::new(config.limits.max_sse_connections)),
            reads: Arc::new(Semaphore::new(config.limits.max_concurrent_reads)),
            writes: Arc::new(Semaphore::new(config.limits.max_concurrent_writes)),
            sse_connects: Arc::new(ConnectLimiter::new(
                config.limits.sse_connects_per_minute,
                config.limits.sse_connect_burst,
            )),
            catch_up_marks: Arc::new(CatchUpMarks::default()),
            shutdown: shutdown_rx,
        };
        Ok(Self {
            listener,
            router: router(state),
            engine,
            authsvc,
            limits: config.limits.clone(),
            shutdown,
        })
    }

    /// The bound address (useful with port 0).
    pub fn local_addr(&self) -> Result<SocketAddr, ServerError> {
        self.listener
            .local_addr()
            .map_err(|source| ServerError::Io {
                context: "local address".into(),
                source,
            })
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> Arc<Engine> {
        self.engine.clone()
    }

    /// The authentication service (tokens, grants, revocation).
    #[must_use]
    pub fn auth_service(&self) -> Arc<AuthService> {
        self.authsvc.clone()
    }

    /// Serves until `signal` completes, then ends event streams and drains connections (see
    /// [`crate::conn`]), and finally closes the engine.
    pub async fn run_until(
        self,
        signal: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServerError> {
        let addr = self.listener.local_addr().ok();
        tracing::info!(addr = ?addr, "listening");
        crate::conn::serve(
            self.listener,
            self.router,
            self.limits,
            self.shutdown,
            signal,
        )
        .await;
        // Refuse late writes (from timed-out requests still on the blocking pool) and release the
        // ownership lock so the next server can open the database.
        let engine = self.engine;
        let _ = tokio::task::spawn_blocking(move || engine.close()).await;
        Ok(())
    }

    /// Runs the server on a background task. Used by tests and embedders.
    pub fn spawn(self) -> Result<RunningServer, ServerError> {
        let addr = self.local_addr()?;
        let engine = self.engine();
        let authsvc = self.auth_service();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(self.run_until(async move {
            let _ = stopped.await;
        }));
        Ok(RunningServer {
            addr,
            engine,
            authsvc,
            stop,
            task,
        })
    }
}

/// A server running on a background task.
#[derive(Debug)]
pub struct RunningServer {
    /// The bound address.
    pub addr: SocketAddr,
    /// The engine.
    pub engine: Arc<Engine>,
    /// The authentication service (tokens, grants, revocation).
    pub authsvc: Arc<AuthService>,
    stop: tokio::sync::oneshot::Sender<()>,
    task: JoinHandle<Result<(), ServerError>>,
}

impl RunningServer {
    /// Base URL, for example `http://127.0.0.1:12345`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Stops the server and waits for it to finish.
    pub async fn shutdown(self) -> Result<(), ServerError> {
        let _ = self.stop.send(());
        match self.task.await {
            Ok(r) => r,
            Err(e) => Err(ServerError::Io {
                context: "server task".into(),
                source: std::io::Error::other(e.to_string()),
            }),
        }
    }
}
