#![allow(clippy::unwrap_used)]
//! Event stream tests: JSON pages, live SSE between two clients, resume, heartbeats and the cap.

mod common;

use common::{TOKEN_A, TOKEN_B, TestServer, first_seq, id};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn json_pages_of_the_log() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    for i in 0..5 {
        t.create_task(&list, &format!("t{i}"), json!({})).await;
    }
    let p = t.get_ok("/api/v1/events?since=0&limit=4").await;
    let seqs: Vec<u64> = p["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [1, 2, 3, 4]);
    assert_eq!(p["has_more"], true);
    assert_eq!(p["next_since"], 4);
    assert_eq!(p["head_seq"], 6);
    let p = t.get_ok("/api/v1/events?since=4").await;
    assert_eq!(p["events"].as_array().unwrap().len(), 2);
    assert_eq!(p["has_more"], false);
    assert_eq!(p["events"][0]["payload"]["op"], "created");
}

#[tokio::test]
async fn client_b_sees_client_a_command_within_a_second() {
    let t = TestServer::start().await;
    let list = t.create_list("Shared").await;
    let head = t.server.engine.head_seq();
    let mut a = t.sse(TOKEN_A, Some(head)).await;
    let mut b = t.sse(TOKEN_B, Some(head)).await;

    let task = id();
    let started = Instant::now();
    let (s, _, r) = t
        .command_with_id(
            TOKEN_A,
            &id(),
            json!({ "cmd": "create_task", "id": task, "list": list, "title": "From A" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let f = b
        .next_event(Duration::from_secs(1))
        .await
        .expect("client B got the event in under 1 s");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(f.event.as_deref(), Some("event"));
    assert_eq!(f.seq(), first_seq(&r));
    let ev = f.json();
    assert_eq!(ev["payload"]["entity"]["data"]["title"], "From A");
    assert_eq!(ev["actor"], json!({ "type": "owner", "device": "alpha" }));
    // A sees its own event too.
    assert_eq!(
        a.next_event(Duration::from_secs(1)).await.unwrap().seq(),
        f.seq()
    );

    // And the other way round.
    let (s, _, r) = t
        .command_with_id(
            TOKEN_B,
            &id(),
            json!({ "cmd": "complete_task", "id": task }),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let f = a.next_event(Duration::from_secs(1)).await.unwrap();
    assert_eq!(f.seq(), first_seq(&r));
    assert_eq!(f.json()["actor"]["device"], "beta");
}

#[tokio::test]
async fn sse_resumes_from_since_and_last_event_id() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    for i in 0..4 {
        t.create_task(&list, &format!("t{i}"), json!({})).await;
    }
    // Replay from 2: gets 3, 4, 5 then live 6.
    let mut s = t.sse(TOKEN_A, Some(2)).await;
    let mut seqs = Vec::new();
    for _ in 0..3 {
        seqs.push(s.next_event(Duration::from_secs(2)).await.unwrap().seq());
    }
    assert_eq!(seqs, [3, 4, 5]);
    t.create_task(&list, "live", json!({})).await;
    assert_eq!(s.next_event(Duration::from_secs(2)).await.unwrap().seq(), 6);
    drop(s);

    // Resume with Last-Event-ID (no `since`): only what came after 4.
    let res = t
        .client
        .get(format!("{}/api/v1/events", t.url))
        .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
        .header(ACCEPT, "text/event-stream")
        .header("last-event-id", "4")
        .send()
        .await
        .unwrap();
    let mut s = common::SseReader::new(res);
    assert_eq!(s.next_event(Duration::from_secs(2)).await.unwrap().seq(), 5);
    assert_eq!(s.next_event(Duration::from_secs(2)).await.unwrap().seq(), 6);
    assert!(s.next_event(Duration::from_millis(200)).await.is_none());
}

#[tokio::test]
async fn many_events_after_subscribing_arrive_in_order() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let mut s = t.sse(TOKEN_A, Some(0)).await;
    let n = 40;
    for i in 0..n {
        t.create_task(&list, &format!("t{i}"), json!({})).await;
    }
    for expected in 1..=(n + 1) {
        let f = s.next_event(Duration::from_secs(2)).await.unwrap();
        assert_eq!(f.seq(), expected);
    }
}

#[tokio::test]
async fn heartbeats_are_sent() {
    let t = TestServer::start_with(|c| c.limits.sse_heartbeat_ms = 100).await;
    let mut s = t.sse(TOKEN_A, None).await;
    let f = s.next_frame(Duration::from_secs(2)).await.unwrap();
    assert_eq!(f.comments, ["heartbeat"]);
    assert!(f.id.is_none());
}

#[tokio::test]
async fn concurrent_streams_are_capped() {
    let t = TestServer::start_with(|c| {
        c.limits.max_sse_connections = 1;
        // The retry loop below must not trip the connect rate limit.
        c.limits.sse_connect_burst = 1000;
    })
    .await;
    let first = t.sse(TOKEN_A, None).await;
    let res = t
        .client
        .get(format!("{}/api/v1/events", t.url))
        .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "too_many_streams");
    // JSON pages are not limited.
    t.get_ok("/api/v1/events").await;
    drop(first);
    // The slot is released once the server notices the client is gone.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let res = t
            .client
            .get(format!("{}/api/v1/events", t.url))
            .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
            .header(ACCEPT, "text/event-stream")
            .send()
            .await
            .unwrap();
        if res.status() == StatusCode::OK {
            break;
        }
        assert!(Instant::now() < deadline, "slot was never released");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn shutdown_ends_open_streams() {
    let t = TestServer::start().await;
    let mut s = t.sse(TOKEN_A, None).await;
    let stop = tokio::time::timeout(Duration::from_secs(5), t.stop()).await;
    assert!(
        stop.is_ok(),
        "graceful shutdown finished with a stream open"
    );
    assert!(s.next_event(Duration::from_millis(500)).await.is_none());
}

async fn open_sse(
    t: &TestServer,
    token: &str,
    query: &str,
    last_event_id: Option<&str>,
) -> reqwest::Response {
    let mut req = t
        .client
        .get(format!("{}/api/v1/events{query}", t.url))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(ACCEPT, "text/event-stream");
    if let Some(l) = last_event_id {
        req = req.header("last-event-id", l);
    }
    req.send().await.unwrap()
}

#[tokio::test]
async fn last_event_id_wins_over_since_on_reconnect() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    for i in 0..4 {
        t.create_task(&list, &format!("t{i}"), json!({})).await;
    }
    // A browser reconnects to the original URL (`since=2`) and adds Last-Event-ID: 4.
    let res = open_sse(&t, TOKEN_A, "?since=2", Some("4")).await;
    assert_eq!(res.status(), StatusCode::OK);
    let mut s = common::SseReader::new(res);
    assert_eq!(s.next_event(Duration::from_secs(2)).await.unwrap().seq(), 5);
    assert!(s.next_event(Duration::from_millis(200)).await.is_none());
}

/// Pages `GET /events?since=` until `head`, returning where it got to.
async fn page_up_to(t: &TestServer, mut since: u64, head: u64) -> u64 {
    while since < head {
        let p = t
            .get_ok(&format!("/api/v1/events?since={since}&limit=2"))
            .await;
        since = p["next_since"].as_u64().unwrap();
    }
    since
}

#[tokio::test]
async fn a_client_too_far_behind_gets_409_and_catches_up_through_json() {
    let t = TestServer::start_with(|c| c.limits.max_replay_events = 3).await;
    let list = t.create_list("L").await;
    for i in 0..4 {
        t.create_task(&list, &format!("t{i}"), json!({})).await;
    }
    // Refused before a stream opens: no slot, no replay.
    let res = open_sse(&t, TOKEN_A, "?since=0", None).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "catch_up_required");
    assert_eq!(v["error"]["details"]["since"], 0);
    assert_eq!(v["error"]["details"]["reason"], "too_far_behind");
    let head = v["error"]["details"]["head_seq"].as_u64().unwrap();
    assert_eq!(head, 5);
    // The protocol: page the JSON form up to the captured head, then stream from there.
    let at = page_up_to(&t, 0, head).await;
    assert_eq!(at, head);
    let mut s = t.sse(TOKEN_A, Some(at)).await;
    t.create_task(&list, "live", json!({})).await;
    assert_eq!(s.next_event(Duration::from_secs(2)).await.unwrap().seq(), 6);
    // Within the budget, replay works.
    let mut s = t.sse(TOKEN_A, Some(3)).await;
    for want in 4..=6 {
        assert_eq!(
            s.next_event(Duration::from_secs(2)).await.unwrap().seq(),
            want
        );
    }
}

