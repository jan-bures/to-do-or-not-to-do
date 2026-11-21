#![allow(clippy::unwrap_used)]
//! HTTP integration tests: commands, idempotency, undo and redo, reads, queries, auth and limits.

mod common;

use common::{TOKEN_A, TOKEN_B, TestServer, first_seq, id};
use reqwest::header::{ACCEPT, AUTHORIZATION, WWW_AUTHENTICATE};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[tokio::test]
async fn health_is_a_bare_200_without_auth() {
    let t = TestServer::start().await;
    let res = t
        .client
        .get(format!("{}/health", t.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.text().await.unwrap().is_empty());
}

#[tokio::test]
async fn create_update_complete_task() {
    let t = TestServer::start().await;
    let list = t.create_list("Groceries").await;
    let task = id();
    let r = t
        .ok(json!({ "cmd": "create_task", "id": task, "list": list, "title": "Milk" }))
        .await;
    assert_eq!(r["events"].as_array().unwrap().len(), 1);
    assert_eq!(r["summary"], "Created task \"Milk\" in list \"Groceries\"");

    let r = t
        .ok(json!({ "cmd": "update_task", "id": task,
                    "patch": { "title": "Oat milk", "due": "2026-12-01", "priority": "high" } }))
        .await;
    assert!(
        r["summary"]
            .as_str()
            .unwrap()
            .starts_with("Updated task \"Oat milk\"")
    );

    let r = t.ok(json!({ "cmd": "complete_task", "id": task })).await;
    assert_eq!(r["summary"], "Completed task \"Oat milk\"");

    let d = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(d["task"]["title"], "Oat milk");
    assert_eq!(d["task"]["due"], "2026-12-01");
    assert_eq!(d["task"]["priority"], "high");
    assert!(d["task"]["completed_at"].is_string());
    assert_eq!(
        d["task"]["created"]["by"],
        json!({ "type": "owner", "device": "alpha" })
    );

    let tasks = t.get_ok(&format!("/api/v1/lists/{list}/tasks")).await;
    assert_eq!(tasks["items"].as_array().unwrap().len(), 1);
    assert!(tasks["next_cursor"].is_null());
    let open = t
        .get_ok(&format!("/api/v1/lists/{list}/tasks?done=false"))
        .await;
    assert!(open["items"].as_array().unwrap().is_empty());

    let lists = t.get_ok("/api/v1/lists").await;
    assert_eq!(lists["items"][0]["name"], "Groceries");
}

#[tokio::test]
async fn task_detail_includes_steps_links_and_tags() {
    let t = TestServer::start().await;
    let list = t.create_list("Work").await;
    let task = t.create_task(&list, "Report", json!({})).await;
    let tag = id();
    t.ok(json!({ "cmd": "create_tag", "id": tag, "name": "urgent" }))
        .await;
    t.ok(json!({ "cmd": "add_tag", "task": task, "tag": tag }))
        .await;
    let step = id();
    t.ok(json!({ "cmd": "create_step", "id": step, "task": task, "title": "Draft" }))
        .await;
    t.ok(json!({ "cmd": "add_link", "id": id(), "task": task,
                 "kind": { "type": "web", "url": "https://example.com/spec", "title": null } }))
        .await;
    let d = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(d["steps"][0]["title"], "Draft");
    assert_eq!(d["links"][0]["kind"]["url"], "https://example.com/spec");
    assert_eq!(d["tags"][0]["name"], "urgent");
    assert_eq!(t.get_ok("/api/v1/tags").await["items"][0]["name"], "urgent");
    assert_eq!(
        t.get_ok("/api/v1/collections").await,
        json!({ "items": [], "next_cursor": null })
    );

    let (s, v) = t.get(&format!("/api/v1/tasks/{}", id())).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "not_found");
    let (s, v) = t.get("/api/v1/tasks/not-a-uuid").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn retrying_a_command_id_returns_the_original_result() {
    let t = TestServer::start().await;
    let list = t.create_list("Inbox").await;
    let cid = id();
    let cmd = json!({ "cmd": "create_task", "id": id(), "list": list, "title": "Once" });
    let (s1, h1, r1) = t.command_with_id(TOKEN_A, &cid, cmd.clone()).await;
    assert_eq!(s1, StatusCode::OK);
    assert!(h1.get("idempotent-replayed").is_none());
    let (s2, h2, r2) = t.command_with_id(TOKEN_A, &cid, cmd.clone()).await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(h2.get("idempotent-replayed").unwrap(), "true");
    assert_eq!(r1, r2);

    let tasks = t.get_ok(&format!("/api/v1/lists/{list}/tasks")).await;
    assert_eq!(
        tasks["items"].as_array().unwrap().len(),
        1,
        "no duplicate task"
    );
    let page = t.get_ok("/api/v1/events").await;
    assert_eq!(
        page["events"].as_array().unwrap().len(),
        2,
        "no duplicate event"
    );

    // Same ID, different request: refused.
    let other = json!({ "cmd": "create_task", "id": id(), "list": list, "title": "Other" });
    let (s3, _, r3) = t.command_with_id(TOKEN_A, &cid, other).await;
    assert_eq!(s3, StatusCode::CONFLICT);
    assert_eq!(r3["error"]["code"], "command_id_reused");
    // Same ID and command from another device (another actor): refused too.
    let (s4, _, r4) = t.command_with_id(TOKEN_B, &cid, cmd).await;
    assert_eq!(s4, StatusCode::CONFLICT);
    assert_eq!(r4["error"]["code"], "command_id_reused");
}

#[tokio::test]
async fn validation_errors_are_typed_4xx() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let cases = [
        (
            json!({ "cmd": "create_task", "id": id(), "list": list, "title": "   " }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
        ),
        (
            json!({ "cmd": "create_task", "id": id(), "list": id(), "title": "x" }),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            json!({ "cmd": "create_list", "id": list, "name": "dup" }),
            StatusCode::CONFLICT,
            "already_exists",
        ),
        (
            json!({ "cmd": "no_such_command" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_body",
        ),
    ];
    for (cmd, status, code) in cases {
        let (s, v) = t.command(cmd.clone()).await;
        assert_eq!(s, status, "{cmd}: {v}");
        assert_eq!(v["error"]["code"], code, "{cmd}: {v}");
        assert!(v["error"]["message"].is_string());
    }
    let (s, v) = t
        .command(json!({ "cmd": "create_task", "id": id(), "list": list, "title": "" }))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["error"]["details"]["field"], "title");

    // Malformed JSON.
    let res = t
        .client
        .post(format!("{}/api/v1/commands", t.url))
        .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn undo_that_would_break_an_invariant_is_a_typed_422_even_with_force() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let task = t.create_task(&list, "water plants", json!({})).await;
    let set_due = t
        .ok(json!({ "cmd": "update_task", "id": task, "patch": { "due": "2026-03-02" } }))
        .await;
    t.ok(json!({ "cmd": "update_task", "id": task,
                 "patch": { "recurrence": { "rule": { "freq": "daily", "interval": 1 } } } }))
        .await;
    for body in [None, Some(json!({ "force": true }))] {
        let (s, v) = t.undo(first_seq(&set_due), body).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        assert_eq!(v["error"]["code"], "undo_invariant");
        assert_eq!(v["error"]["details"]["reason"], "invalid");
        assert_eq!(
            v["error"]["details"]["reason_details"]["field"],
            "recurrence"
        );
    }
    let d = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(d["task"]["due"], "2026-03-02");
}

#[tokio::test]
async fn undo_conflict_force_and_redo() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let task = t.create_task(&list, "A", json!({})).await;
    let title = |v: &Value| v["task"]["title"].as_str().unwrap().to_owned();

    let e1 = t
        .ok(json!({ "cmd": "update_task", "id": task, "patch": { "title": "B" } }))
        .await;
    let e1 = first_seq(&e1);
    let e2 = t
        .ok(json!({ "cmd": "update_task", "id": task, "patch": { "title": "C" } }))
        .await;
    let e2 = first_seq(&e2);

    // Undoing e1 would overwrite e2's title: 409 with details.
    let (s, v) = t.undo(e1, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "undo_conflict");
    assert_eq!(v["error"]["details"]["event"]["seq"], e1);
    let c = &v["error"]["details"]["conflicts"][0];
    assert_eq!(c["field"], "title");
    assert_eq!(c["later_events"][0]["seq"], e2);

    // Undo the latest change instead: title back to B.
    let (s, u2) = t.undo(e2, None).await;
    assert_eq!(s, StatusCode::OK, "{u2}");
    assert!(u2["summary"].as_str().unwrap().starts_with("Undo: "));
    assert_eq!(
        title(&t.get_ok(&format!("/api/v1/tasks/{task}")).await),
        "B"
    );

    // Undoing the same event again is refused.
    let (s, v) = t.undo(e2, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["error"]["code"], "already_undone");

    // Redo = undo the compensating event.
    let (s, _) = t.undo(first_seq(&u2), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        title(&t.get_ok(&format!("/api/v1/tasks/{task}")).await),
        "C"
    );

    // Force the conflicting undo of e1: title back to A.
    let (s, v) = t.undo(e1, Some(json!({ "force": true }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        title(&t.get_ok(&format!("/api/v1/tasks/{task}")).await),
        "A"
    );
    let page = t.get_ok("/api/v1/activity?limit=1").await;
    assert_eq!(page["items"][0]["event"]["forced"], true);

    // Unknown seq.
    let (s, v) = t.undo(9_999, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "event_not_found");
}

#[tokio::test]
async fn undo_with_command_id_is_idempotent() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let task = t.create_task(&list, "A", json!({})).await;
    let r = t.ok(json!({ "cmd": "complete_task", "id": task })).await;
    let cid = id();
    let (s1, u1) = t
        .undo(first_seq(&r), Some(json!({ "command_id": cid })))
        .await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, u2) = t
        .undo(first_seq(&r), Some(json!({ "command_id": cid })))
        .await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(u1, u2);
    let d = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    assert!(d["task"]["completed_at"].is_null());
}

#[tokio::test]
async fn undo_of_a_creation_and_activity_feed() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let task = id();
    let r = t
        .ok(json!({ "cmd": "create_task", "id": task, "list": list, "title": "Temp" }))
        .await;
    let (s, _) = t.undo(first_seq(&r), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = t.get(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let page = t.get_ok("/api/v1/activity").await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["summary"], "Undo: Withdrew task \"Temp\"");
    assert_eq!(items[0]["can_undo"], true);
    assert_eq!(items[1]["summary"], "Created task \"Temp\"");
    assert_eq!(items[1]["undone"], true);
    assert_eq!(items[1]["can_undo"], false);
    assert!(page["next_before"].is_null());

    let p1 = t.get_ok("/api/v1/activity?limit=2").await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 2);
    let before = p1["next_before"].as_u64().unwrap();
    let p2 = t
        .get_ok(&format!("/api/v1/activity?limit=2&before={before}"))
        .await;
    assert_eq!(p2["items"][0]["summary"], "Created list \"L\"");
}

