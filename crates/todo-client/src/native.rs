//! Native platform implementations (feature `native`): a reqwest transport with rustls, a tokio clock
//! and OS entropy. For the desktop app and tests.

pub mod attachments;

use crate::Revocation;
use crate::platform::{
    Clock, Entropy, EventStream, Method, Request, Response, SseFrame, StreamOpen, Transport,
    TransportError,
};
use crate::sse::{DEFAULT_MAX_FRAME_BYTES, SseParser};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;
use todo_core::time::Timestamp;

/// A transport could not be built.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The base URL is not a usable server origin (see [`server_origin`]).
    #[error("{0}")]
    BadUrl(&'static str),
    /// The base URL is plain `http` to another machine: tokens would cross the network unencrypted.
    #[error("the server address must use https:// (plain http only works for this computer)")]
    Insecure,
    /// The token contains characters a header cannot carry.
    #[error("the token is not a valid header value")]
    BadToken,
    /// reqwest refused the configuration.
    #[error("cannot build the HTTP client: {0}")]
    Client(String),
}

/// The longest a finite request (pairing, REST, downloads) may take from start to the last body byte.
/// Event streams have no total limit; the sync engine drops a stream that goes quiet instead.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether a transport may speak plain `http`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cleartext {
    /// `https` only. What a released app uses.
    #[default]
    Refuse,
    /// Also `http` to a loopback host (`localhost`, `127.0.0.1`, `[::1]`): a server on this machine, or
    /// one reached through `adb reverse`. For development builds and tests only.
    Loopback,
}

/// Parses and checks a server base URL, for example `https://todo.example.com`, and returns it without a
/// trailing slash. Every client validates addresses with this one function, so they agree on the host.
///
/// - The scheme is `https`, or `http` only for a loopback host (`localhost`, `127.0.0.1`, `[::1]`) during
///   development. [`server_origin_with`] with [`Cleartext::Refuse`] allows `https` only.
/// - No user name or password, query string or fragment.
///
/// # Errors
///
/// [`BuildError::BadUrl`] or [`BuildError::Insecure`].
pub fn server_origin(base_url: &str) -> Result<String, BuildError> {
    server_origin_with(base_url, Cleartext::Loopback)
}

