//! Runtime configuration.
//!
//! Configuration comes from a TOML file (path from `--config` or `TODO_SERVER_CONFIG`) and environment
//! overrides. Nothing deployment-specific is compiled in: the defaults bind to loopback only and there are
//! no hostnames anywhere in the binary.
//!
//! ```toml
//! bind = "127.0.0.1:47320"
//! data_dir = "/var/lib/todo"
//! time_zone = "UTC"
//! # The public URL clients and browsers use. Passkeys (RP ID and origin), pairing links and share
//! # links are derived from it; without it, sign-in, pairing and share links are disabled.
//! public_origin = "https://todo.example.com"
//! # Trusted proxy. Per-address limits (sign-in, setup, pairing, share pages, failed API tokens) key on
//! # the client IP. By default that is the TCP peer address and every forwarding header
//! # (`CF-Connecting-IP`, `X-Forwarded-For`, ...) is ignored, so clients cannot choose their address.
//! # Set this to true only when the server is reachable solely through a Cloudflare Tunnel (cloudflared
//! # connects from the same host and the server binds to loopback): then the peer is always cloudflared,
//! # and `CF-Connecting-IP`, which Cloudflare sets, is the real client. `X-Forwarded-For` is never used.
//! trust_cloudflare_headers = false
//!
//! [limits]
//! max_body_bytes = 1048576
//! request_timeout_ms = 30000
//! max_sse_connections = 64
//! sse_heartbeat_ms = 25000
//! max_concurrent_reads = 16
//! max_concurrent_writes = 8
//! sse_stall_timeout_ms = 30000
//! sse_connects_per_minute = 30
//! sse_connect_burst = 10
//! max_replay_events = 1000
//! max_replay_bytes = 4194304
//! max_connections = 256
//! header_read_timeout_ms = 5000
//! write_stall_timeout_ms = 30000
//! shutdown_drain_ms = 10000
//! max_mcp_requests_per_token = 4
//!
//! [auth]
//! session_idle_timeout_mins = 60
//! session_absolute_timeout_hours = 12
//! agent_writes_per_hour = 120
//! api_requests_per_minute = 600
//! api_request_burst = 120
//! auth_requests_per_minute = 30
//! auth_request_burst = 15
//! # Failed API token checks allowed per client IP; past it, the address gets 429 before its token
//! # is looked at, until the budget refills.
//! bearer_failures_per_minute = 30
//! bearer_failure_burst = 30
//! # Pending device pairings per client IP (at most 64 in all).
//! pairings_per_ip = 3
//! # Share page requests per client IP.
//! share_requests_per_minute = 60
//! share_request_burst = 30
//! sign_in_failures_before_lockout = 5
//! lockout_mins = 15
//! # Development and test fallback: accept the static owner tokens below. Off by default; devices
//! # normally pair (`POST /api/v1/pair/start`) and get revocable tokens.
//! allow_config_owner_tokens = false
//!
//! # Attachment content (see `todo_server::blobs` for every key).
//! [attachments]
//! store = "r2"
//! credentials_file = "/etc/todo/r2-attachments.json"
//!
//! [[auth.owner_tokens]]
//! device = "desktop"
//! sha256 = "<hex SHA-256 of the bearer token>"
//!
//! [ingest]
//! # Email to task (`POST /api/v1/ingest/email`, see docs/ingest-email.md). The route answers 404 unless
//! # this names an owner-only file holding the HMAC secret shared with the Email Worker (at least 32
//! # characters; surrounding whitespace is ignored). Also `TODO_SERVER_INGEST_EMAIL_HMAC_SECRET_FILE`.
//! email_hmac_secret_file = "/etc/todo/ingest-email-hmac.secret"
//! # Seeds the sender allowlist once, on the first start that finds it never seeded. After that the
//! # allowlist is managed on the admin page (`/admin/ingest`) or the local admin API only.
//! allowed_senders = ["you@example.com"]
//! # Ingest requests per client IP.
//! requests_per_minute = 60
//! request_burst = 20
//!
//! [admin]
//! # The local admin API (`/api/v1/admin/...`, see docs/admin-api.md): a second listener, off unless
//! # `bind` is set. It must be a loopback address; the server refuses to start otherwise. Never route
//! # a tunnel or proxy to it.
//! bind = "127.0.0.1:47321"
//! # Hex SHA-256 of the admin token (`todo-server admin-token --out <file>` makes one and prints this
//! # line). Required with `bind`. Also `TODO_SERVER_ADMIN_TOKEN_SHA256`.
//! token_sha256 = "<hex SHA-256 of the admin token>"
//! # Requests per minute, and burst, across all callers.
//! requests_per_minute = 120
//! request_burst = 60
//! # Failed token checks per minute, and burst, across all callers; past it, rejected requests get
//! # 429 instead of 401. The right token is never turned away by it.
//! failures_per_minute = 10
//! failure_burst = 10
//! ```

