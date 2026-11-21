#![allow(clippy::unwrap_used)]
//! Connection-level bounds: the header-read timeout, the connection cap, the write-progress watchdog
//! (which also frees an event-stream slot) and the shutdown drain deadline.
//!
//! These tests use raw sockets only, so that the test client's connection pool does not count
//! against the limits.

mod common;

use common::{TOKEN_A, TestServer};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Reads until EOF (or a reset), returning what arrived. Panics if that takes longer than `within`.
async fn read_until_closed(sock: &mut TcpStream, within: Duration) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    tokio::time::timeout(within, async {
        loop {
            match sock.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
    })
    .await
    .expect("the server did not close the connection");
    out
}

/// Sends `GET /health` and returns the status line.
async fn health(sock: &mut TcpStream) -> String {
    sock.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf))
        .await
        .expect("no answer")
        .unwrap();
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn a_client_that_sends_headers_slowly_is_dropped() {
    let t = TestServer::start_with(|c| c.limits.header_read_timeout_ms = 300).await;
    let mut sock = TcpStream::connect(t.server.addr).await.unwrap();
    // Half a request head, then nothing.
    sock.write_all(b"GET /health HTTP/1.1\r\nHost: loc")
        .await
        .unwrap();
    let started = Instant::now();
    let got = read_until_closed(&mut sock, Duration::from_secs(5)).await;
    assert!(started.elapsed() >= Duration::from_millis(200));
    let text = String::from_utf8_lossy(&got);
    assert!(
        text.is_empty() || text.starts_with("HTTP/1.1 408"),
        "unexpected answer: {text}"
    );
    // An idle keep-alive connection is closed by the same timer.
    let mut sock = TcpStream::connect(t.server.addr).await.unwrap();
    assert_eq!(health(&mut sock).await, "HTTP/1.1 200 OK");
    read_until_closed(&mut sock, Duration::from_secs(5)).await;
    // A prompt client is served.
    let mut sock = TcpStream::connect(t.server.addr).await.unwrap();
    assert_eq!(health(&mut sock).await, "HTTP/1.1 200 OK");
}

#[tokio::test]
async fn connections_beyond_the_cap_are_closed_until_a_slot_frees() {
    let t = TestServer::start_with(|c| c.limits.max_connections = 2).await;
    let mut a = TcpStream::connect(t.server.addr).await.unwrap();
    assert_eq!(health(&mut a).await, "HTTP/1.1 200 OK");
    let mut b = TcpStream::connect(t.server.addr).await.unwrap();
    assert_eq!(health(&mut b).await, "HTTP/1.1 200 OK");
    // The third is accepted by the kernel and closed by the server without an answer.
    let mut c = TcpStream::connect(t.server.addr).await.unwrap();
    let _ = c
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await;
    assert!(
        read_until_closed(&mut c, Duration::from_secs(5))
            .await
            .is_empty()
    );
    // Closing one frees its slot.
    drop(a);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut d = TcpStream::connect(t.server.addr).await.unwrap();
        let _ = d
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await;
        let mut buf = vec![0u8; 1024];
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(2), d.read(&mut buf)).await
            && n > 0
        {
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
            break;
        }
        assert!(Instant::now() < deadline, "the slot was not freed");
    }
    assert_eq!(health(&mut b).await, "HTTP/1.1 200 OK");
}

#[tokio::test]
async fn a_stream_whose_transport_makes_no_progress_is_closed_and_frees_its_slot() {
    let t = TestServer::start_with(|c| {
        c.limits.max_sse_connections = 1;
        // The stream's own send deadline is far away: only the transport watchdog can act.
        c.limits.sse_stall_timeout_ms = 600_000;
        c.limits.write_stall_timeout_ms = 300;
        c.limits.sse_connect_burst = 1000;
    })
    .await;
    let list = t.create_list("L").await;
    let head = t.server.engine.head_seq();
    // A client that opens a stream and never reads from it.
    let mut stuck = TcpStream::connect(t.server.addr).await.unwrap();
    stuck
        .write_all(
            format!(
                "GET /api/v1/events?since={head} HTTP/1.1\r\nHost: localhost\r\n\
                 Authorization: Bearer {TOKEN_A}\r\nAccept: text/event-stream\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let open = |since: u64| {
        let t = &t;
        async move {
            t.client
                .get(format!("{}/api/v1/events?since={since}", t.url))
                .header("authorization", format!("Bearer {TOKEN_A}"))
                .header("accept", "text/event-stream")
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(open(head).await, reqwest::StatusCode::SERVICE_UNAVAILABLE);
    // Large events fill the socket buffers; the blocked write then trips the watchdog.
    let notes = "x".repeat(90_000);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        for _ in 0..10 {
            t.create_task(&list, "big", json!({ "notes": notes })).await;
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
        let head = t.server.engine.head_seq();
        if open(head).await == reqwest::StatusCode::OK {
            break;
        }
        assert!(Instant::now() < deadline, "the stuck stream kept its slot");
    }
    // The transport itself was closed: after the buffered bytes, EOF.
    read_until_closed(&mut stuck, Duration::from_secs(10)).await;
}

#[tokio::test]
async fn shutdown_closes_connections_that_do_not_finish_within_the_drain_deadline() {
    let t = TestServer::start_with(|c| {
        c.limits.shutdown_drain_ms = 300;
        c.limits.request_timeout_ms = 60_000;
    })
    .await;
    // A request whose body never arrives keeps its connection busy.
    let mut sock = TcpStream::connect(t.server.addr).await.unwrap();
    sock.write_all(
        format!(
            "POST /api/v1/commands HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN_A}\r\n\
             Content-Type: application/json\r\nContent-Length: 200\r\n\r\n{{"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(5), t.stop())
        .await
        .expect("shutdown waited for the stuck connection");
    assert!(started.elapsed() >= Duration::from_millis(250));
    read_until_closed(&mut sock, Duration::from_secs(5)).await;
}