/// [`server_origin`] with an explicit [`Cleartext`] policy.
///
/// # Errors
///
/// [`BuildError::BadUrl`] or [`BuildError::Insecure`].
pub fn server_origin_with(base_url: &str, cleartext: Cleartext) -> Result<String, BuildError> {
    let url = url::Url::parse(base_url.trim())
        .map_err(|_| BuildError::BadUrl("that doesn't look like a server address"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(BuildError::BadUrl(
            "the server address can't contain a user name or password",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(BuildError::BadUrl(
            "the server address can't contain ? or #",
        ));
    }
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => return Err(BuildError::BadUrl("the server address has no host")),
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback && cleartext == Cleartext::Loopback => {}
        "http" => return Err(BuildError::Insecure),
        _ => {
            return Err(BuildError::BadUrl(
                "the server address must start with https://",
            ));
        }
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// The paired server's admin pages. Use the same base URL and cleartext policy as the transport.
#[must_use]
pub fn admin_url(base_url: &str, cleartext: Cleartext) -> Option<String> {
    let base = server_origin_with(base_url, cleartext).ok()?;
    Some(format!("{base}{}", crate::ADMIN_PATH))
}

/// HTTP over reqwest (rustls with the *ring* provider), with the bearer token on every request.
#[derive(Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    base: String,
    /// `None` for an anonymous transport (pairing).
    auth: Option<HeaderValue>,
    revocation: Option<Revocation>,
    max_frame_bytes: usize,
    request_timeout: Duration,
}

impl fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the token.
        f.debug_struct("HttpTransport")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl HttpTransport {
    /// A transport for the server at `base_url` (for example `https://todo.example.com`), learnt at
    /// pairing time, authenticating with `token`.
    pub fn new(base_url: &str, token: &str) -> Result<Self, BuildError> {
        let mut auth =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| BuildError::BadToken)?;
        auth.set_sensitive(true);
        Self::build(base_url, Some(auth))
    }

    /// A transport without a token, for the endpoints that take none (pairing).
    pub fn anonymous(base_url: &str) -> Result<Self, BuildError> {
        Self::build(base_url, None)
    }

    /// The server's base URL, without a trailing slash.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// A transport with an explicit [`Cleartext`] policy: `token` for the device's requests, `None` for
    /// pairing. [`new`](Self::new) and [`anonymous`](Self::anonymous) allow loopback `http`.
    pub fn with_cleartext(
        base_url: &str,
        token: Option<&str>,
        cleartext: Cleartext,
    ) -> Result<Self, BuildError> {
        let auth = match token {
            Some(token) => {
                let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| BuildError::BadToken)?;
                auth.set_sensitive(true);
                Some(auth)
            }
            None => None,
        };
        Self::build_with(base_url, auth, cleartext)
    }

    fn build(base_url: &str, auth: Option<HeaderValue>) -> Result<Self, BuildError> {
        Self::build_with(base_url, auth, Cleartext::Loopback)
    }

    fn build_with(
        base_url: &str,
        auth: Option<HeaderValue>,
        cleartext: Cleartext,
    ) -> Result<Self, BuildError> {
        let base = server_origin_with(base_url, cleartext)?;
        // Ignore the error: another part of the process may have installed a provider already.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .tls_backend_rustls()
            .connect_timeout(Duration::from_secs(10))
            // Never follow a redirect: it would carry the token, or a pairing secret in a 307/308 body,
            // to wherever the server points. A 3xx is an error instead (see `refuse_redirect`).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| BuildError::Client(e.to_string()))?;
        Ok(Self {
            client,
            base,
            auth,
            revocation: None,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    /// Shares a pairing's revocation latch, including for transfers outside the sync loops.
    #[must_use]
    pub fn with_revocation(mut self, revocation: Revocation) -> Self {
        self.revocation = Some(revocation);
        self
    }

    fn observe_response(&self, res: &reqwest::Response) {
        if self.auth.is_some()
            && let Some(revocation) = &self.revocation
        {
            revocation.observe_status(res.status().as_u16());
        }
    }

    /// Limits each finite request to `timeout`, from sending it to its last body byte (default
    /// [`DEFAULT_REQUEST_TIMEOUT`]). For an event stream it limits the wait for the response headers.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Caps one event stream frame at `bytes` (default [`DEFAULT_MAX_FRAME_BYTES`]). A larger frame
    /// ends the stream with [`TransportError::Protocol`].
    #[must_use]
    pub fn with_max_frame_bytes(mut self, bytes: usize) -> Self {
        self.max_frame_bytes = bytes;
        self
    }

    /// The request's URL on the configured server, or an error if it would be anywhere else. The path must
    /// start with exactly one `/`; the resulting URL must keep the base's scheme, host, port and path
    /// prefix. Checked before the bearer token is attached, so a path like `@evil.example/x` or
    /// `.evil.example/x` can never carry it to another host.
    fn url_for(&self, path: &str) -> Result<url::Url, TransportError> {
        let refuse = || {
            TransportError::Protocol(format!(
                "refusing a request path that could leave the server: {path:?}"
            ))
        };
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains('\\')
            || path.chars().any(char::is_control)
        {
            return Err(refuse());
        }
        let base = url::Url::parse(&self.base).map_err(|_| refuse())?;
        let url = url::Url::parse(&format!("{}{path}", self.base)).map_err(|_| refuse())?;
        let same_origin = url.scheme() == base.scheme()
            && url.host() == base.host()
            && url.port_or_known_default() == base.port_or_known_default()
            && url.username().is_empty()
            && url.password().is_none();
        let prefix = base.path().trim_end_matches('/');
        let under_base =
            url.path().starts_with(prefix) && url.path()[prefix.len()..].starts_with('/');
        if !same_origin || !under_base {
            return Err(refuse());
        }
        Ok(url)
    }

    fn authenticated(
        &self,
        method: Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, TransportError> {
        let url = self.url_for(path)?;
        let mut b = match method {
            Method::Get => self.client.get(url),
            Method::Post => self.client.post(url),
            Method::Put => self.client.put(url),
            Method::Delete => self.client.delete(url),
        };
        if let Some(auth) = &self.auth {
            b = b.header(AUTHORIZATION, auth.clone());
        }
        Ok(b)
    }

    /// Open a bounded-lifetime authenticated response without buffering its body.
    #[cfg(feature = "assistant")]
    pub(crate) async fn assistant_body(
        &self,
        request: Request,
    ) -> Result<reqwest::Response, TransportError> {
        let res = self
            .builder(&request)?
            .header(ACCEPT, "text/event-stream, application/json")
            .timeout(Duration::from_secs(85))
            .send()
            .await
            .map_err(network)?;
        self.observe_response(&res);
        refuse_redirect(&res)?;
        Ok(res)
    }

    fn builder(&self, request: &Request) -> Result<reqwest::RequestBuilder, TransportError> {
        let mut b = self.authenticated(request.method, &request.path)?;
        if let Some(body) = &request.body {
            b = b
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone());
        }
        Ok(b)
    }
}

fn network(e: reqwest::Error) -> TransportError {
    if e.is_timeout() {
        TransportError::Timeout
    } else {
        // Without the URL: it is the configured server, and status messages need not repeat it.
        TransportError::Network(error_chain(&e.without_url()))
    }
}

/// An error and its sources, joined with `: `.
///
/// reqwest's own message is only "error sending request"; the cause (DNS, connection refused, a TLS
/// certificate problem) is in the source chain.
pub fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        // Some layers repeat their source's message; keep each message once.
        if parts.last().is_none_or(|last| !last.contains(&text)) {
            parts.push(text);
        }
        source = s.source();
    }
    parts.join(": ")
}

fn retry_after(res: &reqwest::Response) -> Option<Duration> {
    res.headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_secs)
}

/// A 3xx is never followed (the client has no redirect policy) and never treated as an answer.
fn refuse_redirect(res: &reqwest::Response) -> Result<(), TransportError> {
    if res.status().is_redirection() {
        return Err(TransportError::Protocol(format!(
            "the server answered with a redirect (HTTP {}), which is not followed",
            res.status().as_u16()
        )));
    }
    Ok(())
}

async fn read(res: reqwest::Response) -> Result<Response, TransportError> {
    refuse_redirect(&res)?;
    let status = res.status().as_u16();
    let retry_after = retry_after(&res);
    let body = if status == 401 {
        // Authentication refusal is conclusive at the headers, even if the body stalls or breaks.
        Vec::new()
    } else {
        res.bytes().await.map_err(network)?.to_vec()
    };
    Ok(Response {
        status,
        retry_after,
        body,
    })
}

/// An SSE response body, parsed incrementally.
pub struct HttpEventStream {
    response: reqwest::Response,
    parser: SseParser,
    queue: VecDeque<SseFrame>,
}

impl fmt::Debug for HttpEventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpEventStream")
            .field("queued", &self.queue.len())
            .finish_non_exhaustive()
    }
}

