//! The TCP accept loop and per-connection HTTP/1 serving.
//!
//! Replaces `axum::serve` so that connections themselves are bounded:
//!
//! - **Connection cap.** A permit from a global semaphore (`limits.max_connections`) is taken before a
//!   connection task is spawned and held for the connection's lifetime. Without a permit the connection
//!   is closed at once.
//! - **Header read timeout.** HTTP/1 with a Tokio timer: a client must send a complete request head
//!   within `limits.header_read_timeout_ms`. The same timer runs while a keep-alive connection waits for
//!   its next request, so idle connections are closed too.
//! - **Write progress.** The socket is wrapped so that a write the kernel cannot accept (the client stopped
//!   reading) is noticed. A connection whose writes make no progress for `limits.write_stall_timeout_ms`
//!   is dropped, which closes the socket and drops the response body (and the event-stream slot it owns).
//! - **Drain.** On shutdown the listener closes, every connection is asked to finish gracefully (event
//!   streams end on the same signal), and after `limits.shutdown_drain_ms` the rest are closed.
//!
//! HTTP/1.1 only: the tunnel in front of the server speaks HTTP/1.1 to the origin.

use crate::config::Limits;
use axum::Router;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;

/// When a connection's writes last stopped making progress.
#[derive(Debug)]
struct WriteProgress {
    origin: Instant,
    /// Milliseconds since `origin` (plus one) at which the current blocked write began; 0 when writes
    /// are flowing.
    blocked_since: AtomicU64,
}

impl WriteProgress {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            blocked_since: AtomicU64::new(0),
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis())
            .unwrap_or(u64::MAX - 1)
            .saturating_add(1)
    }

    fn blocked(&self) {
        let _ =
            self.blocked_since
                .compare_exchange(0, self.now(), Ordering::AcqRel, Ordering::Acquire);
    }

    fn flowing(&self) {
        self.blocked_since.store(0, Ordering::Release);
    }

    fn blocked_for(&self) -> Duration {
        match self.blocked_since.load(Ordering::Acquire) {
            0 => Duration::ZERO,
            since => Duration::from_millis(self.now().saturating_sub(since)),
        }
    }

    /// Completes once a write has been blocked for at least `limit`.
    async fn stalled(&self, limit: Duration) {
        let tick = (limit / 4).max(Duration::from_millis(10));
        loop {
            tokio::time::sleep(tick).await;
            if self.blocked_for() >= limit {
                return;
            }
        }
    }
}

/// A socket that reports whether its writes are making progress.
#[derive(Debug)]
struct Watched {
    inner: TcpStream,
    progress: Arc<WriteProgress>,
}

impl Watched {
    fn track<T>(&self, r: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        match &r {
            Poll::Pending => self.progress.blocked(),
            Poll::Ready(_) => self.progress.flowing(),
        }
        r
    }
}

impl AsyncRead for Watched {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Watched {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let r = Pin::new(&mut this.inner).poll_write(cx, buf);
        this.track(r)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let r = Pin::new(&mut this.inner).poll_write_vectored(cx, bufs);
        this.track(r)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let r = Pin::new(&mut this.inner).poll_flush(cx);
        this.track(r)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

async fn serve_connection(
    stream: TcpStream,
    router: Router,
    limits: Limits,
    mut stop: watch::Receiver<bool>,
    _permit: OwnedSemaphorePermit,
) {
    let _ = stream.set_nodelay(true);
    let progress = Arc::new(WriteProgress::new());
    let io = TokioIo::new(Watched {
        inner: stream,
        progress: progress.clone(),
    });
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header_read_timeout())
        .keep_alive(true);
    let conn = builder.serve_connection(io, TowerToHyperService::new(router));
    tokio::pin!(conn);
    let mut stopping = *stop.borrow_and_update();
    if stopping {
        conn.as_mut().graceful_shutdown();
    }
    let stall = limits.write_stall_timeout();
    loop {
        tokio::select! {
            r = conn.as_mut() => {
                if let Err(e) = r {
                    tracing::debug!(error = %e, "connection ended with an error");
                }
                return;
            }
            _ = stop.changed(), if !stopping => {
                stopping = true;
                conn.as_mut().graceful_shutdown();
            }
            () = progress.stalled(stall) => {
                tracing::debug!("closing a connection whose writes made no progress");
                return;
            }
        }
    }
}

/// Accepts connections until `signal` completes, then drains them. `stop` is set to `true` once the
/// signal fired (it also ends event streams).
pub async fn serve(
    listener: TcpListener,
    router: Router,
    limits: Limits,
    stop: watch::Sender<bool>,
    signal: impl Future<Output = ()> + Send,
) {
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    let mut conns = JoinSet::new();
    tokio::pin!(signal);
    loop {
        tokio::select! {
            () = &mut signal => break,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((stream, _peer)) => {
                    let Ok(permit) = slots.clone().try_acquire_owned() else {
                        tracing::debug!("connection limit reached; closing a new connection");
                        drop(stream);
                        continue;
                    };
                    conns.spawn(serve_connection(
                        stream,
                        router.clone(),
                        limits.clone(),
                        stop.subscribe(),
                        permit,
                    ));
                }
                Err(e) => {
                    // For example too many open files: back off instead of spinning.
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
    }
    tracing::info!("shutting down");
    drop(listener);
    let _ = stop.send(true);
    let drained = tokio::time::timeout(limits.shutdown_drain(), async {
        while conns.join_next().await.is_some() {}
    })
    .await
    .is_ok();
    if !drained {
        tracing::warn!(
            open = conns.len(),
            "drain deadline passed; closing the remaining connections"
        );
        conns.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn progress_reports_blocked_writes_only() {
        let p = WriteProgress::new();
        assert_eq!(p.blocked_for(), Duration::ZERO);
        p.blocked();
        tokio::time::sleep(Duration::from_millis(30)).await;
        p.blocked(); // A repeated Pending keeps the original start.
        assert!(p.blocked_for() >= Duration::from_millis(30));
        p.flowing();
        assert_eq!(p.blocked_for(), Duration::ZERO);
        p.blocked();
        tokio::time::timeout(Duration::from_secs(2), p.stalled(Duration::from_millis(40)))
            .await
            .unwrap();
    }
}
