#![allow(clippy::unwrap_used)]
//! Resource limits: bounded admission, the request deadline (and what it means for writes),
//! pagination, query caps, exclusive database ownership and failed commits.

mod common;

use common::{TOKEN_A, TestServer, id};
use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use todo_server::db::StoreError;
use todo_server::engine::Engine;
use todo_server::{Options, Server, ServerError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn post_command(
    client: reqwest::Client,
    url: String,
    command: Value,
) -> (StatusCode, Option<String>, Value) {
    let res = client
        .post(format!("{url}/api/v1/commands"))
        .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
        .json(&json!({ "command_id": id(), "command": command }))
        .send()
        .await
        .unwrap();
    let status = res.status();
    let retry = res
        .headers()
        .get("retry-after")
        .map(|v| v.to_str().unwrap().to_owned());
    (status, retry, res.json().await.unwrap())
}

#[tokio::test]
async fn writes_beyond_the_admission_limit_get_503_with_retry_after() {
    let t = TestServer::start_with(|c| c.limits.max_concurrent_writes = 1).await;
    let list = t.create_list("L").await;
    let engine = t.server.engine.clone();
    let hold = engine.hold_writer();
    // The first write is admitted and waits for the (held) writer.
    let first = tokio::spawn(post_command(
        t.client.clone(),
        t.url.clone(),
        json!({ "cmd": "create_task", "id": id(), "list": list, "title": "first" }),
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (s, retry, v) = post_command(
        t.client.clone(),
        t.url.clone(),
        json!({ "cmd": "create_task", "id": id(), "list": list, "title": "second" }),
    )
    .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    assert_eq!(v["error"]["code"], "overloaded");
    assert_eq!(retry.as_deref(), Some("1"));
    // Reads have their own pool.
    t.get_ok("/api/v1/lists").await;
    drop(hold);
    let (s, _, v) = first.await.unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    // The permit is back.
    t.ok(json!({ "cmd": "create_task", "id": id(), "list": list, "title": "third" }))
        .await;
}

/// Sends a request head and the start of a body, then stalls. Returns the status line.
async fn stalled_body(t: &TestServer, extra_headers: &str) -> String {
    let mut sock = tokio::net::TcpStream::connect(t.server.addr).await.unwrap();
    let head = format!(
        "POST /api/v1/commands HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN_A}\r\n\
         Content-Type: application/json\r\nContent-Length: 200\r\n{extra_headers}\r\n{{\"command_id\""
    );
    sock.write_all(head.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf))
        .await
        .expect("the server answered before the test gave up")
        .unwrap();
    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
    text.lines().next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn a_stalled_body_hits_the_request_deadline() {
    let t = TestServer::start_with(|c| c.limits.request_timeout_ms = 300).await;
    let started = Instant::now();
    assert!(stalled_body(&t, "").await.starts_with("HTTP/1.1 408"));
    assert!(started.elapsed() < Duration::from_secs(3));
    // Claiming to want an event stream does not escape the deadline.
    assert!(
        stalled_body(&t, "Accept: text/event-stream\r\n")
            .await
            .starts_with("HTTP/1.1 408")
    );
}

#[tokio::test]
async fn event_streams_outlive_the_request_deadline() {
    let t = TestServer::start_with(|c| {
        c.limits.request_timeout_ms = 200;
        c.limits.sse_heartbeat_ms = 100;
    })
    .await;
    let mut s = t.sse(TOKEN_A, None).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let list = t.create_list("L").await;
    let f = s.next_event(Duration::from_secs(2)).await.unwrap();
    assert_eq!(f.json()["payload"]["entity"]["data"]["id"], list);
}

#[tokio::test]
async fn a_timed_out_command_is_not_stored_and_a_retry_runs_it_exactly_once() {
    let t = TestServer::start_with(|c| c.limits.request_timeout_ms = 300).await;
    let list = t.create_list("L").await;
    let engine = t.server.engine.clone();
    let head = engine.head_seq();
    let command_id = id();
    let task = id();
    let cmd = json!({ "cmd": "create_task", "id": task, "list": list, "title": "late" });
    {
        let _hold = engine.hold_writer();
        let (s, _, v) = t.command_with_id(TOKEN_A, &command_id, cmd.clone()).await;
        assert_eq!(s, StatusCode::REQUEST_TIMEOUT, "{v}");
        assert_eq!(v["error"]["code"], "timeout");
    }
    // The abandoned write now gets the writer, sees it was cancelled, and stores nothing.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(engine.head_seq(), head);
    let (s, _) = t.get(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // A retry with the same command ID runs it, once.
    let (s, h, _) = t.command_with_id(TOKEN_A, &command_id, cmd.clone()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h.get("idempotent-replayed").is_none());
    let (s, h, _) = t.command_with_id(TOKEN_A, &command_id, cmd).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.get("idempotent-replayed").unwrap(), "true");
    assert_eq!(engine.head_seq(), head + 1);
}

#[tokio::test]
async fn a_commit_in_flight_at_the_deadline_is_awaited_not_reported_as_a_timeout() {
    let t = TestServer::start_with(|c| c.limits.request_timeout_ms = 300).await;
    let list = t.create_list("L").await;
    let engine = t.server.engine.clone();
    let head = engine.head_seq();
    // The write claims the commit at once and then takes longer than the deadline to finish.
    engine.set_commit_delay(Duration::from_millis(450));
    let task = id();
    let (s, _, v) = t
        .command_with_id(
            TOKEN_A,
            &id(),
            json!({ "cmd": "create_task", "id": task, "list": list, "title": "slow" }),
        )
        .await;
    engine.set_commit_delay(Duration::ZERO);
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(engine.head_seq(), head + 1);
    t.get_ok(&format!("/api/v1/tasks/{task}")).await;
}

#[tokio::test]
async fn a_commit_that_outlives_the_grace_period_is_outcome_unknown_and_a_retry_replays_it() {
    let t = TestServer::start_with(|c| c.limits.request_timeout_ms = 300).await;
    let list = t.create_list("L").await;
    let engine = t.server.engine.clone();
    let head = engine.head_seq();
    engine.set_commit_delay(Duration::from_millis(1000));
    let command_id = id();
    let cmd = json!({ "cmd": "create_task", "id": id(), "list": list, "title": "slower" });
    let (s, h, v) = t.command_with_id(TOKEN_A, &command_id, cmd.clone()).await;
    engine.set_commit_delay(Duration::ZERO);
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    assert_eq!(v["error"]["code"], "outcome_unknown");
    assert_eq!(h.get("retry-after").unwrap(), "1");
    // The commit lands; a retry with the same command ID returns it instead of running it again.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(engine.head_seq(), head + 1);
    let (s, h, _) = t.command_with_id(TOKEN_A, &command_id, cmd).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.get("idempotent-replayed").unwrap(), "true");
    assert_eq!(engine.head_seq(), head + 1);
}

// ---- pagination ---------------------------------------------------------------------------------------

/// Follows `next_cursor` from `path` (which must already have a `?`), returning every page's titles.
async fn walk(t: &TestServer, path: &str, field: &str) -> Vec<Vec<String>> {
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let url = match &cursor {
            Some(c) => format!("{path}&cursor={c}"),
            None => path.to_owned(),
        };
        let p = t.get_ok(&url).await;
        pages.push(
            p["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x[field].as_str().unwrap().to_owned())
                .collect(),
        );
        match p["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => return pages,
        }
        assert!(pages.len() < 50, "pagination does not terminate");
    }
}

#[tokio::test]
async fn collection_reads_and_queries_are_paginated() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    for i in 0..7 {
        t.create_task(&list, &format!("t{i}"), json!({})).await;
    }
    let pages = walk(&t, &format!("/api/v1/lists/{list}/tasks?limit=3"), "title").await;
    assert_eq!(
        pages,
        [vec!["t0", "t1", "t2"], vec!["t3", "t4", "t5"], vec!["t6"]]
    );

    let pages = walk(&t, "/api/v1/tasks?query=sort:-title&limit=4", "title").await;
    assert_eq!(
        pages,
        [vec!["t6", "t5", "t4", "t3"], vec!["t2", "t1", "t0"]]
    );
    let pages = walk(&t, "/api/v1/smart-lists/all/tasks?limit=5", "title").await;
    assert_eq!(pages.iter().map(Vec::len).collect::<Vec<_>>(), [5, 2]);

    for name in ["b", "a", "c"] {
        t.ok(json!({ "cmd": "create_tag", "id": id(), "name": name }))
            .await;
    }
    let pages = walk(&t, "/api/v1/tags?limit=2", "name").await;
    assert_eq!(pages, [vec!["a", "b"], vec!["c"]]);
    for name in ["L2", "L3"] {
        t.create_list(name).await;
    }
    let pages = walk(&t, "/api/v1/lists?limit=2", "name").await;
    assert_eq!(pages, [vec!["L", "L2"], vec!["L3"]]);
    for name in ["C1", "C2"] {
        t.ok(json!({ "cmd": "create_collection", "id": id(), "name": name }))
            .await;
    }
    let pages = walk(&t, "/api/v1/collections?limit=1", "name").await;
    assert_eq!(pages, [vec!["C1"], vec!["C2"]]);

    // A huge limit is clamped, not refused.
    let p = t
        .get_ok(&format!("/api/v1/lists/{list}/tasks?limit=1000000"))
        .await;
    assert_eq!(p["items"].as_array().unwrap().len(), 7);
}

#[tokio::test]
async fn a_cursor_survives_its_item_changing_and_bad_cursors_are_400() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let mut ids = Vec::new();
    for i in 0..4 {
        ids.push(t.create_task(&list, &format!("t{i}"), json!({})).await);
    }
    let p = t.get_ok("/api/v1/tasks?query=not done&limit=2").await;
    let cursor = p["next_cursor"].as_str().unwrap().to_owned();
    // The cursor's task no longer matches the query; the next page still starts after it.
    t.ok(json!({ "cmd": "complete_task", "id": ids[1] })).await;
    let p = t
        .get_ok(&format!(
            "/api/v1/tasks?query=not done&limit=2&cursor={cursor}"
        ))
        .await;
    let titles: Vec<&str> = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["t2", "t3"]);

    for bad in ["nope", &id()] {
        let (s, v) = t
            .get(&format!("/api/v1/lists/{list}/tasks?cursor={bad}"))
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        assert_eq!(v["error"]["code"], "invalid_cursor");
    }
}