use jiff::tz::TimeZone;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use todo_core::DeviceId;

/// The default bind address: loopback only.
pub const DEFAULT_BIND: &str = "127.0.0.1:47320";
/// Environment variable holding the config file path.
pub const CONFIG_ENV: &str = "TODO_SERVER_CONFIG";
/// Device name used for an owner token supplied through `TODO_SERVER_OWNER_TOKEN_SHA256`.
pub const ENV_TOKEN_DEVICE: &str = "owner";

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file could not be read.
    #[error("cannot read config file {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// The config file is not valid TOML for this schema.
    #[error("invalid config file {path}: {source}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: Box<toml::de::Error>,
    },
    /// An environment override has a bad value.
    #[error("invalid value for {var}: {reason}")]
    Env {
        /// The variable.
        var: &'static str,
        /// What is wrong.
        reason: String,
    },
    /// A value failed validation.
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// Request and connection limits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    /// Largest accepted request body.
    pub max_body_bytes: usize,
    /// Deadline for producing a response, including reading the request body. Applies to every
    /// route; an event stream only needs it for its response head.
    pub request_timeout_ms: u64,
    /// Most concurrent SSE connections.
    pub max_sse_connections: usize,
    /// Interval between SSE heartbeat comments.
    pub sse_heartbeat_ms: u64,
    /// Most reads (state queries, log pages, SSE replay pages) running on the blocking pool at once.
    /// Further reads get 503 with `Retry-After`.
    pub max_concurrent_reads: usize,
    /// Most commands and undos admitted at once (running or waiting for the single writer). Further
    /// writes get 503 with `Retry-After`.
    pub max_concurrent_writes: usize,
    /// An event stream whose client has not accepted a frame for this long is closed and its slot freed.
    pub sse_stall_timeout_ms: u64,
    /// Sustained rate of event-stream (re)connects allowed per principal.
    pub sse_connects_per_minute: u32,
    /// Burst of event-stream (re)connects allowed per principal.
    pub sse_connect_burst: u32,
    /// Most events an event stream replays on connect (or after lagging). A client further behind is
    /// told to page through `GET /api/v1/events?since=` and reconnect.
    pub max_replay_events: usize,
    /// Most bytes of event JSON an event stream replays at once (same fallback as `max_replay_events`).
    pub max_replay_bytes: usize,
    /// Most open TCP connections. Further connections are accepted and closed at once.
    pub max_connections: usize,
    /// Time a client has to send a complete request head, and the most a keep-alive connection may sit
    /// idle between requests.
    pub header_read_timeout_ms: u64,
    /// A connection whose response writes make no progress for this long is closed (the client stopped
    /// reading).
    pub write_stall_timeout_ms: u64,
    /// On shutdown, how long in-flight requests may take to finish before their connections are closed.
    pub shutdown_drain_ms: u64,
    /// Most MCP requests one token may have in flight at once (MCP is served statelessly, so this is
    /// the per-token session cap). Further requests get 429 `too_many_mcp_requests`.
    pub max_mcp_requests_per_token: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_body_bytes: 1024 * 1024,
            request_timeout_ms: 30_000,
            max_sse_connections: 64,
            sse_heartbeat_ms: 25_000,
            max_concurrent_reads: 16,
            max_concurrent_writes: 8,
            sse_stall_timeout_ms: 30_000,
            sse_connects_per_minute: 30,
            sse_connect_burst: 10,
            max_replay_events: 1000,
            max_replay_bytes: 4 * 1024 * 1024,
            max_connections: 256,
            header_read_timeout_ms: 5_000,
            write_stall_timeout_ms: 30_000,
            shutdown_drain_ms: 10_000,
            max_mcp_requests_per_token: 4,
        }
    }
}