#[tokio::test]
async fn a_resume_point_ahead_of_the_head_is_400() {
    let t = TestServer::start().await;
    t.create_list("L").await;
    let res = open_sse(&t, TOKEN_A, "?since=99", None).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "invalid_since");
    assert_eq!(v["error"]["details"]["head_seq"], 1);
    let res = open_sse(&t, TOKEN_A, "", Some("99")).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let (s, v) = t.get("/api/v1/events?since=99").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "invalid_since");
    // At the head is fine.
    t.get_ok("/api/v1/events?since=1").await;
}

#[tokio::test]
async fn replay_is_bounded_by_bytes_too() {
    let t = TestServer::start_with(|c| c.limits.max_replay_bytes = 2000).await;
    let list = t.create_list("L").await;
    let notes = "n".repeat(700);
    for i in 0..4 {
        t.create_task(&list, &format!("t{i}"), json!({ "notes": notes }))
            .await;
    }
    let mut s = common::SseReader::new(open_sse(&t, TOKEN_A, "?since=0", None).await);
    let mut delivered = Vec::new();
    let notice = loop {
        let f = s.next_event_or_named(Duration::from_secs(2)).await.unwrap();
        if f.event.as_deref() == Some("catch_up") {
            break f.json();
        }
        delivered.push(f.seq());
    };
    assert!(
        !delivered.is_empty() && delivered.len() < 5,
        "{delivered:?}"
    );
    let at = *delivered.last().unwrap();
    assert_eq!(notice["since"], at);
    assert_eq!(notice["reason"], "too_many_bytes");
    assert!(f_is_last(&mut s).await, "the stream ends after the notice");
    // What EventSource does next: reconnect to the same point. That is refused at once (and a 409
    // stops EventSource), instead of replaying the same budget again.
    for _ in 0..3 {
        let res = open_sse(&t, TOKEN_A, "?since=0", Some(&at.to_string())).await;
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let v: serde_json::Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "catch_up_required");
        assert_eq!(v["error"]["details"]["reason"], "catch_up_sent");
    }
    // Another stream of the same principal, from the head, may connect; that does not clear the mark.
    let head = notice["head_seq"].as_u64().unwrap();
    let res = open_sse(&t, TOKEN_A, &format!("?since={head}"), None).await;
    assert_eq!(res.status(), StatusCode::OK);
    drop(res);
    let res = open_sse(&t, TOKEN_A, "", Some(&at.to_string())).await;
    assert_eq!(
        res.status(),
        StatusCode::CONFLICT,
        "the later stream cleared the mark"
    );
    // After paging forward, a stream opens again.
    let at = page_up_to(&t, at, head).await;
    let res = open_sse(&t, TOKEN_A, "", Some(&at.to_string())).await;
    assert_eq!(res.status(), StatusCode::OK);
}

