//! Platform abstractions: HTTP transport, event stream, clock and entropy.
//!
//! The sync engine is generic over these traits so that it runs on any executor: tokio on the desktop
//! (see the `native` feature), the Android runtime behind UniFFI, or the browser's event loop on wasm32.
//! Nothing in the default build depends on a runtime.
//!
//! On wasm32 futures are usually not `Send` (browser handles are single-threaded), so the bounds use
//! [`MaybeSend`] and [`MaybeSync`]: `Send`/`Sync` on native targets, nothing on wasm32.

use std::future::Future;
use std::time::Duration;
use todo_core::time::Timestamp;

#[cfg(not(target_arch = "wasm32"))]
mod bounds {
    /// `Send` on native targets; no bound on wasm32.
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}
    /// `Sync` on native targets; no bound on wasm32.
    pub trait MaybeSync: Sync {}
    impl<T: Sync + ?Sized> MaybeSync for T {}
}

#[cfg(target_arch = "wasm32")]
mod bounds {
    /// `Send` on native targets; no bound on wasm32.
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}
    /// `Sync` on native targets; no bound on wasm32.
    pub trait MaybeSync {}
    impl<T: ?Sized> MaybeSync for T {}
}

pub use bounds::{MaybeSend, MaybeSync};

/// One upload route for browser files and native streams. Encode the name as a query value,
/// never as part of the path. A retry with the same attachment ID is idempotent.
#[must_use]
pub fn attachment_upload_path(
    task: todo_core::ids::TaskId,
    attachment: todo_core::ids::AttachmentId,
    name: &str,
) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("file_name", name)
        .append_pair("attachment_id", &attachment.to_string())
        .finish();
    format!("/api/v1/tasks/{task}/attachments?{query}")
}

/// HTTP method of a [`Request`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `GET`
    Get,
    /// `POST`
    Post,
}

/// An HTTP request relative to the server's base URL.
///
/// The transport adds the base URL and the `Authorization: Bearer` header; the engine never sees the
/// token. A body is always JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The method.
    pub method: Method,
    /// Path and query, for example `/api/v1/events?since=42`.
    pub path: String,
    /// JSON body.
    pub body: Option<Vec<u8>>,
}

impl Request {
    /// A `GET` without a body.
    #[must_use]
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            path: path.into(),
            body: None,
        }
    }

    /// A `POST` with a JSON body.
    #[must_use]
    pub fn post(path: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: Method::Post,
            path: path.into(),
            body: Some(body),
        }
    }
}

/// An HTTP response with its whole body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// Status code.
    pub status: u16,
    /// The `Retry-After` header, if it was a number of seconds.
    pub retry_after: Option<Duration>,
    /// Body bytes.
    pub body: Vec<u8>,
}

/// The request did not produce a response.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Connection refused or reset, DNS failure, TLS failure, the body could not be read, ...
    #[error("network error: {0}")]
    Network(String),
    /// No response in time.
    #[error("request timed out")]
    Timeout,
    /// The server's answer broke the protocol (for example an oversized event stream frame).
    #[error("protocol error: {0}")]
    Protocol(String),
}

/// One frame of a Server-Sent Events stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SseFrame {
    /// An event: the `event:` name (`message` if absent), the `id:` and the joined `data:` lines.
    Event {
        /// Event name.
        event: String,
        /// Last event ID, if the frame had one.
        id: Option<String>,
        /// Data.
        data: String,
    },
    /// A comment line (the server's `:heartbeat`). Proves the connection is alive.
    Comment,
}

/// An open event stream.
pub trait EventStream: MaybeSend {
    /// The next frame; `None` when the server closed the stream.
    fn next_frame(
        &mut self,
    ) -> impl Future<Output = Option<Result<SseFrame, TransportError>>> + MaybeSend;
}

/// The outcome of opening an event stream.
#[derive(Debug)]
pub enum StreamOpen<S> {
    /// The server answered 200 with `text/event-stream`.
    Open(S),
    /// The server answered with something else (409 `catch_up_required`, 400 `invalid_since`, 429, ...).
    Refused(Response),
}

/// Request/response HTTP plus Server-Sent Events, with the bearer token and base URL built in.
pub trait Transport: MaybeSend + MaybeSync + 'static {
    /// The event stream type.
    type Stream: EventStream;

    /// Sends a request and reads the whole response. Any status is a `Response`; only failures to get
    /// one are errors.
    fn send(
        &self,
        request: Request,
    ) -> impl Future<Output = Result<Response, TransportError>> + MaybeSend;

    /// Opens an event stream (`GET` with `Accept: text/event-stream`). Resolves once the response head
    /// arrived.
    fn open_stream(
        &self,
        request: Request,
    ) -> impl Future<Output = Result<StreamOpen<Self::Stream>, TransportError>> + MaybeSend;
}

/// Wall-clock time and timers.
pub trait Clock: MaybeSend + MaybeSync + 'static {
    /// The current time.
    fn now(&self) -> Timestamp;

    /// Completes after `duration`.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + MaybeSend;
}

/// A source of random bytes, for command IDs and backoff jitter. It need not be cryptographically
/// strong, but it must differ between devices and runs (UUIDv7 IDs rely on it to avoid collisions).
pub trait Entropy: MaybeSend + MaybeSync + 'static {
    /// Fills `buf` with random bytes.
    fn fill(&self, buf: &mut [u8]);
}