impl Limits {
    /// The request timeout as a duration.
    #[must_use]
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }

    /// The SSE heartbeat interval as a duration.
    #[must_use]
    pub fn sse_heartbeat(&self) -> Duration {
        Duration::from_millis(self.sse_heartbeat_ms)
    }

    /// The header read timeout as a duration.
    #[must_use]
    pub fn header_read_timeout(&self) -> Duration {
        Duration::from_millis(self.header_read_timeout_ms)
    }

    /// The write stall deadline as a duration.
    #[must_use]
    pub fn write_stall_timeout(&self) -> Duration {
        Duration::from_millis(self.write_stall_timeout_ms)
    }

    /// The shutdown drain deadline as a duration.
    #[must_use]
    pub fn shutdown_drain(&self) -> Duration {
        Duration::from_millis(self.shutdown_drain_ms)
    }

    /// The SSE stall deadline as a duration.
    #[must_use]
    pub fn sse_stall_timeout(&self) -> Duration {
        Duration::from_millis(self.sse_stall_timeout_ms)
    }
}

/// An owner bearer token, stored as the SHA-256 hash of the token.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerToken {
    /// The device the token belongs to; recorded as the actor of the owner's events.
    pub device: DeviceId,
    /// Lowercase or uppercase hex SHA-256 of the token.
    pub sha256: String,
}

/// Authentication settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuthConfig {
    /// Accept [`owner_tokens`](Self::owner_tokens). A development and test fallback; off by default.
    pub allow_config_owner_tokens: bool,
    /// Static owner tokens (only with `allow_config_owner_tokens`).
    pub owner_tokens: Vec<OwnerToken>,
    /// A browser session ends after this long without a request.
    pub session_idle_timeout_mins: u64,
    /// A browser session ends this long after sign-in, however active.
    pub session_absolute_timeout_hours: u64,
    /// Default write quota of an agent token (commands and undos per rolling hour).
    pub agent_writes_per_hour: u32,
    /// Sustained API requests per minute per bearer token.
    pub api_requests_per_minute: u32,
    /// API request burst per bearer token.
    pub api_request_burst: u32,
    /// Sustained requests per minute per client IP to the sign-in, setup and pairing routes.
    pub auth_requests_per_minute: u32,
    /// Burst per client IP on those routes.
    pub auth_request_burst: u32,
    /// Sustained failed bearer authentications per minute per client IP.
    pub bearer_failures_per_minute: u32,
    /// Burst of failed bearer authentications per client IP.
    pub bearer_failure_burst: u32,
    /// Pending device pairings per client IP.
    pub pairings_per_ip: u32,
    /// Sustained share page requests per minute per client IP.
    pub share_requests_per_minute: u32,
    /// Share page request burst per client IP.
    pub share_request_burst: u32,
    /// Failed sign-ins (or setup codes) from one IP before it is locked out.
    pub sign_in_failures_before_lockout: u32,
    /// How long a lockout lasts, and the window failures are counted in.
    pub lockout_mins: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            allow_config_owner_tokens: false,
            owner_tokens: Vec::new(),
            session_idle_timeout_mins: 60,
            session_absolute_timeout_hours: 12,
            agent_writes_per_hour: 120,
            api_requests_per_minute: 600,
            api_request_burst: 120,
            auth_requests_per_minute: 30,
            auth_request_burst: 15,
            bearer_failures_per_minute: 30,
            bearer_failure_burst: 30,
            pairings_per_ip: 3,
            share_requests_per_minute: 60,
            share_request_burst: 30,
            sign_in_failures_before_lockout: 5,
            lockout_mins: 15,
        }
    }
}

/// The local admin API (see the module docs and `docs/admin-api.md`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AdminConfig {
    /// Where the admin listener binds: a loopback address. Without it the admin API is off.
    pub bind: Option<SocketAddr>,
    /// Hex SHA-256 of the admin bearer token. Required with `bind`.
    pub token_sha256: Option<String>,
    /// Sustained requests per minute on the admin listener, across all callers.
    pub requests_per_minute: u32,
    /// Request burst on the admin listener.
    pub request_burst: u32,
    /// Failed token checks per minute on the admin listener, across all callers; once spent,
    /// rejected requests get 429 instead of 401 until the budget refills. The right token is never
    /// charged or turned away.
    pub failures_per_minute: u32,
    /// Burst of failed token checks.
    pub failure_burst: u32,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            bind: None,
            token_sha256: None,
            requests_per_minute: 120,
            request_burst: 60,
            failures_per_minute: 10,
            failure_burst: 10,
        }
    }
}