#[tokio::test]
async fn queries_and_smart_lists() {
    let t = TestServer::start().await;
    let list = t.create_list("Home").await;
    let other = t.create_list("Work").await;
    t.create_task(
        &list,
        "Buy mléko",
        json!({ "priority": "high", "due": "2026-01-10" }),
    )
    .await;
    t.create_task(&list, "Water plants", json!({ "due": "2026-01-05" }))
        .await;
    let done = t
        .create_task(&other, "Ship release", json!({ "priority": "high" }))
        .await;
    t.ok(json!({ "cmd": "complete_task", "id": done })).await;

    let titles = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["title"].as_str().unwrap().to_owned())
            .collect()
    };
    let q = |s: &'static str| t.query_tasks(s);
    let (s, v) = q("priority>=high and not done").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(titles(&v), ["Buy mléko"]);
    let (_, v) = q("list:home sort:due").await;
    assert_eq!(titles(&v), ["Water plants", "Buy mléko"]);
    let (_, v) = q("mleko").await;
    assert_eq!(titles(&v), ["Buy mléko"]);
    let (_, v) = q("done").await;
    assert_eq!(titles(&v), ["Ship release"]);
    let (_, v) = q("").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 3);
    let (s, v) = q("due<=(").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["error"]["code"], "invalid_query");
    assert!(v["error"]["details"]["pos"].is_u64());

    let lists = t.get_ok("/api/v1/smart-lists").await;
    let ids: Vec<&str> = lists
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"important") && ids.contains(&"my_day"));
    let v = t.get_ok("/api/v1/smart-lists/important/tasks").await;
    assert_eq!(titles(&v), ["Buy mléko"]);
    let v = t.get_ok("/api/v1/smart-lists/completed/tasks").await;
    assert_eq!(titles(&v), ["Ship release"]);
    let (s, _) = t.get("/api/v1/smart-lists/nope/tasks").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// Every route except `/health`, with a request that would otherwise succeed or fail validation.