impl EventStream for HttpEventStream {
    async fn next_frame(&mut self) -> Option<Result<SseFrame, TransportError>> {
        loop {
            if let Some(frame) = self.queue.pop_front() {
                return Some(Ok(frame));
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => match self.parser.push(&chunk) {
                    Ok(frames) => self.queue.extend(frames),
                    Err(e) => return Some(Err(TransportError::Protocol(e.to_string()))),
                },
                Ok(None) => return None,
                Err(e) => return Some(Err(network(e))),
            }
        }
    }
}

impl Transport for HttpTransport {
    type Stream = HttpEventStream;

    async fn send(&self, request: Request) -> Result<Response, TransportError> {
        // The timeout covers the body as well: a server that stalls mid-answer can't hang a request.
        let res = self
            .builder(&request)?
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(network)?;
        self.observe_response(&res);
        read(res).await
    }

    async fn open_stream(
        &self,
        request: Request,
    ) -> Result<StreamOpen<HttpEventStream>, TransportError> {
        // Only the wait for the headers is limited; the open stream is watched by the sync engine's
        // idle timeout.
        let res = tokio::time::timeout(
            self.request_timeout,
            self.builder(&request)?
                .header(ACCEPT, "text/event-stream")
                .send(),
        )
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(network)?;
        self.observe_response(&res);
        refuse_redirect(&res)?;
        if res.status().as_u16() != 200 {
            return Ok(StreamOpen::Refused(read(res).await?));
        }
        Ok(StreamOpen::Open(HttpEventStream {
            response: res,
            parser: SseParser::with_max_frame_bytes(self.max_frame_bytes),
            queue: VecDeque::new(),
        }))
    }
}

/// The system clock with tokio timers. Needs a tokio runtime with the time driver.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Random bytes from the operating system.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(16) {
            let r = uuid::Uuid::new_v4().into_bytes();
            chunk.copy_from_slice(&r[..chunk.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cleartext, admin_url};

    #[test]
    fn admin_pages_use_the_paired_base_and_transport_policy() {
        for (server, expected) in [
            ("https://todo.example.com", "https://todo.example.com/admin"),
            (
                "https://todo.example.com:8443/",
                "https://todo.example.com:8443/admin",
            ),
            (
                "https://todo.example.com/todo/",
                "https://todo.example.com/todo/admin",
            ),
        ] {
            assert_eq!(
                admin_url(server, Cleartext::Refuse).as_deref(),
                Some(expected)
            );
        }
        for server in [
            "http://localhost:47390",
            "http://127.0.0.1:47390",
            "http://[::1]:47390",
        ] {
            assert_eq!(
                admin_url(server, Cleartext::Loopback),
                Some(format!("{server}/admin"))
            );
            assert_eq!(admin_url(server, Cleartext::Refuse), None);
        }
        for invalid in [
            "todo.example.com",
            "http://todo.example.com",
            "file:///etc/hosts",
            "javascript:alert(1)",
            "https://user:password@todo.example.com",
            "https://todo.example.com/?redirect=elsewhere",
            "https://todo.example.com/#fragment",
        ] {
            assert_eq!(admin_url(invalid, Cleartext::Loopback), None, "{invalid}");
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("{0}")]
    struct Layer(&'static str, #[source] Option<Box<Layer>>);

    #[test]
    fn error_chain_includes_every_source_once() {
        let inner = Layer("invalid peer certificate: UnknownIssuer", None);
        let middle = Layer(
            "invalid peer certificate: UnknownIssuer",
            Some(Box::new(inner)),
        );
        let outer = Layer("error sending request", Some(Box::new(middle)));
        assert_eq!(
            super::error_chain(&outer),
            "error sending request: invalid peer certificate: UnknownIssuer"
        );
        assert_eq!(super::error_chain(&Layer("plain", None)), "plain");
    }
}
