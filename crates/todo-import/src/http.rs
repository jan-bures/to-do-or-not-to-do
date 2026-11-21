//! A small blocking REST client for the server, with retries.
//!
//! Retries: `429` and `503` wait for exactly as long as `Retry-After` says (seconds or an HTTP-date),
//! or back off exponentially without one; `408`, `500`, `502`, `504` and transport errors back off
//! exponentially. The status is classified before the body is read, so a broken error body cannot hide
//! a `Retry-After`. Every other status is returned to the caller. Commands are safe to retry because
//! each carries its own `command_id`.
//!
//! The client never waits longer than its budget ([`Client::max_wait`]) in total for one request. If the
//! server asks for more, the request fails at once with [`HttpError::WaitTooLong`]; `apply` resumes
//! from its journal when it is run again.
//!
//! The token is sent only over HTTPS, or over plain HTTP to a loopback address. It is never logged and
//! never part of an error.

use jiff::Timestamp;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;

/// Header the server sets on a replayed command result.
pub const REPLAYED_HEADER: &str = "idempotent-replayed";

/// Default for [`Client::max_wait`].
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(15 * 60);

/// The longest one attempt of a content upload ([`Client::put_bytes`]) may take: a 25 MiB body over a
/// slow link needs longer than other requests (the server's own upload deadline is 10 minutes).
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(11 * 60);

/// How the client waits between attempts. Tests replace it to record waits instead.
pub type Sleeper = Arc<dyn Fn(Duration) + Send + Sync>;

/// Errors from the client.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The URL is not acceptable.
    #[error("server URL: {0}")]
    BadUrl(String),
    /// Retries were exhausted.
    #[error("{what}: giving up after {attempts} attempts (last: {last})")]
    GaveUp {
        /// The request (method and route, no IDs of content).
        what: String,
        /// Attempts made.
        attempts: u32,
        /// The last failure.
        last: String,
    },
    /// The server answered with an error status.
    #[error("{what}: HTTP {status} {code}")]
    Status {
        /// The request.
        what: String,
        /// HTTP status.
        status: u16,
        /// The server's error code.
        code: String,
        /// The server's message (may mention entity IDs; never logged above debug).
        message: String,
    },
    /// The server asked to wait longer than the client's budget.
    #[error(
        "{what}: the server asks to wait {}s more (HTTP {status}), past the budget of {}s; nothing is lost, run again later to resume",
        wait.as_secs(),
        budget.as_secs()
    )]
    WaitTooLong {
        /// The request.
        what: String,
        /// The status that asked for the wait.
        status: u16,
        /// What the server asked for.
        wait: Duration,
        /// [`Client::max_wait`].
        budget: Duration,
    },
    /// The body could not be parsed.
    #[error("{what}: unexpected response body: {error}")]
    Body {
        /// The request.
        what: String,
        /// The parse error.
        error: String,
    },
}

/// A successful response.
#[derive(Debug)]
pub struct Reply {
    /// The parsed body.
    pub body: serde_json::Value,
    /// True if the server replayed a stored command result (or found the content stored already).
    pub replayed: bool,
    /// The HTTP status.
    pub status: u16,
    /// `Content-Length`, if the server sent one.
    pub content_length: Option<u64>,
}

/// The client. `Debug` does not show the token.
pub struct Client {
    agent: ureq::Agent,
    base: String,
    token: String,
    /// Attempts for transport errors and 5xx.
    pub max_attempts: u32,
    /// Attempts while the server says it is busy (429, 503).
    pub max_busy_attempts: u32,
    /// Base delay of the exponential back-off.
    pub base_delay: Duration,
    /// The longest the client waits in total, over all retries of one request.
    pub max_wait: Duration,
    sleep: Sleeper,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split_once(']').map(|(h, _)| h);
    }
    Some(authority.split(':').next().unwrap_or(authority))
}

/// Checks a server base URL: HTTPS, or HTTP to a loopback host. Returns it without a trailing slash.
pub fn check_base_url(url: &str) -> Result<String, HttpError> {
    let url = url.trim().trim_end_matches('/');
    let host = host_of(url).filter(|h| !h.is_empty()).ok_or_else(|| {
        HttpError::BadUrl("expected https://<host>, for example https://todo.example.com".into())
    })?;
    if url.starts_with("https://") {
        return Ok(url.to_owned());
    }
    let loopback = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if url.starts_with("http://") && loopback {
        Ok(url.to_owned())
    } else {
        Err(HttpError::BadUrl(
            "plain http is only allowed to localhost; use https".into(),
        ))
    }
}