fn protected_routes() -> Vec<(Method, String, Option<Value>)> {
    let u = id();
    vec![
        (Method::GET, "/api/v1/openapi.json".into(), None),
        (
            Method::POST,
            "/api/v1/commands".into(),
            Some(
                json!({ "command_id": id(), "command": { "cmd": "create_list", "id": id(), "name": "x" } }),
            ),
        ),
        (Method::GET, "/api/v1/events".into(), None),
        (Method::POST, "/api/v1/events/1/undo".into(), None),
        (Method::GET, "/api/v1/activity".into(), None),
        (Method::GET, "/api/v1/collections".into(), None),
        (Method::GET, "/api/v1/lists".into(), None),
        (Method::GET, format!("/api/v1/lists/{u}/tasks"), None),
        (Method::GET, "/api/v1/tasks?query=done".into(), None),
        (Method::GET, format!("/api/v1/tasks/{u}"), None),
        (Method::GET, "/api/v1/tags".into(), None),
        (Method::GET, "/api/v1/smart-lists".into(), None),
        (Method::GET, "/api/v1/smart-lists/my_day/tasks".into(), None),
        (Method::GET, "/api/v1/no-such-route".into(), None),
    ]
}

#[tokio::test]
async fn every_non_health_route_requires_a_token() {
    let t = TestServer::start().await;
    for (method, path, body) in protected_routes() {
        for token in [None, Some("wrong-token")] {
            let (s, h, v) = t.request(method.clone(), &path, token, body.clone()).await;
            assert_eq!(
                s,
                StatusCode::UNAUTHORIZED,
                "{method} {path} with {token:?}"
            );
            assert_eq!(h.get(WWW_AUTHENTICATE).unwrap(), "Bearer");
            assert_eq!(v["error"]["code"], "unauthorized");
        }
    }
    // The SSE variant of the event endpoint too.
    let res = t
        .client
        .get(format!("{}/api/v1/events", t.url))
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    // Nothing was written by the rejected requests.
    assert_eq!(t.server.engine.head_seq(), 0);
}

