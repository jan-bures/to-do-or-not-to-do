//! The engine's transport: the native HTTP transport for a paired server, or none at all for the
//! screenshot fixtures (which must never touch the network).

use todo_client::native::{HttpEventStream, HttpTransport};
use todo_client::platform::{
    EventStream, Request, Response, SseFrame, StreamOpen, Transport, TransportError,
};

/// HTTP to the paired server, or permanently offline.
#[derive(Debug)]
pub enum AppTransport {
    /// The paired server.
    Http(HttpTransport),
    /// No server: every request fails as a network error, so commands stay in the outbox.
    Offline,
}

/// The event stream of an [`AppTransport`].
#[derive(Debug)]
pub enum AppStream {
    /// An open SSE response.
    Http(HttpEventStream),
}

impl EventStream for AppStream {
    async fn next_frame(&mut self) -> Option<Result<SseFrame, TransportError>> {
        match self {
            Self::Http(s) => s.next_frame().await,
        }
    }
}

fn offline() -> TransportError {
    TransportError::Network("offline (no server)".into())
}

impl Transport for AppTransport {
    type Stream = AppStream;

    async fn send(&self, request: Request) -> Result<Response, TransportError> {
        match self {
            Self::Http(t) => t.send(request).await,
            Self::Offline => Err(offline()),
        }
    }

    async fn open_stream(&self, request: Request) -> Result<StreamOpen<AppStream>, TransportError> {
        match self {
            Self::Http(t) => Ok(match t.open_stream(request).await? {
                StreamOpen::Open(s) => StreamOpen::Open(AppStream::Http(s)),
                StreamOpen::Refused(r) => StreamOpen::Refused(r),
            }),
            Self::Offline => Err(offline()),
        }
    }
}