/// The server configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Address to listen on.
    pub bind: SocketAddr,
    /// Directory holding the SQLite database.
    pub data_dir: PathBuf,
    /// The owner's IANA time zone, used for "today" in smart lists.
    pub time_zone: String,
    /// The public origin (`https://host[:port]`) browsers and clients use; see the module docs.
    pub public_origin: Option<String>,
    /// Trust `CF-Connecting-IP` for the client IP (only behind a Cloudflare Tunnel; see the module
    /// docs). Off: the TCP peer address, and forwarding headers are ignored.
    pub trust_cloudflare_headers: bool,
    /// Limits.
    pub limits: Limits,
    /// Authentication.
    pub auth: AuthConfig,
    /// Email ingest.
    pub ingest: IngestConfig,
    /// Attachment content.
    pub attachments: crate::blobs::AttachmentsConfig,
    /// The local admin API.
    pub admin: AdminConfig,
}

/// Email ingest settings (see the module docs and `docs/ingest-email.md`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IngestConfig {
    /// An owner-only file holding the HMAC secret shared with the Email Worker. Without it the ingest
    /// route answers 404.
    pub email_hmac_secret_file: Option<PathBuf>,
    /// Addresses that seed the sender allowlist on the first start (see the module docs).
    pub allowed_senders: Vec<String>,
    /// Sustained ingest requests per minute per client IP.
    pub requests_per_minute: u32,
    /// Ingest request burst per client IP.
    pub request_burst: u32,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            email_hmac_secret_file: None,
            allowed_senders: Vec::new(),
            requests_per_minute: 60,
            request_burst: 20,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 47320)),
            data_dir: PathBuf::from("data"),
            time_zone: "UTC".to_owned(),
            public_origin: None,
            trust_cloudflare_headers: false,
            limits: Limits::default(),
            auth: AuthConfig::default(),
            ingest: IngestConfig::default(),
            attachments: crate::blobs::AttachmentsConfig::default(),
            admin: AdminConfig::default(),
        }
    }
}