async fn f_is_last(s: &mut common::SseReader) -> bool {
    s.next_frame(Duration::from_secs(2)).await.is_none()
}

#[tokio::test]
async fn stream_reconnects_are_rate_limited_per_principal() {
    let t = TestServer::start_with(|c| {
        c.limits.sse_connect_burst = 2;
        c.limits.sse_connects_per_minute = 1;
    })
    .await;
    for _ in 0..2 {
        assert_eq!(
            open_sse(&t, TOKEN_A, "", None).await.status(),
            StatusCode::OK
        );
    }
    let res = open_sse(&t, TOKEN_A, "", None).await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = res.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry));
    let v: serde_json::Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "rate_limited");
    // Another principal has its own budget.
    assert_eq!(
        open_sse(&t, TOKEN_B, "", None).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_stuck_client_is_dropped_and_its_slot_freed() {
    use tokio::io::AsyncWriteExt;
    let t = TestServer::start_with(|c| {
        c.limits.max_sse_connections = 1;
        c.limits.sse_stall_timeout_ms = 300;
        c.limits.sse_connect_burst = 1000;
    })
    .await;
    let list = t.create_list("L").await;
    // A client that opens a stream and never reads from it.
    let mut stuck = tokio::net::TcpStream::connect(t.server.addr).await.unwrap();
    stuck
        .write_all(
            format!(
                "GET /api/v1/events HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN_A}\r\n\
                 Accept: text/event-stream\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        open_sse(&t, TOKEN_A, "", None).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    // Large events fill the socket and the per-client buffer; the stalled send then gives up.
    let notes = "x".repeat(90_000);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        for _ in 0..10 {
            t.create_task(&list, "big", json!({ "notes": notes })).await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        let head = t.server.engine.head_seq().to_string();
        if open_sse(&t, TOKEN_A, "", Some(&head)).await.status() == StatusCode::OK {
            break;
        }
        assert!(Instant::now() < deadline, "the stuck stream kept its slot");
    }
    drop(stuck);
}
