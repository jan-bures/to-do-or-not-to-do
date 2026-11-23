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
//!
//! # The admin listener
//!
//! With `[admin] bind`, a second, loopback-only socket serves [`crate::admin`]'s router and nothing
//! else. The public router never mounts those routes and answers 404 under `/api/v1/admin`.

use crate::api;
use crate::api::RouteLabel;
use crate::api::error::ApiError;
use crate::auth::Authenticator;
use crate::auth::service::{AuthInitError, AuthService, SetupCodeSink};
use crate::cancel::Cancel;
use crate::config::{Config, ConfigError, Limits};
use crate::db::StoreError;
use crate::engine::{Clock, Engine, system_clock};
use crate::ingest::{Ingest, IngestInitError};
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
use todo_core::query::{EvalContext, LocalZone};
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
    /// Email ingest (the secret, nonces, the sender allowlist and list keys).
    pub ingest: Arc<Ingest>,
    /// Attachment content (see [`crate::blobs`]).
    pub attachments: Arc<crate::blobs::Attachments>,
    /// The web app served at `/app`, when `[web] dir` is set.
    pub web: Option<Arc<crate::web::spa::WebApp>>,
    /// The private desktop release feed, when `[releases] dir` is set.
    pub releases: Option<std::path::PathBuf>,
    /// Firebase client options, when Android push wake-ups are configured.
    pub push: Option<Arc<crate::config::PushConfig>>,
}