/// Parses a `Retry-After` value (RFC 9110): delay seconds, or an HTTP-date, which is turned into the
/// time from `now` until then (zero if it has passed). Nothing is capped.
#[must_use]
pub fn retry_after(value: Option<&str>, now: Timestamp) -> Option<Duration> {
    let v = value?.trim();
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        // More digits than a u64 holds: wait "forever", which the budget then refuses.
        return Some(Duration::from_secs(v.parse().unwrap_or(u64::MAX)));
    }
    let at = jiff::fmt::rfc2822::DateTimeParser::new()
        .parse_timestamp(v)
        .ok()?;
    let ms = at.as_millisecond().saturating_sub(now.as_millisecond());
    Some(Duration::from_millis(u64::try_from(ms).unwrap_or(0)))
}

/// What one request sends.
#[derive(Clone, Copy)]
enum Outgoing<'a> {
    Get,
    Head,
    Json(&'a [u8]),
    /// `PUT` with a raw body.
    Bytes(&'a [u8]),
}

enum Attempt {
    Done(u16, bool, String, Option<u64>),
    /// Retry after the given wait (`None`: back off); true if the server is busy; the status if any;
    /// a description.
    Retry(Option<Duration>, bool, Option<u16>, String),
}

impl Client {
    /// A client for the server at `base` (see [`check_base_url`]).
    pub fn new(base: &str, token: String) -> Result<Self, HttpError> {
        let base = check_base_url(base)?;
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(120)))
            .build();
        Ok(Self {
            agent: ureq::Agent::new_with_config(config),
            base,
            token,
            max_attempts: 8,
            max_busy_attempts: 30,
            base_delay: Duration::from_millis(500),
            max_wait: DEFAULT_MAX_WAIT,
            sleep: Arc::new(std::thread::sleep),
        })
    }

    /// Replaces how the client waits between attempts.
    #[must_use]
    pub fn with_sleeper(mut self, sleep: Sleeper) -> Self {
        self.sleep = sleep;
        self
    }

    /// The base URL.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    fn once(&self, path: &str, send: Outgoing<'_>) -> Attempt {
        let url = format!("{}{path}", self.base);
        let auth = format!("Bearer {}", self.token);
        let result = match send {
            Outgoing::Json(b) => self
                .agent
                .post(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .send(b),
            Outgoing::Bytes(b) => self
                .agent
                .put(&url)
                .config()
                .timeout_global(Some(UPLOAD_TIMEOUT))
                .build()
                .header("Authorization", &auth)
                .header("Content-Type", "application/octet-stream")
                .send(b),
            Outgoing::Get => self.agent.get(&url).header("Authorization", &auth).call(),
            Outgoing::Head => self.agent.head(&url).header("Authorization", &auth).call(),
        };
        let mut resp = match result {
            Ok(r) => r,
            Err(e) => return Attempt::Retry(None, false, None, format!("transport: {e}")),
        };
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        // Classify before touching the body: a retry needs only the status and `Retry-After`.
        match status {
            429 | 503 => {
                let wait = retry_after(header("retry-after").as_deref(), Timestamp::now());
                return Attempt::Retry(wait, true, Some(status), format!("HTTP {status}"));
            }
            408 | 500 | 502 | 504 => {
                return Attempt::Retry(None, false, Some(status), format!("HTTP {status}"));
            }
            _ => {}
        }
        let replayed = header(REPLAYED_HEADER).as_deref() == Some("true");
        let length = header("content-length").and_then(|v| v.trim().parse().ok());
        if matches!(send, Outgoing::Head) {
            return Attempt::Done(status, replayed, String::new(), length);
        }
        match resp
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
        {
            Ok(text) => Attempt::Done(status, replayed, text, length),
            Err(e) => Attempt::Retry(None, false, Some(status), format!("reading body: {e}")),
        }
    }

    fn request(&self, what: &str, path: &str, send: Outgoing<'_>) -> Result<Reply, HttpError> {
        let mut attempts = 0u32;
        let mut busy = 0u32;
        let mut waited = Duration::ZERO;
        loop {
            match self.once(path, send) {
                Attempt::Done(status, replayed, text, length) => {
                    let value: serde_json::Value = if text.trim().is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::from_str(&text).map_err(|e| HttpError::Body {
                            what: what.to_owned(),
                            error: e.to_string(),
                        })?
                    };
                    if (200..300).contains(&status) {
                        return Ok(Reply {
                            body: value,
                            replayed,
                            status,
                            content_length: length,
                        });
                    }
                    let s = |k: &str| value["error"][k].as_str().unwrap_or("").to_owned();
                    return Err(HttpError::Status {
                        what: what.to_owned(),
                        status,
                        code: s("code"),
                        message: s("message"),
                    });
                }
                Attempt::Retry(wait, is_busy, status, last) => {
                    let (n, max) = if is_busy {
                        busy += 1;
                        (busy, self.max_busy_attempts)
                    } else {
                        attempts += 1;
                        (attempts, self.max_attempts)
                    };
                    if n >= max {
                        return Err(HttpError::GaveUp {
                            what: what.to_owned(),
                            attempts: attempts + busy,
                            last,
                        });
                    }
                    let backoff = self
                        .base_delay
                        .saturating_mul(1 << n.min(6))
                        .min(Duration::from_secs(30));
                    let delay = wait.unwrap_or(backoff);
                    if waited.saturating_add(delay) > self.max_wait {
                        return Err(HttpError::WaitTooLong {
                            what: what.to_owned(),
                            status: status.unwrap_or(0),
                            wait: delay,
                            budget: self.max_wait.saturating_sub(waited),
                        });
                    }
                    tracing::warn!(%what, %last, delay_ms = delay.as_millis(), "retrying");
                    (self.sleep)(delay);
                    waited += delay;
                }
            }
        }
    }

    /// `POST path` with a JSON body.
    pub fn post<B: Serialize>(&self, what: &str, path: &str, body: &B) -> Result<Reply, HttpError> {
        let bytes = serde_json::to_vec(body).map_err(|e| HttpError::Body {
            what: what.to_owned(),
            error: e.to_string(),
        })?;
        self.request(what, path, Outgoing::Json(&bytes))
    }

    /// `PUT path` with a raw body (`application/octet-stream`). Retried like every request, so only for
    /// idempotent uploads.
    pub fn put_bytes(&self, what: &str, path: &str, body: &[u8]) -> Result<Reply, HttpError> {
        self.request(what, path, Outgoing::Bytes(body))
    }

    /// `HEAD path`: `Some(Content-Length)` on 2xx (0 if none was sent), `None` on 404.
    pub fn head(&self, what: &str, path: &str) -> Result<Option<u64>, HttpError> {
        match self.request(what, path, Outgoing::Head) {
            Ok(r) => Ok(Some(r.content_length.unwrap_or(0))),
            Err(HttpError::Status { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `GET path`, parsed as `T`.
    pub fn get<T: DeserializeOwned>(&self, what: &str, path: &str) -> Result<T, HttpError> {
        let reply = self.request(what, path, Outgoing::Get)?;
        serde_json::from_value(reply.body).map_err(|e| HttpError::Body {
            what: what.to_owned(),
            error: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls() {
        assert_eq!(
            check_base_url("https://todo.example.com/").unwrap(),
            "https://todo.example.com"
        );
        assert!(check_base_url("http://127.0.0.1:8080").is_ok());
        assert!(check_base_url("http://localhost:8080").is_ok());
        assert!(check_base_url("http://[::1]:8080").is_ok());
        assert!(check_base_url("http://todo.example.com").is_err());
        assert!(check_base_url("todo.example.com").is_err());
        assert!(check_base_url("ftp://todo.example.com").is_err());
    }

    #[test]
    fn retry_after_values() {
        let now: Timestamp = "2015-10-21T07:20:00Z".parse().unwrap();
        let secs = |s| Some(Duration::from_secs(s));
        assert_eq!(retry_after(Some("3"), now), secs(3));
        assert_eq!(retry_after(Some(" 0 "), now), secs(0));
        // Not capped.
        assert_eq!(retry_after(Some("99999"), now), secs(99_999));
        assert_eq!(
            retry_after(Some("99999999999999999999999"), now),
            secs(u64::MAX)
        );
        // An HTTP-date: the time until then, or zero once it has passed.
        assert_eq!(
            retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT"), now),
            secs(480)
        );
        assert_eq!(
            retry_after(Some("Wed, 21 Oct 2015 07:00:00 GMT"), now),
            secs(0)
        );
        assert_eq!(retry_after(Some("soon"), now), None);
        assert_eq!(retry_after(Some("-5"), now), None);
        assert_eq!(retry_after(Some(""), now), None);
        assert_eq!(retry_after(None, now), None);
    }

    #[test]
    fn debug_hides_the_token() {
        let c = Client::new("https://todo.example.com", "secret-token".into()).unwrap();
        assert!(!format!("{c:?}").contains("secret"));
    }
}