#[tokio::test]
async fn cursors_are_bound_to_their_route_and_filters() {
    let t = TestServer::start().await;
    let (a, b) = (t.create_list("A").await, t.create_list("B").await);
    for i in 0..3 {
        t.create_task(&a, &format!("a{i}"), json!({})).await;
        t.create_task(&b, &format!("b{i}"), json!({})).await;
    }
    let cursor = |p: &Value| p["next_cursor"].as_str().unwrap().to_owned();
    let of_a = cursor(&t.get_ok(&format!("/api/v1/lists/{a}/tasks?limit=1")).await);
    let of_query = cursor(&t.get_ok("/api/v1/tasks?query=not done&limit=1").await);
    let of_lists = cursor(&t.get_ok("/api/v1/lists?limit=1").await);
    // Each works where it came from.
    let p = t
        .get_ok(&format!("/api/v1/lists/{a}/tasks?limit=1&cursor={of_a}"))
        .await;
    assert_eq!(p["items"][0]["title"], "a1");
    for wrong in [
        format!("/api/v1/lists/{b}/tasks?cursor={of_a}"),
        format!("/api/v1/lists/{a}/tasks?done=false&cursor={of_a}"),
        format!("/api/v1/lists/{a}/tasks?include_deleted=true&cursor={of_a}"),
        format!("/api/v1/tasks?query=done&cursor={of_query}"),
        format!("/api/v1/smart-lists/all/tasks?cursor={of_query}"),
        format!("/api/v1/tags?cursor={of_lists}"),
        format!("/api/v1/collections?cursor={of_lists}"),
        format!("/api/v1/lists/{a}/tasks?cursor={of_lists}"),
    ] {
        let (s, v) = t.get(&wrong).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{wrong}: {v}");
        assert_eq!(v["error"]["code"], "invalid_cursor");
    }
    // The same query written differently normalizes to the same binding.
    let p = t
        .get_ok(&format!(
            "/api/v1/tasks?query=not%20%20done&limit=1&cursor={of_query}"
        ))
        .await;
    assert_eq!(p["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn every_emitted_cursor_is_accepted_even_for_the_largest_keys() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    // Longest titles, in 4-byte characters, so every title key is as large as it gets.
    let title = |i: usize| format!("{i}{}", "\u{1F600}".repeat(499));
    for i in 0..3 {
        t.create_task(&list, &title(i), json!({})).await;
    }
    // Many sort clauses, most of them repeats, within the query caps.
    let mut clauses = ["sort:title", "sort:-title"].repeat(20);
    clauses.extend([
        "sort:due",
        "sort:-priority",
        "sort:created",
        "sort:updated",
        "sort:completed",
    ]);
    let text = clauses.join(" ");
    let (s, v) = t.query_tasks(&text).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let encoded: String = text.replace(' ', "%20");
    let mut cursor: Option<String> = None;
    let mut seen = Vec::new();
    loop {
        let path = match &cursor {
            Some(c) => format!("/api/v1/tasks?query={encoded}&limit=1&cursor={c}"),
            None => format!("/api/v1/tasks?query={encoded}&limit=1"),
        };
        let (s, p) = t.get(&path).await;
        assert_eq!(
            s,
            StatusCode::OK,
            "a cursor this query emitted was refused: {p}"
        );
        seen.push(p["items"][0]["title"].as_str().unwrap().to_owned());
        match p["next_cursor"].as_str() {
            Some(c) => {
                assert!(c.len() <= 8 * 1024, "cursor of {} bytes", c.len());
                cursor = Some(c.to_owned());
            }
            None => break,
        }
    }
    assert_eq!(seen, [title(0), title(1), title(2)]);
}

#[tokio::test]
async fn a_cursor_keeps_its_position_when_its_item_is_deleted() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let mut ids = Vec::new();
    for i in 0..4 {
        ids.push(t.create_task(&list, &format!("t{i}"), json!({})).await);
    }
    let p = t
        .get_ok(&format!("/api/v1/lists/{list}/tasks?limit=2"))
        .await;
    let cursor = p["next_cursor"].as_str().unwrap().to_owned();
    t.ok(json!({ "cmd": "delete", "id": { "kind": "task", "id": ids[1] } }))
        .await;
    let p = t
        .get_ok(&format!(
            "/api/v1/lists/{list}/tasks?limit=2&cursor={cursor}"
        ))
        .await;
    let titles: Vec<&str> = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["t2", "t3"]);
    assert!(p["next_cursor"].is_null());
}