#[tokio::test]
async fn protected_route_list_covers_the_openapi_document() {
    let t = TestServer::start().await;
    let doc = t.get_ok("/api/v1/openapi.json").await;
    assert_eq!(
        doc["components"]["securitySchemes"]["bearer"]["scheme"],
        "bearer"
    );
    let mut documented = BTreeSet::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for method in item.as_object().unwrap().keys() {
            documented.insert((method.to_uppercase(), path.clone()));
        }
    }
    let tested: BTreeSet<(String, String)> = protected_routes()
        .into_iter()
        .map(|(m, p, _)| {
            let p = p.split('?').next().unwrap().to_owned();
            // Map concrete paths back to their templates.
            let p = regex_lite_replace(&p);
            (m.to_string(), p)
        })
        .collect();
    let public: BTreeSet<(String, String)> =
        todo_server::app::public_routes().into_iter().collect();
    for d in documented.iter().filter(|d| !public.contains(*d)) {
        assert!(
            tested.contains(d),
            "route {d:?} is not covered by protected_routes()"
        );
    }
    for name in ["Command", "Event", "Task", "CommandResult", "ErrorBody"] {
        assert!(
            doc["components"]["schemas"].get(name).is_some(),
            "schema {name} missing"
        );
    }
}

/// Replaces UUIDs and numeric segments with the OpenAPI path templates used by the router.
fn regex_lite_replace(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    let mut out = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let prev = if i > 0 { segs[i - 1] } else { "" };
        let templ = match (
            prev,
            s.parse::<uuid::Uuid>().is_ok(),
            s.parse::<u64>().is_ok(),
        ) {
            (_, true, _) => "{id}",
            ("events", _, true) => "{seq}",
            ("smart-lists", _, _) => "{id}",
            _ => s,
        };
        out.push(templ);
    }
    out.join("/")
}

