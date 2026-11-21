//! Native platform implementations (feature `native`): a reqwest transport with rustls, a tokio clock
//! and OS entropy. For the desktop app and tests.

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
    /// The base URL is not an `http` or `https` URL.
    #[error("the server URL must start with http:// or https://")]
    BadUrl,
    /// The token contains characters a header cannot carry.
    #[error("the token is not a valid header value")]
    BadToken,
    /// reqwest refused the configuration.
    #[error("cannot build the HTTP client: {0}")]
    Client(String),
}

/// HTTP over reqwest (rustls with the *ring* provider), with the bearer token on every request.
#[derive(Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    base: String,
    /// `None` for an anonymous transport (pairing).
    auth: Option<HeaderValue>,
    max_frame_bytes: usize,
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

    fn build(base_url: &str, auth: Option<HeaderValue>) -> Result<Self, BuildError> {
        let base = base_url.trim_end_matches('/').to_owned();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(BuildError::BadUrl);
        }
        // Ignore the error: another part of the process may have installed a provider already.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .tls_backend_rustls()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BuildError::Client(e.to_string()))?;
        Ok(Self {
            client,
            base,
            auth,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        })
    }

    /// Caps one event stream frame at `bytes` (default [`DEFAULT_MAX_FRAME_BYTES`]). A larger frame
    /// ends the stream with [`TransportError::Protocol`].
    #[must_use]
    pub fn with_max_frame_bytes(mut self, bytes: usize) -> Self {
        self.max_frame_bytes = bytes;
        self
    }

    fn builder(&self, request: &Request) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base, request.path);
        let mut b = match request.method {
            Method::Get => self.client.get(url),
            Method::Post => self.client.post(url),
        };
        if let Some(auth) = &self.auth {
            b = b.header(AUTHORIZATION, auth.clone());
        }
        if let Some(body) = &request.body {
            b = b
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone());
        }
        b
    }
}

fn network(e: reqwest::Error) -> TransportError {
    if e.is_timeout() {
        TransportError::Timeout
    } else {
        // Without the URL: it is the configured server, and status messages need not repeat it.
        TransportError::Network(e.without_url().to_string())
    }
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

async fn read(res: reqwest::Response) -> Result<Response, TransportError> {
    let status = res.status().as_u16();
    let retry_after = retry_after(&res);
    let body = res.bytes().await.map_err(network)?.to_vec();
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
        let res = self.builder(&request).send().await.map_err(network)?;
        read(res).await
    }

    async fn open_stream(
        &self,
        request: Request,
    ) -> Result<StreamOpen<HttpEventStream>, TransportError> {
        let res = self
            .builder(&request)
            .header(ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(network)?;
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