#[tokio::test]
async fn task_detail_children_are_capped_and_flagged() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let task = t.create_task(&list, "parent", json!({})).await;
    let detail = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(detail["truncated"], false);
    let max = todo_server::api::read::MAX_DETAIL_CHILDREN;
    for i in 0..=max {
        t.ok(
            json!({ "cmd": "create_step", "id": id(), "task": task, "title": format!("s{i:03}") }),
        )
        .await;
    }
    let detail = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(detail["truncated"], true);
    let steps = detail["steps"].as_array().unwrap();
    assert_eq!(steps.len(), max);
    assert_eq!(steps[0]["title"], "s000");
    assert_eq!(steps[max - 1]["title"], format!("s{:03}", max - 1));
}

// ---- query caps ----------------------------------------------------------------------------------------

#[tokio::test]
async fn overly_complex_queries_get_a_typed_400() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    t.create_task(&list, "w31", json!({})).await;

    let terms: Vec<String> = (0..33).map(|i| format!("w{i}")).collect();
    let (s, v) = t.query_tasks(&terms.join(" or ")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["code"], "query_too_complex");
    assert_eq!(v["error"]["details"]["limit"], "terms");
    assert_eq!(v["error"]["details"]["max"], 32);

    let nots: Vec<String> = (0..32).map(|i| format!("not w{i}")).collect();
    let (s, v) = t.query_tasks(&nots.join(" ")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["details"]["limit"], "nodes");

    let (s, v) = t.query_tasks(&"x".repeat(1025)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["details"]["limit"], "bytes");

    // At the limits, the query runs.
    let (s, v) = t.query_tasks(&terms[..32].join(" or ")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["items"][0]["title"], "w31");
}

