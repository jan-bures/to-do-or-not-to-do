#![allow(clippy::unwrap_used)]
//! Connection-level bounds: the write-progress watchdog (which also frees an event-stream slot) and the
//! shutdown drain deadline. The header-read timeout and the connection cap are in `tests/security.rs`.
//!
//! These tests use raw sockets only, so that the test client's connection pool does not count
//! against the limits.

mod common;

use common::raw::read_until_closed;
use common::{TOKEN_A, TestServer};
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

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
