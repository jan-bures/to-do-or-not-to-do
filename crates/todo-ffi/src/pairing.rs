//! Device pairing, the `gh auth login` way: `POST /api/v1/pair/start` with a device name, show the owner
//! a code and a link to approve it with a passkey, then `POST /api/v1/pair/poll` until they do.
//!
//! The requests carry no token. The client never follows redirects, so a hostile or misconfigured server
//! can't bounce the poll token (or later the device token) to another origin. The approval link is built
//! from the address the user typed, never from the server's `verification_uri`.
//!
//! This mirrors `todo_client::pairing` from the desktop's pairing work; once that is on `main`, these
//! functions become thin wrappers around it.

use crate::FfiError;
use crate::runtime::runtime;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;
use todo_client::platform::Response;
use todo_client::protocol::error_detail;

/// `POST /api/v1/pair/start`.
pub const START_PATH: &str = "/api/v1/pair/start";
/// `POST /api/v1/pair/poll`.
pub const POLL_PATH: &str = "/api/v1/pair/poll";

const TIMEOUT: Duration = Duration::from_secs(30);

/// Checks a server address the user typed and returns its origin, for example `https://todo.example.com`.
///
/// A missing scheme means `https://`. Plain `http://` is refused, unless `allow_loopback_http` is set
/// (debug builds only) and the host is a loopback address (a server on this device, or one reached
/// through `adb reverse`). User names, passwords, paths, queries and fragments are refused: the address
/// is the server's origin, nothing more.
#[uniffi::export]
pub fn normalize_server_address(
    input: String,
    allow_loopback_http: bool,
) -> Result<String, FfiError> {
    let bad = |reason: &str| FfiError::BadAddress {
        reason: reason.into(),
    };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(bad("Enter the server's address."));
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let url = url::Url::parse(&with_scheme).map_err(|_| bad("That isn't a web address."))?;
    let Some(host) = url.host_str() else {
        return Err(bad("The address has no host name."));
    };
    match url.scheme() {
        "https" => {}
        "http" if allow_loopback_http && is_loopback(host) => {}
        "http" if allow_loopback_http => {
            return Err(bad(
                "Use https:// (plain http:// only works for this device's own loopback address).",
            ));
        }
        "http" => {
            return Err(bad(
                "Use https://: this app only talks to servers over HTTPS.",
            ));
        }
        _ => return Err(bad("The address must start with https://.")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad("The address must not contain a user name or password."));
    }
    if url.query().is_some() || url.fragment().is_some() || !matches!(url.path(), "" | "/") {
        return Err(bad(
            "Enter just the server's address, without a path (for example todo.example.com).",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn is_loopback(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The admin pages on the paired server, with the same HTTP policy as the native client.
#[uniffi::export]
pub fn admin_url(server: String, allow_loopback_http: bool) -> Option<String> {
    use todo_client::native::{self, Cleartext};
    let policy = if allow_loopback_http {
        Cleartext::Loopback
    } else {
        Cleartext::Refuse
    };
    native::admin_url(&server, policy)
}

/// The approval page for `user_code` on `server` (an origin from [`normalize_server_address`]):
/// `<server>/pair?code=<code>`, with the code percent-encoded.
#[uniffi::export]
pub fn approval_url(server: String, user_code: String) -> String {
    let mut url = format!("{}/pair?code=", server.trim_end_matches('/'));
    for b in user_code.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            url.push(char::from(b));
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

/// A started pairing.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairStarted {
    /// Show this to the owner: they confirm it on the approval page.
    pub user_code: String,
    /// Secret: poll with it. Never show or log it.
    pub poll_token: String,
    /// The page the owner opens, built from the typed address.
    pub approval_url: String,
    /// Seconds until the request expires.
    pub expires_in_secs: u32,
    /// Seconds to wait between polls.
    pub interval_secs: u32,
}

impl std::fmt::Debug for PairStarted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairStarted")
            .field("user_code", &self.user_code)
            .field("expires_in_secs", &self.expires_in_secs)
            .finish_non_exhaustive()
    }
}

/// Where a pairing stands.
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PairPoll {
    /// Not decided yet: poll again after `interval_secs`.
    Pending {
        /// Seconds to wait.
        interval_secs: u32,
    },
    /// Approved. The only time the server sends the token: store it at once.
    Approved {
        /// The device's ID (the actor the server records on its changes).
        device_id: String,
        /// The device's bearer token (`tdd_…`).
        token: String,
    },
    /// The owner denied it.
    Denied,
}

impl std::fmt::Debug for PairPoll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending { interval_secs } => write!(f, "Pending({interval_secs}s)"),
            Self::Approved { device_id, .. } => write!(f, "Approved({device_id})"),
            Self::Denied => f.write_str("Denied"),
        }
    }
}

