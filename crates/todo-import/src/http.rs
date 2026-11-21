//! A small blocking REST client for the server, with retries.
//!
//! Retries: `429` and `503` wait for `Retry-After` (or back off exponentially without one); `408`,
//! `500`, `502`, `504` and transport errors back off exponentially. Every other status is returned to
//! the caller. Commands are safe to retry because each carries its own `command_id`.
//!
//! The token is sent only over HTTPS, or over plain HTTP to a loopback address. It is never logged and
//! never part of an error.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

/// Header the server sets on a replayed command result.
pub const REPLAYED_HEADER: &str = "idempotent-replayed";

/// Longest wait honoured from a `Retry-After` header.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(120);

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
    /// True if the server replayed a stored command result.
    pub replayed: bool,
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

/// Parses a `Retry-After` value in seconds, capped at [`MAX_RETRY_AFTER`].
#[must_use]
pub fn retry_after(value: Option<&str>) -> Option<Duration> {
    let secs: u64 = value?.trim().parse().ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER))
}

enum Attempt {
    Done(u16, bool, String),
    Retry(Option<Duration>, bool, String),
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
        })
    }

    /// The base URL.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    fn once(&self, path: &str, body: Option<&[u8]>) -> Attempt {
        let url = format!("{}{path}", self.base);
        let auth = format!("Bearer {}", self.token);
        let result = match body {
            Some(b) => self
                .agent
                .post(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .send(b),
            None => self.agent.get(&url).header("Authorization", &auth).call(),
        };
        let mut resp = match result {
            Ok(r) => r,
            Err(e) => return Attempt::Retry(None, false, format!("transport: {e}")),
        };
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let replayed = header(REPLAYED_HEADER).as_deref() == Some("true");
        let wait = retry_after(header("retry-after").as_deref());
        let text = match resp
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
        {
            Ok(t) => t,
            Err(e) => return Attempt::Retry(None, false, format!("reading body: {e}")),
        };
        match status {
            429 | 503 => Attempt::Retry(wait, true, format!("HTTP {status}")),
            408 | 500 | 502 | 504 => Attempt::Retry(None, false, format!("HTTP {status}")),
            _ => Attempt::Done(status, replayed, text),
        }
    }

    fn request(&self, what: &str, path: &str, body: Option<&[u8]>) -> Result<Reply, HttpError> {
        let mut attempts = 0u32;
        let mut busy = 0u32;
        loop {
            match self.once(path, body) {
                Attempt::Done(status, replayed, text) => {
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
                Attempt::Retry(wait, is_busy, last) => {
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
                    tracing::warn!(%what, %last, delay_ms = delay.as_millis(), "retrying");
                    std::thread::sleep(delay);
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
        self.request(what, path, Some(&bytes))
    }

    /// `GET path`, parsed as `T`.
    pub fn get<T: DeserializeOwned>(&self, what: &str, path: &str) -> Result<T, HttpError> {
        let reply = self.request(what, path, None)?;
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
        assert_eq!(retry_after(Some("3")), Some(Duration::from_secs(3)));
        assert_eq!(retry_after(Some("99999")), Some(MAX_RETRY_AFTER));
        assert_eq!(retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")), None);
        assert_eq!(retry_after(None), None);
    }

    #[test]
    fn debug_hides_the_token() {
        let c = Client::new("https://todo.example.com", "secret-token".into()).unwrap();
        assert!(!format!("{c:?}").contains("secret"));
    }
}