// ---- exclusive ownership ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_server_on_the_same_database_fails_fast() {
    let t = TestServer::start().await;
    t.create_list("L").await;
    let started = Instant::now();
    let err = Server::bind(&t.config, Options::default())
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        matches!(err, ServerError::Store(StoreError::Locked { .. })),
        "{err}"
    );
    assert!(
        err.to_string()
            .contains("in use by another todo-server process"),
        "{err}"
    );
    // `rebuild-projections` takes the same lock.
    let err = Engine::rebuild_projections(&t.db_path()).unwrap_err();
    assert!(matches!(err, StoreError::Locked { .. }), "{err}");
    // The first server is unaffected, and releases the lock when it stops.
    t.create_list("L2").await;
    let path = t.db_path();
    let (_dir, _config) = t.stop().await;
    Engine::rebuild_projections(&path).unwrap();
}

#[tokio::test]
async fn the_lock_follows_the_file_not_the_spelling_of_its_path() {
    let t = TestServer::start().await;
    let path = t.db_path();
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let aliased = dir.join("sub").join("..").join(path.file_name().unwrap());
    assert_eq!(
        todo_server::db::lock_path(&aliased).unwrap(),
        todo_server::db::lock_path(&path).unwrap()
    );
    // Another spelling of the same file is refused while the server holds it.
    let err = Engine::rebuild_projections(&aliased).unwrap_err();
    assert!(matches!(err, StoreError::Locked { .. }), "{err}");
    // A database that does not exist yet still gets a canonical lock path (from its directory).
    let fresh = dir.join("sub").join("..").join("new.sqlite3");
    assert_eq!(
        todo_server::db::lock_path(&fresh).unwrap(),
        todo_server::db::lock_path(&dir.join("new.sqlite3")).unwrap()
    );
}

