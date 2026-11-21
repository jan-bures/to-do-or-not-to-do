//! Device pairing, the `gh auth login` way: `POST /api/v1/pair/start`, show the owner a code and a page to
//! open, then `POST /api/v1/pair/poll` until they approve it with a passkey. Runs on any [`Transport`]; the
//! requests carry no token (use an anonymous transport, `native::HttpTransport::anonymous` on the
//! desktop).
//!
//! The poll token and the device token are secrets. Neither type here prints them in `Debug`.

use crate::platform::{Request, Response, Transport, TransportError};
use crate::protocol::error_detail;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;
use todo_core::DeviceId;

/// `POST /api/v1/pair/start`.
pub const START_PATH: &str = "/api/v1/pair/start";
/// `POST /api/v1/pair/poll`.
pub const POLL_PATH: &str = "/api/v1/pair/poll";

/// A started pairing.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Started {
    /// Show this to the owner: they confirm it on the verification page.
    pub user_code: String,
    /// Secret; poll with it. Never show or log it.
    pub poll_token: String,
    /// The page the server suggests. Don't open it as given: a hostile server could point it at another
    /// site or a non-web handler. Open [`approval_url`] instead, built from the address the owner typed.
    pub verification_uri: String,
    /// Seconds until the request expires.
    pub expires_in: u32,
    /// Seconds to wait between polls.
    pub interval: u32,
}

impl fmt::Debug for Started {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Started")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

impl Started {
    /// How long to wait before the next poll.
    #[must_use]
    pub fn interval(&self) -> Duration {
        Duration::from_secs(u64::from(self.interval.max(1)))
    }
}

/// Where a pairing stands.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Polled {
    /// Not decided yet; poll again after `interval` seconds.
    Pending {
        /// Seconds to wait.
        interval: u32,
    },
    /// Approved. The only time the server sends the token: store it securely.
    Approved {
        /// The device's ID (the actor recorded on its changes).
        device_id: DeviceId,
        /// The device's bearer token.
        token: String,
    },
    /// The owner denied the request.
    Denied,
}

impl fmt::Debug for Polled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending { interval } => f
                .debug_struct("Pending")
                .field("interval", interval)
                .finish(),
            Self::Approved { device_id, .. } => f
                .debug_struct("Approved")
                .field("device_id", device_id)
                .finish_non_exhaustive(),
            Self::Denied => f.write_str("Denied"),
        }
    }
}

/// Pairing failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PairError {
    /// No response (offline, DNS, TLS, timeout, ...).
    #[error("cannot reach the server: {0}")]
    Transport(#[from] TransportError),
    /// The request expired, was already finished, or the server forgot it: start again.
    #[error("the pairing request expired; start again")]
    Expired,
    /// The server answered with an error (bad device name, rate limit, pairing not configured, ...).
    #[error("the server refused: {message} ({status})")]
    Refused {
        /// HTTP status.
        status: u16,
        /// The error code, if the body had one.
        code: String,
        /// Its message.
        message: String,
        /// The `Retry-After` delay, if any.
        retry_after: Option<Duration>,
    },
    /// The answer was not understood (not a pairing server?).
    #[error("unexpected answer from the server: {0}")]
    Protocol(String),
}

/// The approval page for `user_code` on the server at `base_url` (an origin already checked, for example
/// by `native::server_origin`): `<base>/pair?code=<code>`, with the code percent-encoded.
#[must_use]
pub fn approval_url(base_url: &str, user_code: &str) -> String {
    let mut url = format!("{}/pair?code=", base_url.trim_end_matches('/'));
    for b in user_code.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            url.push(char::from(b));
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

#[derive(Serialize)]
struct StartBody<'a> {
    device_name: &'a str,
}

#[derive(Serialize)]
struct PollBody<'a> {
    poll_token: &'a str,
}