fn parse_env<T: std::str::FromStr>(var: &'static str, value: &str) -> Result<T, ConfigError>
where
    T::Err: std::fmt::Display,
{
    value.trim().parse().map_err(|e: T::Err| ConfigError::Env {
        var,
        reason: e.to_string(),
    })
}

impl Config {
    /// Loads the file (if any), applies environment overrides from the process environment, and
    /// validates the result.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut cfg = match path {
            Some(p) => Self::from_file(p)?,
            None => Self::default(),
        };
        cfg.apply_env(|k| std::env::var(k).ok())?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Parses a config file.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::from_toml(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })
    }

    /// Parses TOML text.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Applies `TODO_SERVER_*` overrides read through `get`.
    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) -> Result<(), ConfigError> {
        if let Some(v) = get("TODO_SERVER_BIND") {
            self.bind = parse_env("TODO_SERVER_BIND", &v)?;
        }
        if let Some(v) = get("TODO_SERVER_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Some(v) = get("TODO_SERVER_TIME_ZONE") {
            self.time_zone = v.trim().to_owned();
        }
        if let Some(v) = get("TODO_SERVER_PUBLIC_ORIGIN") {
            let v = v.trim();
            self.public_origin = (!v.is_empty()).then(|| v.to_owned());
        }
        if let Some(v) = get("TODO_SERVER_TRUST_CLOUDFLARE_HEADERS") {
            self.trust_cloudflare_headers = parse_env("TODO_SERVER_TRUST_CLOUDFLARE_HEADERS", &v)?;
        }
        if let Some(v) = get("TODO_SERVER_ALLOW_CONFIG_OWNER_TOKENS") {
            self.auth.allow_config_owner_tokens =
                parse_env("TODO_SERVER_ALLOW_CONFIG_OWNER_TOKENS", &v)?;
        }
        if let Some(v) = get("TODO_SERVER_MAX_BODY_BYTES") {
            self.limits.max_body_bytes = parse_env("TODO_SERVER_MAX_BODY_BYTES", &v)?;
        }
        if let Some(v) = get("TODO_SERVER_REQUEST_TIMEOUT_MS") {
            self.limits.request_timeout_ms = parse_env("TODO_SERVER_REQUEST_TIMEOUT_MS", &v)?;
        }
        if let Some(v) = get("TODO_SERVER_MAX_SSE_CONNECTIONS") {
            self.limits.max_sse_connections = parse_env("TODO_SERVER_MAX_SSE_CONNECTIONS", &v)?;
        }
        if let Some(v) = get("TODO_SERVER_SSE_HEARTBEAT_MS") {
            self.limits.sse_heartbeat_ms = parse_env("TODO_SERVER_SSE_HEARTBEAT_MS", &v)?;
        }
        macro_rules! limit {
            ($var:literal, $field:ident) => {
                if let Some(v) = get($var) {
                    self.limits.$field = parse_env($var, &v)?;
                }
            };
        }
        limit!("TODO_SERVER_MAX_CONCURRENT_READS", max_concurrent_reads);
        limit!("TODO_SERVER_MAX_CONCURRENT_WRITES", max_concurrent_writes);
        limit!("TODO_SERVER_SSE_STALL_TIMEOUT_MS", sse_stall_timeout_ms);
        limit!(
            "TODO_SERVER_SSE_CONNECTS_PER_MINUTE",
            sse_connects_per_minute
        );
        limit!("TODO_SERVER_SSE_CONNECT_BURST", sse_connect_burst);
        limit!("TODO_SERVER_MAX_REPLAY_EVENTS", max_replay_events);
        limit!("TODO_SERVER_MAX_REPLAY_BYTES", max_replay_bytes);
        limit!("TODO_SERVER_MAX_CONNECTIONS", max_connections);
        limit!("TODO_SERVER_HEADER_READ_TIMEOUT_MS", header_read_timeout_ms);
        limit!("TODO_SERVER_WRITE_STALL_TIMEOUT_MS", write_stall_timeout_ms);
        limit!("TODO_SERVER_SHUTDOWN_DRAIN_MS", shutdown_drain_ms);
        limit!(
            "TODO_SERVER_MAX_MCP_REQUESTS_PER_TOKEN",
            max_mcp_requests_per_token
        );
        if let Some(v) = get("TODO_SERVER_INGEST_EMAIL_HMAC_SECRET_FILE") {
            let v = v.trim();
            self.ingest.email_hmac_secret_file = (!v.is_empty()).then(|| PathBuf::from(v));
        }
        if let Some(v) = get("TODO_SERVER_ADMIN_BIND") {
            let v = v.trim();
            self.admin.bind = if v.is_empty() {
                None
            } else {
                Some(parse_env("TODO_SERVER_ADMIN_BIND", v)?)
            };
        }
        if let Some(v) = get("TODO_SERVER_ADMIN_TOKEN_SHA256") {
            let v = v.trim();
            self.admin.token_sha256 = (!v.is_empty()).then(|| v.to_owned());
        }
        if let Some(v) = get("TODO_SERVER_OWNER_TOKEN_SHA256") {
            let device = DeviceId::new(ENV_TOKEN_DEVICE).map_err(|e| ConfigError::Env {
                var: "TODO_SERVER_OWNER_TOKEN_SHA256",
                reason: e.to_string(),
            })?;
            self.auth.owner_tokens.push(OwnerToken {
                device,
                sha256: v.trim().to_owned(),
            });
        }
        Ok(())
    }

    /// Checks values that the type system cannot.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.zone()?;
        self.origin()?;
        let i = &self.ingest;
        if i.requests_per_minute == 0 || i.request_burst == 0 {
            return Err(ConfigError::Invalid(
                "ingest.requests_per_minute and ingest.request_burst must be > 0".into(),
            ));
        }
        for a in &i.allowed_senders {
            if crate::ingest::normalize_address(a).is_none() {
                // Never echo the value: it is an email address.
                return Err(ConfigError::Invalid(
                    "ingest.allowed_senders holds an entry that is not an email address".into(),
                ));
            }
        }
        self.attachments.validate().map_err(ConfigError::Invalid)?;
        self.validate_admin()?;
        let a = &self.auth;
        for (name, value) in [
            ("session_idle_timeout_mins", a.session_idle_timeout_mins),
            (
                "session_absolute_timeout_hours",
                a.session_absolute_timeout_hours,
            ),
            ("agent_writes_per_hour", u64::from(a.agent_writes_per_hour)),
            (
                "api_requests_per_minute",
                u64::from(a.api_requests_per_minute),
            ),
            ("api_request_burst", u64::from(a.api_request_burst)),
            (
                "auth_requests_per_minute",
                u64::from(a.auth_requests_per_minute),
            ),
            ("auth_request_burst", u64::from(a.auth_request_burst)),
            (
                "bearer_failures_per_minute",
                u64::from(a.bearer_failures_per_minute),
            ),
            ("bearer_failure_burst", u64::from(a.bearer_failure_burst)),
            ("pairings_per_ip", u64::from(a.pairings_per_ip)),
            (
                "share_requests_per_minute",
                u64::from(a.share_requests_per_minute),
            ),
            ("share_request_burst", u64::from(a.share_request_burst)),
            (
                "sign_in_failures_before_lockout",
                u64::from(a.sign_in_failures_before_lockout),
            ),
            ("lockout_mins", a.lockout_mins),
        ] {
            if value == 0 {
                return Err(ConfigError::Invalid(format!("auth.{name} must be > 0")));
            }
        }
        for t in &self.auth.owner_tokens {
            decode_hash(&t.sha256).ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "owner token for device `{}` is not a 64-digit hex SHA-256",
                    t.device
                ))
            })?;
        }
        if self.limits.max_body_bytes == 0 {
            return Err(ConfigError::Invalid(
                "limits.max_body_bytes must be > 0".into(),
            ));
        }
        if self.limits.request_timeout_ms == 0 || self.limits.sse_heartbeat_ms == 0 {
            return Err(ConfigError::Invalid(
                "limits.request_timeout_ms and limits.sse_heartbeat_ms must be > 0".into(),
            ));
        }
        let l = &self.limits;
        for (name, value) in [
            ("max_concurrent_reads", l.max_concurrent_reads),
            ("max_concurrent_writes", l.max_concurrent_writes),
            ("max_replay_events", l.max_replay_events),
            ("max_replay_bytes", l.max_replay_bytes),
            (
                "sse_stall_timeout_ms",
                usize::try_from(l.sse_stall_timeout_ms).unwrap_or(usize::MAX),
            ),
            (
                "sse_connects_per_minute",
                l.sse_connects_per_minute as usize,
            ),
            ("sse_connect_burst", l.sse_connect_burst as usize),
            ("max_connections", l.max_connections),
            ("max_mcp_requests_per_token", l.max_mcp_requests_per_token),
            (
                "header_read_timeout_ms",
                usize::try_from(l.header_read_timeout_ms).unwrap_or(usize::MAX),
            ),
            (
                "write_stall_timeout_ms",
                usize::try_from(l.write_stall_timeout_ms).unwrap_or(usize::MAX),
            ),
        ] {
            if value == 0 {
                return Err(ConfigError::Invalid(format!("limits.{name} must be > 0")));
            }
        }
        Ok(())
    }

    fn validate_admin(&self) -> Result<(), ConfigError> {
        let a = &self.admin;
        if a.requests_per_minute == 0
            || a.request_burst == 0
            || a.failures_per_minute == 0
            || a.failure_burst == 0
        {
            return Err(ConfigError::Invalid(
                "admin.requests_per_minute, admin.request_burst, admin.failures_per_minute and admin.failure_burst must be > 0".into(),
            ));
        }
        if let Some(h) = &a.token_sha256
            && decode_hash(h).is_none()
        {
            return Err(ConfigError::Invalid(
                "admin.token_sha256 is not a 64-digit hex SHA-256".into(),
            ));
        }
        let Some(bind) = a.bind else {
            return Ok(());
        };
        if !bind.ip().is_loopback() {
            return Err(ConfigError::Invalid(format!(
                "admin.bind {bind} is not a loopback address; the admin API is local only (use an SSH tunnel to reach it from elsewhere)"
            )));
        }
        if a.token_sha256.is_none() {
            return Err(ConfigError::Invalid(
                "admin.bind is set but admin.token_sha256 is not; run `todo-server admin-token --out <file>` and put the printed line in the config".into(),
            ));
        }
        if bind.port() != 0 && bind == self.bind {
            return Err(ConfigError::Invalid(
                "admin.bind must differ from bind".into(),
            ));
        }
        Ok(())
    }

    /// The admin token's hash, when the admin listener is on.
    #[must_use]
    pub fn admin_token_hash(&self) -> Option<[u8; 32]> {
        self.admin.bind?;
        self.admin.token_sha256.as_deref().and_then(decode_hash)
    }

    /// The configured time zone.
    pub fn zone(&self) -> Result<TimeZone, ConfigError> {
        TimeZone::get(&self.time_zone).map_err(|e| {
            ConfigError::Invalid(format!("unknown time zone `{}`: {e}", self.time_zone))
        })
    }

    /// The validated public origin, if configured.
    pub fn origin(&self) -> Result<Option<PublicOrigin>, ConfigError> {
        self.public_origin
            .as_deref()
            .map(PublicOrigin::parse)
            .transpose()
    }

    /// Path of the SQLite database file.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("todo.sqlite3")
    }

    /// The database path as the server resolves it when it runs in `cwd`: a relative `data_dir` is
    /// relative to the working directory, so this is the file a server started there opens.
    #[must_use]
    pub fn database_path_in(&self, cwd: &Path) -> PathBuf {
        cwd.join(self.database_path())
    }
}