impl AppState {
    /// "Now" and the owner's "today" for smart-list evaluation.
    #[must_use]
    pub fn eval_context(&self) -> EvalContext {
        let now = self.engine.now();
        EvalContext::with_zone(now, LocalZone::Jiff(self.zone.clone()))
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
    /// Email ingest could not be set up.
    #[error(transparent)]
    Ingest(#[from] IngestInitError),
    /// Attachment storage could not be set up.
    #[error(transparent)]
    Attachments(#[from] crate::blobs::AttachmentError),
    /// The web app could not be loaded.
    #[error(transparent)]
    WebApp(#[from] crate::web::spa::WebAppError),
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
pub(crate) async fn json_413(res: Response) -> Response {
    let is_json = res
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if res.status() == StatusCode::PAYLOAD_TOO_LARGE && !is_json {
        body_too_large().into_response()
    } else {
        res
    }
}

/// What the global body limit answers.
pub(crate) fn body_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        "request body is too large",
    )
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
pub(crate) async fn deadline(
    State(limit): State<Duration>,
    mut req: Request,
    next: Next,
) -> Response {
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
pub(crate) async fn access_log(req: Request, next: Next) -> Response {
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
        ("POST".to_owned(), "/api/v1/pair/redeem".to_owned()),
        // Authenticated by its HMAC signature instead (see `crate::ingest`).
        ("POST".to_owned(), "/api/v1/ingest/email".to_owned()),
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
    let limited = Router::new()
        .route("/health", get(health))
        .merge(crate::web::router(state.clone()))
        .merge(crate::mcp::router(state.clone()))
        .merge(crate::releases::router(&state))
        .route_layer(from_fn(api::tag_route))
        .nest("/api/v1", api::router(state.clone()))
        .layer(from_fn_with_state(limits.request_timeout(), deadline))
        .layer(DefaultBodyLimit::max(limits.max_body_bytes))
        .layer(RequestBodyLimitLayer::new(limits.max_body_bytes));
    // The upload route has its own body cap and deadline instead of the ones above (see
    // `api::attachments`). Merging `limited` into it, not the other way round, keeps `limited`'s
    // fallback (with the limits) as the app's fallback.
    api::attachments::upload_router(state.clone())
        .merge(limited)
        .layer(from_fn(crate::admin::not_on_the_public_listener))
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
    /// The admin listener and its router, when `[admin] bind` is set.
    admin: Option<(TcpListener, Router)>,
    engine: Arc<Engine>,
    authsvc: Arc<AuthService>,
    ingest: Arc<Ingest>,
    attachments: Arc<crate::blobs::Attachments>,
    /// The attachment garbage collector; awaited before the engine is closed.
    collector: JoinHandle<()>,
    push_worker: Option<JoinHandle<()>>,
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
        let ingest = {
            let (config, svc) = (config.clone(), authsvc.clone());
            let db = config.database_path();
            tokio::task::spawn_blocking(move || -> Result<Ingest, IngestInitError> {
                let ingest = Ingest::new(&config, &db)?;
                if let Some(n) = ingest.seed(&config.ingest, svc.now())? {
                    // Audited by the store, in the same transaction.
                    tracing::info!(
                        count = n,
                        "seeded the email sender allowlist from the config"
                    );
                }
                Ok(ingest)
            })
            .await
            .map_err(|e| ServerError::Io {
                context: "starting email ingest".into(),
                source: std::io::Error::other(e.to_string()),
            })??
        };
        if ingest.enabled() {
            tracing::info!("email ingest is enabled");
        }
        let ingest = Arc::new(ingest);
        let attachments = {
            let (cfg, dir, engine) = (
                config.attachments.clone(),
                config.data_dir.clone(),
                engine.clone(),
            );
            tokio::task::spawn_blocking(move || crate::blobs::Attachments::open(&cfg, &dir, engine))
                .await
                .map_err(|e| ServerError::Io {
                    context: "opening attachment storage".into(),
                    source: std::io::Error::other(e.to_string()),
                })??
        };
        let attachments = Arc::new(attachments);
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
        let admin = match (config.admin.bind, config.admin_token_hash()) {
            (Some(bind), Some(hash)) => {
                // `validate` refused anything else; checked again because exposure is the point.
                if !bind.ip().is_loopback() {
                    return Err(ServerError::Config(ConfigError::Invalid(format!(
                        "admin.bind {bind} is not a loopback address"
                    ))));
                }
                let l = TcpListener::bind(bind)
                    .await
                    .map_err(|source| ServerError::Io {
                        context: format!("cannot bind the admin listener {bind}"),
                        source,
                    })?;
                Some((l, hash))
            }
            _ => None,
        };
        let web = match config.web.dir.clone() {
            Some(dir) => {
                let app = tokio::task::spawn_blocking(move || crate::web::spa::WebApp::load(&dir))
                    .await
                    .map_err(|e| ServerError::Io {
                        context: "loading the web app".into(),
                        source: std::io::Error::other(e.to_string()),
                    })??;
                tracing::info!("serving the web app at /app");
                Some(Arc::new(app))
            }
            None => None,
        };
        let releases = if let Some(dir) = &config.releases.dir {
            Some(
                crate::releases::directory(dir)
                    .await
                    .map_err(|source| ServerError::Io {
                        context: "opening the desktop releases directory".into(),
                        source,
                    })?,
            )
        } else {
            None
        };
        let push = config.push.clone().map(Arc::new);
        let sender = if let Some(cfg) = push.clone() {
            Some(
                tokio::task::spawn_blocking(move || crate::push::Sender::new(&cfg))
                    .await
                    .map_err(|_| ConfigError::Invalid("could not initialize push".into()))?
                    .map_err(|err| ConfigError::Invalid(err.to_string()))?,
            )
        } else {
            None
        };
        let (shutdown, shutdown_rx) = watch::channel(false);
        let push_worker = sender.map(|sender| {
            tracing::info!("Android reminder wake-ups are enabled");
            tokio::spawn(sender.run(authsvc.clone(), engine.enable_push(), shutdown_rx.clone()))
        });
        let collector = attachments.spawn_collector(shutdown_rx.clone());
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
            ingest: ingest.clone(),
            attachments: attachments.clone(),
            web,
            releases,
            push,
        };
        let admin = admin.map(|(l, hash)| {
            let st = crate::admin::AdminState::new(state.clone(), hash, &config.admin);
            (l, crate::admin::router(st))
        });
        Ok(Self {
            listener,
            router: router(state),
            admin,
            engine,
            authsvc,
            ingest,
            attachments,
            collector,
            push_worker,
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

    /// The admin listener's bound address, when it is on.
    pub fn admin_addr(&self) -> Result<Option<SocketAddr>, ServerError> {
        self.admin
            .as_ref()
            .map(|(l, _)| {
                l.local_addr().map_err(|source| ServerError::Io {
                    context: "admin local address".into(),
                    source,
                })
            })
            .transpose()
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
        // The admin listener stops when the public one starts shutting down, and drains alongside it.
        let admin = self.admin.map(|(listener, router)| {
            tracing::info!(addr = ?listener.local_addr().ok(), "admin API listening (loopback only)");
            let mut stopping = self.shutdown.subscribe();
            let (stop, _) = watch::channel(false);
            tokio::spawn(crate::conn::serve(
                listener,
                router,
                self.limits.clone(),
                stop,
                async move {
                    let _ = stopping.wait_for(|s| *s).await;
                },
            ))
        });
        crate::conn::serve(
            self.listener,
            self.router,
            self.limits,
            self.shutdown,
            signal,
        )
        .await;
        if let Some(a) = admin
            && let Err(e) = a.await
        {
            tracing::error!(error = %e, "admin listener failed");
        }
        // `serve` set the shutdown flag and dropped its sender: the collector finishes a run in progress
        // (it may need the writer) and ends, dropping its handle on the engine.
        if let Err(e) = self.collector.await {
            tracing::error!(error = %e, "attachment garbage collector failed");
        }
        if let Some(worker) = self.push_worker
            && let Err(e) = worker.await
        {
            tracing::error!(error = %e, "FCM worker failed");
        }
        drop(self.attachments);
        // Refuse late writes (from timed-out requests still on the blocking pool) and release the
        // ownership lock so the next server can open the database.
        let engine = self.engine;
        let _ = tokio::task::spawn_blocking(move || engine.close()).await;
        Ok(())
    }

    /// Runs the server on a background task. Used by tests and embedders.
    pub fn spawn(self) -> Result<RunningServer, ServerError> {
        let addr = self.local_addr()?;
        let admin_addr = self.admin_addr()?;
        let engine = self.engine();
        let authsvc = self.auth_service();
        let ingest = self.ingest.clone();
        let attachments = self.attachments.clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(self.run_until(async move {
            let _ = stopped.await;
        }));
        Ok(RunningServer {
            addr,
            admin_addr,
            engine,
            authsvc,
            ingest,
            attachments,
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
    /// The admin listener's address, when it is on.
    pub admin_addr: Option<SocketAddr>,
    /// The engine.
    pub engine: Arc<Engine>,
    /// The authentication service (tokens, grants, revocation).
    pub authsvc: Arc<AuthService>,
    /// Email ingest (its store holds the allowlist, list keys and the Inbox).
    pub ingest: Arc<Ingest>,
    /// Attachment content (registry and budget).
    pub attachments: Arc<crate::blobs::Attachments>,
    stop: tokio::sync::oneshot::Sender<()>,
    task: JoinHandle<Result<(), ServerError>>,
}

impl RunningServer {
    /// Base URL, for example `http://127.0.0.1:12345`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Base URL of the admin listener, for example `http://127.0.0.1:12346`, when it is on.
    #[must_use]
    pub fn admin_url(&self) -> Option<String> {
        self.admin_addr.map(|a| format!("http://{a}"))
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