/// Sends only the request head with a large `Content-Length` and reads the response. The server must
/// answer 413 without waiting for (or reading) the body.
async fn raw_oversized(t: &TestServer, method: &str, path: &str, len: usize) -> (u16, Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sock = tokio::net::TcpStream::connect(t.server.addr).await.unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN_A}\r\n\
         Content-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
    );
    sock.write_all(head.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sock.read_to_end(&mut buf),
    )
    .await
    .unwrap()
    .unwrap();
    let text = String::from_utf8_lossy(&buf);
    let status: u16 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let body = serde_json::from_str(body.trim()).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn oversized_bodies_get_413() {
    let t = TestServer::start().await;
    assert_eq!(t.config.limits.max_body_bytes, 1024 * 1024);
    let (s, v) = raw_oversized(&t, "POST", "/api/v1/commands", 1024 * 1024 + 1).await;
    assert_eq!(s, 413);
    assert_eq!(v["error"]["code"], "payload_too_large");
    // Routes that do not read a body are limited too.
    let (s, _) = raw_oversized(&t, "GET", "/api/v1/lists", 2 * 1024 * 1024).await;
    assert_eq!(s, 413);
    // Bodies under the limit work.
    let list = t.create_list("L").await;
    let task = t.create_task(&list, "T", json!({})).await;
    t.ok(json!({ "cmd": "add_note", "task": task, "text": "x".repeat(60_000) }))
        .await;
}

#[tokio::test]
async fn body_limit_is_configurable_and_applies_to_json_commands() {
    let t = TestServer::start_with(|c| c.limits.max_body_bytes = 4096).await;
    let list = t.create_list("L").await;
    let task = t.create_task(&list, "T", json!({})).await;
    let (s, v) = t
        .command(json!({ "cmd": "add_note", "task": task, "text": "x".repeat(5000) }))
        .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(v["error"]["code"], "payload_too_large");
    t.ok(json!({ "cmd": "add_note", "task": task, "text": "x".repeat(1000) }))
        .await;
}

#[tokio::test]
async fn data_survives_a_restart() {
    let t = TestServer::start().await;
    let list = t.create_list("Kept").await;
    t.create_task(&list, "Persisted", json!({})).await;
    let before = t.get_ok(&format!("/api/v1/lists/{list}/tasks")).await;
    let head = t.server.engine.head_seq();
    let t = t.restart().await;
    assert_eq!(t.server.engine.head_seq(), head);
    assert_eq!(
        t.get_ok(&format!("/api/v1/lists/{list}/tasks")).await,
        before
    );
    // Commands keep working and seqs continue.
    let r = t
        .ok(json!({ "cmd": "update_list", "id": list, "name": "Kept 2" }))
        .await;
    assert_eq!(first_seq(&r), head + 1);
}