/// The public origin: `https://host[:port]`, or `http://` for loopback development hosts only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOrigin {
    url: webauthn_rs::prelude::Url,
    origin: String,
    host: String,
}

impl PublicOrigin {
    /// Parses and checks an origin.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let bad = |why: &str| ConfigError::Invalid(format!("public_origin `{text}`: {why}"));
        let url = webauthn_rs::prelude::Url::parse(text.trim()).map_err(|e| bad(&e.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| bad("no host"))?
            .to_ascii_lowercase();
        let loopback = matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
        match url.scheme() {
            "https" => {}
            "http" if loopback => {}
            _ => return Err(bad("must be https (http only for localhost)")),
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(bad("must be an origin, without a path, query or fragment"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(bad("must not contain credentials"));
        }
        let origin = url.origin().ascii_serialization();
        Ok(Self { url, origin, host })
    }

    /// The origin as browsers send it in the `Origin` header (no trailing slash).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.origin
    }

    /// The origin as a URL.
    #[must_use]
    pub fn url(&self) -> &webauthn_rs::prelude::Url {
        &self.url
    }

    /// The host: the WebAuthn relying-party ID.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// True if the origin is served over HTTPS.
    #[must_use]
    pub fn is_https(&self) -> bool {
        self.url.scheme() == "https"
    }
}

/// Decodes a hex SHA-256 digest.
#[must_use]
pub fn decode_hash(hex_digest: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex_digest.trim()).ok()?;
    bytes.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_origins() {
        let o = PublicOrigin::parse("https://Todo.Example.com:8443/").unwrap();
        assert_eq!(o.as_str(), "https://todo.example.com:8443");
        assert_eq!(o.host(), "todo.example.com");
        assert!(o.is_https());
        let dev = PublicOrigin::parse("http://localhost:47320").unwrap();
        assert!(!dev.is_https());
        for bad in [
            "ftp://todo.example.com",
            "https://u:p@todo.example.com",
            "nope",
        ] {
            assert!(PublicOrigin::parse(bad).is_err(), "{bad}");
        }
        let c = Config::default();
        assert!(
            !c.auth.allow_config_owner_tokens,
            "static owner tokens are off by default"
        );
        assert!(!c.trust_cloudflare_headers);
    }

