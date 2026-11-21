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
//!
//! [[auth.owner_tokens]]
//! device = "desktop"
//! sha256 = "<hex SHA-256 of the bearer token>"
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuthConfig {
    /// Accepted owner tokens.
    pub owner_tokens: Vec<OwnerToken>,
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
    /// Limits.
    pub limits: Limits,
    /// Authentication.
    pub auth: AuthConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 47320)),
            data_dir: PathBuf::from("data"),
            time_zone: "UTC".to_owned(),
            limits: Limits::default(),
            auth: AuthConfig::default(),
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

    /// The configured time zone.
    pub fn zone(&self) -> Result<TimeZone, ConfigError> {
        TimeZone::get(&self.time_zone).map_err(|e| {
            ConfigError::Invalid(format!("unknown time zone `{}`: {e}", self.time_zone))
        })
    }

    /// Path of the SQLite database file.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("todo.sqlite3")
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
        c.auth.owner_tokens.push(OwnerToken {
            device: DeviceId::new("x").unwrap(),
            sha256: "abc".into(),
        });
        assert!(c.validate().is_err());
    }
}