#[tokio::test]
async fn reusing_a_withdrawn_id_is_409_but_a_retry_of_the_create_replays() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let task = id();
    let command_id = id();
    let create = json!({ "cmd": "create_task", "id": task, "list": list, "title": "once" });
    let (s, _, first) = t
        .command_with_id(TOKEN_A, &command_id, create.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{first}");
    // Undoing the creation withdraws the task; its ID stays used.
    let (s, v) = t.undo(first_seq(&first), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = t.command(create.clone()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "already_exists");
    // A retry of the original request still returns the stored result.
    let (s, h, again) = t.command_with_id(TOKEN_A, &command_id, create).await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(h.get("idempotent-replayed").unwrap(), "true");
    assert_eq!(again, first);
}

#[tokio::test]
async fn undoing_a_delete_whose_position_was_taken_stores_a_rekey_in_the_same_group() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let x = t.create_task(&list, "x", json!({})).await;
    let x_before = t.get_ok(&format!("/api/v1/tasks/{x}")).await["task"].clone();
    let del = t
        .ok(json!({ "cmd": "delete", "id": { "kind": "task", "id": x } }))
        .await;
    let after_delete = t.get_ok(&format!("/api/v1/tasks/{x}")).await["task"].clone();
    // y is appended where x was, taking its position key.
    let y = t.create_task(&list, "y", json!({})).await;
    let y_task = t.get_ok(&format!("/api/v1/tasks/{y}")).await["task"].clone();
    assert_eq!(y_task["position"], x_before["position"]);

    let head = t.server.engine.head_seq();
    let (s, undo) = t.undo(first_seq(&del), None).await;
    assert_eq!(s, StatusCode::OK, "{undo}");
    let events = undo["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "the restore and a rekey: {undo}");
    assert_eq!(t.server.engine.head_seq(), head + 2);
    // Both are stored as one group under one command.
    let page = t.get_ok(&format!("/api/v1/events?since={head}")).await;
    let stored = page["events"].as_array().unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0]["group"], stored[1]["group"]);
    assert_eq!(stored[0]["group"], stored[0]["id"]);
    let titles: Vec<String> = t.get_ok(&format!("/api/v1/lists/{list}/tasks")).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(titles, ["x", "y"]);
    let x_restored = t.get_ok(&format!("/api/v1/tasks/{x}")).await["task"].clone();
    assert_ne!(x_restored["position"], y_task["position"]);

    // Undoing the undo (any of its events) re-deletes x with its old key, as a unit.
    let (s, redo) = t.undo(events[1]["seq"].as_u64().unwrap(), None).await;
    assert_eq!(s, StatusCode::OK, "{redo}");
    let x_again = t.get_ok(&format!("/api/v1/tasks/{x}")).await["task"].clone();
    assert_eq!(x_again["position"], after_delete["position"]);
    assert_eq!(x_again["deleted"], after_delete["deleted"]);
    assert!(!x_again["deleted"].is_null());
    let titles: Vec<Value> = t.get_ok(&format!("/api/v1/lists/{list}/tasks")).await["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].clone())
        .collect();
    assert_eq!(titles, [json!("y")]);
    assert_eq!(
        t.get_ok(&format!("/api/v1/tasks/{y}")).await["task"],
        y_task
    );
}

#[tokio::test]
async fn undo_requires_a_command_id() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let task = t.create_task(&list, "A", json!({})).await;
    let r = t.ok(json!({ "cmd": "complete_task", "id": task })).await;
    let head = t.server.engine.head_seq();
    for body in [None, Some(json!({})), Some(json!({ "force": true }))] {
        let (s, v) = t.undo_raw(first_seq(&r), body.clone()).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body:?}: {v}");
        assert_eq!(v["error"]["code"], "command_id_required");
    }
    assert_eq!(t.server.engine.head_seq(), head, "nothing was undone");
    let (s, v) = t.undo(first_seq(&r), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}