    #[test]
    fn defaults_bind_to_loopback() {
        let c = Config::default();
        assert_eq!(c.bind.to_string(), DEFAULT_BIND);
        assert!(c.bind.ip().is_loopback());
        c.validate().unwrap();
    }

    #[test]
    fn file_and_env_overrides() {
        let mut c = Config::from_toml(
            r#"
            data_dir = "d"
            [limits]
            sse_heartbeat_ms = 100
            [[auth.owner_tokens]]
            device = "desktop"
            sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
            "#,
        )
        .unwrap();
        assert_eq!(c.limits.sse_heartbeat_ms, 100);
        assert_eq!(c.limits.max_body_bytes, 1024 * 1024);
        c.apply_env(|k| match k {
            "TODO_SERVER_BIND" => Some("127.0.0.1:9".into()),
            "TODO_SERVER_MAX_CONCURRENT_READS" => Some("3".into()),
            "TODO_SERVER_TIME_ZONE" => Some("Europe/Paris".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.bind.port(), 9);
        assert_eq!(c.limits.max_concurrent_reads, 3);
        c.validate().unwrap();
        c.limits.max_concurrent_writes = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn bad_values_are_rejected() {
        assert!(Config::from_toml("nope = 1").is_err());
        let mut c = Config::default();
        assert!(
            c.apply_env(|k| (k == "TODO_SERVER_BIND").then(|| "x".into()))
                .is_err()
        );
        let mut c = Config {
            time_zone: "Mars/Olympus".into(),
            ..Config::default()
        };
        assert!(c.validate().is_err());
        c.time_zone = "UTC".into();
        c.public_origin = Some("http://todo.example.com".into());
        assert!(c.validate().is_err(), "plain http off loopback");
        c.public_origin = Some("https://todo.example.com/app".into());
        assert!(c.validate().is_err(), "a path");
        c.public_origin = Some("https://todo.example.com".into());
        c.validate().unwrap();
        c.auth.agent_writes_per_hour = 0;
        assert!(c.validate().is_err());
        c.auth.agent_writes_per_hour = 1;
        c.auth.owner_tokens.push(OwnerToken {
            device: DeviceId::new("x").unwrap(),
            sha256: "abc".into(),
        });
        assert!(c.validate().is_err());
    }

    #[test]
    fn database_path_resolves_a_relative_data_dir_against_the_working_directory() {
        let cwd = Path::new("/var/lib/todo");
        let relative = Config::default();
        assert_eq!(
            relative.database_path_in(cwd),
            cwd.join("data").join("todo.sqlite3")
        );
        let absolute = Config {
            data_dir: std::env::temp_dir().join("todo-data"),
            ..Config::default()
        };
        assert_eq!(
            absolute.database_path_in(cwd),
            std::env::temp_dir().join("todo-data").join("todo.sqlite3"),
            "an absolute data_dir ignores the working directory"
        );
    }

    const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn the_admin_listener_is_off_by_default_and_loopback_only() {
        let c = Config::default();
        assert_eq!(c.admin.bind, None);
        assert_eq!(c.admin_token_hash(), None);

        let mut c = Config::from_toml(&format!(
            "[admin]\nbind = \"127.0.0.1:47321\"\ntoken_sha256 = \"{HASH}\"\n"
        ))
        .unwrap();
        c.validate().unwrap();
        assert_eq!(c.admin_token_hash(), Some([0; 32]));
        c.admin.bind = Some("[::1]:47321".parse().unwrap());
        c.validate().unwrap();

        for bad in [
            "0.0.0.0:47321",
            "[::]:47321",
            "192.168.1.10:47321",
            "8.8.8.8:1",
        ] {
            c.admin.bind = Some(bad.parse().unwrap());
            let e = c.validate().unwrap_err().to_string();
            assert!(e.contains("not a loopback address"), "{bad}: {e}");
        }
        c.admin.bind = Some(c.bind);
        assert!(
            c.validate().is_err(),
            "the same port as the public listener"
        );
        c.admin.bind = Some("127.0.0.1:47321".parse().unwrap());
        c.admin.token_sha256 = None;
        assert!(c.validate().is_err(), "a bind without a token");
        c.admin.token_sha256 = Some("abc".into());
        assert!(c.validate().is_err(), "a malformed hash");

        let mut c = Config::default();
        c.apply_env(|k| match k {
            "TODO_SERVER_ADMIN_BIND" => Some("127.0.0.1:9".into()),
            "TODO_SERVER_ADMIN_TOKEN_SHA256" => Some(HASH.into()),
            _ => None,
        })
        .unwrap();
        c.validate().unwrap();
        assert_eq!(c.admin.bind.unwrap().port(), 9);
        c.apply_env(|k| (k == "TODO_SERVER_ADMIN_BIND").then(|| "10.0.0.1:9".into()))
            .unwrap();
        assert!(c.validate().is_err());
    }
}