#[derive(Deserialize)]
struct StartedBody {
    user_code: String,
    poll_token: String,
    expires_in: u32,
    interval: u32,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PolledBody {
    Pending { interval: u32 },
    Approved { device_id: String, token: String },
    Denied,
}

#[derive(Serialize)]
struct StartRequest<'a> {
    device_name: &'a str,
}

#[derive(Serialize)]
struct PollRequest<'a> {
    poll_token: &'a str,
}

fn client() -> Result<reqwest::Client, FfiError> {
    // Ignore the error: the sync transport may have installed the provider already.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .tls_backend_rustls()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| FfiError::Network {
            detail: format!("cannot build the HTTP client: {e}"),
        })
}

fn network(e: &reqwest::Error) -> FfiError {
    if e.is_timeout() {
        FfiError::Network {
            detail: "no answer in time".into(),
        }
    } else {
        FfiError::Network {
            detail: todo_client::native::error_chain(e),
        }
    }
}

async fn post(server: &str, path: &str, body: Vec<u8>) -> Result<Response, FfiError> {
    let res = client()?
        .post(format!("{server}{path}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| network(&e.without_url()))?;
    let status = res.status().as_u16();
    let retry_after = res
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .map(Duration::from_secs);
    let body = res
        .bytes()
        .await
        .map_err(|e| network(&e.without_url()))?
        .to_vec();
    Ok(Response {
        status,
        retry_after,
        body,
    })
}

fn refused(res: &Response) -> FfiError {
    if (300..400).contains(&res.status) {
        return FfiError::Protocol {
            detail: format!("the server redirected ({}); check the address", res.status),
        };
    }
    let detail = error_detail(res);
    if res.status == 404
        && detail
            .as_ref()
            .is_some_and(|d| d.code == "pairing_not_found")
    {
        return FfiError::Expired;
    }
    let (code, message) = detail.map_or_else(
        || (String::new(), format!("HTTP {}", res.status)),
        |d| (d.code, d.message),
    );
    FfiError::Refused {
        status: res.status,
        code,
        detail: message,
        retry_after_secs: res.retry_after.map(|d| d.as_secs()),
    }
}

fn json<T: Serialize>(v: &T) -> Result<Vec<u8>, FfiError> {
    serde_json::to_vec(v).map_err(|e| FfiError::Protocol {
        detail: e.to_string(),
    })
}

fn parse<T: for<'de> Deserialize<'de>>(res: &Response) -> Result<T, FfiError> {
    serde_json::from_slice(&res.body).map_err(|_| FfiError::Protocol {
        detail: "not a to-do server (unexpected pairing response)".into(),
    })
}

/// Starts pairing at `server` as `device_name`.
pub async fn start(
    server: &str,
    device_name: &str,
    allow_loopback_http: bool,
) -> Result<PairStarted, FfiError> {
    let server = normalize_server_address(server.to_owned(), allow_loopback_http)?;
    let res = post(
        &server,
        START_PATH,
        json(&StartRequest {
            device_name: device_name.trim(),
        })?,
    )
    .await?;
    if res.status != 200 {
        return Err(refused(&res));
    }
    let body: StartedBody = parse(&res)?;
    Ok(PairStarted {
        approval_url: approval_url(server, body.user_code.clone()),
        user_code: body.user_code,
        poll_token: body.poll_token,
        expires_in_secs: body.expires_in,
        interval_secs: body.interval.max(1),
    })
}

/// Polls a pairing once.
pub async fn poll(
    server: &str,
    poll_token: &str,
    allow_loopback_http: bool,
) -> Result<PairPoll, FfiError> {
    let server = normalize_server_address(server.to_owned(), allow_loopback_http)?;
    let res = post(&server, POLL_PATH, json(&PollRequest { poll_token })?).await?;
    if res.status != 200 {
        return Err(refused(&res));
    }
    Ok(match parse::<PolledBody>(&res)? {
        PolledBody::Pending { interval } => PairPoll::Pending {
            interval_secs: interval.max(1),
        },
        PolledBody::Approved { device_id, token } => PairPoll::Approved { device_id, token },
        PolledBody::Denied => PairPoll::Denied,
    })
}

/// Starts pairing (blocking; call it off the main thread). `allow_loopback_http` as for
/// [`normalize_server_address`]: only a debug build passes `true`.
#[uniffi::export]
pub fn pair_start(
    server: String,
    device_name: String,
    allow_loopback_http: bool,
) -> Result<PairStarted, FfiError> {
    runtime()?.block_on(start(&server, &device_name, allow_loopback_http))
}

/// Polls a pairing once (blocking; call it off the main thread).
#[uniffi::export]
pub fn pair_poll(
    server: String,
    poll_token: String,
    allow_loopback_http: bool,
) -> Result<PairPoll, FfiError> {
    runtime()?.block_on(poll(&server, &poll_token, allow_loopback_http))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As a debug build calls it (loopback http allowed).
    fn ok(s: &str) -> String {
        normalize_server_address(s.into(), true).unwrap()
    }

    fn err(s: &str) -> bool {
        matches!(
            normalize_server_address(s.into(), true),
            Err(FfiError::BadAddress { .. })
        )
    }

    /// As a release build calls it.
    fn release(s: &str) -> Result<String, FfiError> {
        normalize_server_address(s.into(), false)
    }

    #[test]
    fn a_release_build_refuses_every_plain_http_address() {
        for s in [
            "http://localhost:47390",
            "http://127.0.0.1",
            "http://[::1]:8080",
            "http://todo.example.com",
            "http://10.0.2.2:47390",
        ] {
            assert!(
                matches!(release(s), Err(FfiError::BadAddress { .. })),
                "{s}"
            );
        }
        assert_eq!(
            release("todo.example.com").unwrap(),
            "https://todo.example.com"
        );
    }

    #[test]
    fn a_debug_build_allows_plain_http_only_to_loopback() {
        assert_eq!(ok("http://localhost:47390"), "http://localhost:47390");
        assert!(err("http://todo.example.com"));
        assert!(err("http://10.0.2.2:47390"));
        assert!(err("http://192.168.1.10:47390"));
    }

    #[test]
    fn addresses_become_origins() {
        assert_eq!(ok("todo.example.com"), "https://todo.example.com");
        assert_eq!(
            ok("  https://todo.example.com/ "),
            "https://todo.example.com"
        );
        assert_eq!(
            ok("https://TODO.example.com:8443"),
            "https://todo.example.com:8443"
        );
        assert_eq!(ok("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(ok("http://localhost:8080"), "http://localhost:8080");
        assert_eq!(ok("http://[::1]:8080"), "http://[::1]:8080");
    }

    #[test]
    fn unsafe_or_ambiguous_addresses_are_refused() {
        assert!(err(""));
        assert!(err("http://todo.example.com"));
        assert!(err("http://10.0.2.2:8080"));
        assert!(err("ftp://todo.example.com"));
        assert!(err("https://user:pw@todo.example.com"));
        assert!(err("https://todo.example.com/api"));
        assert!(err("https://todo.example.com/?x=1"));
        assert!(err("https://todo.example.com/#frag"));
        assert!(err("https://"));
    }

    #[test]
    fn approval_urls_encode_the_code() {
        assert_eq!(
            approval_url("https://todo.example.com/".into(), "ABCD-EFGH".into()),
            "https://todo.example.com/pair?code=ABCD-EFGH"
        );
        assert_eq!(
            approval_url("https://todo.example.com".into(), "a b&c".into()),
            "https://todo.example.com/pair?code=a%20b%26c"
        );
    }

    #[test]
    fn debug_never_prints_secrets() {
        let s = PairStarted {
            user_code: "ABCD-EFGH".into(),
            poll_token: "poll-secret".into(),
            approval_url: "https://todo.example.com/pair?code=ABCD-EFGH".into(),
            expires_in_secs: 300,
            interval_secs: 5,
        };
        assert!(!format!("{s:?}").contains("poll-secret"));
        let p = PairPoll::Approved {
            device_id: "phone".into(),
            token: "tdd_secret".into(),
        };
        assert!(!format!("{p:?}").contains("tdd_secret"));
    }
}