// ---- failed commits ------------------------------------------------------------------------------------

#[tokio::test]
async fn a_failed_commit_leaves_no_trace_in_memory_or_on_disk() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let head = t.server.engine.head_seq();
    let before = t.server.engine.dump_projections().unwrap();
    // Fault injection: the idempotency record (written after the events were applied) fails.
    let conn = rusqlite::Connection::open(t.db_path()).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER inject_fault BEFORE INSERT ON commands
         BEGIN SELECT RAISE(ABORT, 'injected fault'); END;",
    )
    .unwrap();
    let task = id();
    let cmd = json!({ "cmd": "create_task", "id": task, "list": list, "title": "doomed" });
    let (s, v) = t.command(cmd.clone()).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{v}");
    assert_eq!(v["error"]["code"], "internal");
    // Nothing is visible: not in memory, not in the log, not in the projections.
    let (s, _) = t.get(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, v) = t.query_tasks("doomed").await;
    assert_eq!(v["items"], json!([]));
    assert_eq!(t.server.engine.head_seq(), head);
    let page = t.get_ok(&format!("/api/v1/events?since={head}")).await;
    assert_eq!(page["events"], json!([]));
    assert_eq!(t.server.engine.dump_projections().unwrap(), before);
    // Once the fault is gone, the same command works and the projections still match a fold.
    conn.execute_batch("DROP TRIGGER inject_fault").unwrap();
    let r = t.ok(cmd).await;
    assert_eq!(common::first_seq(&r), head + 1);
    let path = t.db_path();
    let (_dir, _config) = t.stop().await;
    let reader = todo_server::db::open_reader(&path).unwrap();
    let folded = todo_server::db::fold_log(&reader).unwrap();
    assert!(
        todo_server::db::load_projections(&reader)
            .unwrap()
            .data_eq(&folded)
    );
}