fn refused(res: &Response) -> PairError {
    let detail = error_detail(res);
    if res.status == 404
        && detail
            .as_ref()
            .is_some_and(|d| d.code == "pairing_not_found")
    {
        return PairError::Expired;
    }
    let (code, message) = detail.map_or_else(
        || (String::new(), format!("HTTP {}", res.status)),
        |d| (d.code, d.message),
    );
    PairError::Refused {
        status: res.status,
        code,
        message,
        retry_after: res.retry_after,
    }
}

fn parse<T: for<'de> Deserialize<'de>>(res: &Response) -> Result<T, PairError> {
    if res.status != 200 {
        return Err(refused(res));
    }
    serde_json::from_slice(&res.body).map_err(|e| PairError::Protocol(e.to_string()))
}

fn body(value: &impl Serialize) -> Result<Vec<u8>, PairError> {
    serde_json::to_vec(value).map_err(|e| PairError::Protocol(e.to_string()))
}

/// Starts pairing this device under `device_name` (1–64 printable characters; the owner sees it when
/// approving).
pub async fn start<T: Transport>(transport: &T, device_name: &str) -> Result<Started, PairError> {
    let req = Request::post(START_PATH, body(&StartBody { device_name })?);
    parse(&transport.send(req).await?)
}

/// Polls a started pairing once. Call it every [`Started::interval`] until it is no longer pending.
pub async fn poll<T: Transport>(transport: &T, poll_token: &str) -> Result<Polled, PairError> {
    let req = Request::post(POLL_PATH, body(&PollBody { poll_token })?);
    parse(&transport.send(req).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(status: u16, body: &str) -> Response {
        Response {
            status,
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn answers_parse_and_secrets_stay_out_of_debug() {
        let started: Started = parse(&res(
            200,
            r#"{"user_code":"ABCD-EFGH","poll_token":"s3cret-poll","verification_uri":"https://todo.example.com/pair?code=ABCD-EFGH","expires_in":300,"interval":2}"#,
        ))
        .unwrap();
        assert_eq!(started.user_code, "ABCD-EFGH");
        assert_eq!(started.interval(), Duration::from_secs(2));
        assert!(!format!("{started:?}").contains("s3cret"));

        let approved: Polled = parse(&res(
            200,
            r#"{"status":"approved","device_id":"laptop","token":"s3cret-token"}"#,
        ))
        .unwrap();
        assert!(
            matches!(&approved, Polled::Approved { device_id, .. } if device_id.as_str() == "laptop")
        );
        assert!(!format!("{approved:?}").contains("s3cret"));
        let pending: Polled = parse(&res(200, r#"{"status":"pending","interval":2}"#)).unwrap();
        assert_eq!(pending, Polled::Pending { interval: 2 });
    }

    #[test]
    fn the_approval_url_is_built_from_the_typed_origin() {
        assert_eq!(
            approval_url("https://todo.example.com/", "ABCD-EFGH"),
            "https://todo.example.com/pair?code=ABCD-EFGH"
        );
        // Whatever the server sends as a code, it can't leave the query or the origin.
        assert_eq!(
            approval_url("https://todo.example.com", "x&y=1#@evil.example/ä"),
            "https://todo.example.com/pair?code=x%26y%3D1%23%40evil.example%2F%C3%A4"
        );
    }

    #[test]
    fn errors_are_classified() {
        let gone = res(
            404,
            r#"{"error":{"code":"pairing_not_found","message":"no such pairing"}}"#,
        );
        assert_eq!(parse::<Polled>(&gone), Err(PairError::Expired));
        let busy = res(
            429,
            r#"{"error":{"code":"too_many_pairings","message":"slow down"}}"#,
        );
        assert!(matches!(
            parse::<Started>(&busy),
            Err(PairError::Refused { status: 429, code, .. }) if code == "too_many_pairings"
        ));
        assert!(matches!(
            parse::<Started>(&res(200, "<html>")),
            Err(PairError::Protocol(_))
        ));
        assert!(matches!(
            parse::<Started>(&res(502, "bad gateway")),
            Err(PairError::Refused { status: 502, .. })
        ));
    }
}
