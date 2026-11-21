//! The MCP server at `/mcp`, end to end: `rmcp`'s streamable HTTP client against an in-process server.
//! Agent tokens are real ones, created and revoked through the auth service.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::{TOKEN_A, TestServer, first_seq, id};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, ORIGIN, WWW_AUTHENTICATE};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use std::time::Duration;
use todo_core::ids::ListId;
use todo_core::{AccessLevel, AgentId, Grants};
use todo_server::Config;

type Client = RunningService<RoleClient, ()>;

struct Env {
    t: TestServer,
}

impl Env {
    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    async fn start_with(f: impl FnOnce(&mut Config)) -> Self {
        Self {
            t: TestServer::start_with(f).await,
        }
    }

    /// A real agent token with `grants`; returns `(token row id, token)`.
    fn agent(&self, name: &str, grants: &Grants, writes_per_hour: Option<u32>) -> (i64, String) {
        let svc = &self.t.server.authsvc;
        let (row, token) = svc
            .create_agent(
                &AgentId::new(name).unwrap(),
                AccessLevel::None,
                writes_per_hour,
                None,
                None,
            )
            .unwrap();
        assert!(svc.set_grants(row, grants, None).unwrap());
        (row, token)
    }

    async fn connect(&self, token: &str) -> Client {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", self.t.url))
                .auth_header(token),
        );
        ().serve(transport).await.unwrap()
    }

    /// A raw JSON-RPC POST to `/mcp` (no client, so status codes and headers are visible).
    async fn post(&self, token: Option<&str>, body: &Value) -> reqwest::Response {
        let mut req = self
            .t
            .client
            .post(format!("{}/mcp", self.t.url))
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string());
        if let Some(t) = token {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        req.send().await.unwrap()
    }
}

fn args(v: Value) -> rmcp::model::JsonObject {
    match v {
        Value::Object(m) => m,
        _ => panic!("arguments must be an object"),
    }
}

async fn call(c: &Client, tool: &str, a: Value) -> CallToolResult {
    c.call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args(a)))
        .await
        .unwrap()
}

/// The structured result of a call that must succeed.
async fn ok(c: &Client, tool: &str, a: Value) -> Value {
    let r = call(c, tool, a.clone()).await;
    let v = r.structured_content.clone().unwrap_or(Value::Null);
    assert_ne!(r.is_error, Some(true), "{tool} {a}: {v}");
    v
}

/// The error code of a call that must fail as a tool error.
async fn fails(c: &Client, tool: &str, a: Value) -> Value {
    let r = call(c, tool, a.clone()).await;
    assert_eq!(r.is_error, Some(true), "{tool} {a} succeeded");
    let v = r.structured_content.unwrap();
    // The text content carries the same envelope, for clients that only show text.
    v["error"].clone()
}

fn titles(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap().to_owned())
        .collect()
}

fn names(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

fn list_id(s: &str) -> ListId {
    serde_json::from_value(json!(s)).unwrap()
}

fn grants(default: AccessLevel, lists: &[(&str, AccessLevel)]) -> Grants {
    let mut g = Grants::with_default(default);
    for (l, level) in lists {
        g.lists.insert(list_id(l), *level);
    }
    g
}

const ALL_TOOLS: [&str; 16] = [
    "add_note",
    "add_step",
    "archive_task",
    "complete_task",
    "create_task",
    "get_activity",
    "get_task",
    "list_collections",
    "list_lists",
    "merge_tasks",
    "move_task",
    "query_tasks",
    "set_my_day",
    "undo_event",
    "update_step",
    "update_task",
];

#[tokio::test]
async fn an_agent_initializes_lists_tools_and_captures_into_the_inbox() {
    let env = Env::start().await;
    let inbox = env.t.create_list("Inbox").await;
    let errands = env.t.create_list("Errands").await;
    let (_, token) = env.agent("scout", &Grants::with_default(AccessLevel::Capture), None);
    let c = env.connect(&token).await;

    let info = c.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "todo");
    assert!(info.instructions.as_deref().unwrap().contains("Inbox"));
    let mut tools: Vec<String> = c
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    tools.sort();
    assert_eq!(tools, ALL_TOOLS);

    let lists = ok(&c, "list_lists", json!({})).await;
    assert_eq!(names(&lists), ["Inbox", "Errands"]);
    assert!(lists["next_cursor"].is_null());
    let one = ok(&c, "list_lists", json!({ "limit": 1 })).await;
    assert_eq!(names(&one), ["Inbox"]);
    let rest = ok(&c, "list_lists", json!({ "cursor": one["next_cursor"] })).await;
    assert_eq!(names(&rest), ["Errands"]);
    assert_eq!(
        ok(&c, "list_collections", json!({})).await["items"],
        json!([])
    );

    // Without a list, a task goes to the Inbox.
    let created = ok(
        &c,
        "create_task",
        json!({ "title": "Buy milk", "notes": "2 l", "due": "2030-01-02", "priority": "high" }),
    )
    .await;
    assert_eq!(created["list_id"], inbox);
    assert_eq!(created["replayed"], false);
    assert!(created["summary"].as_str().unwrap().contains("Buy milk"));
    let events = created["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0]["seq"].as_u64().is_some() && events[0]["id"].is_string());
    let task = created["task_id"].as_str().unwrap().to_owned();
    let elsewhere = ok(
        &c,
        "create_task",
        json!({ "title": "Post office", "list_id": errands }),
    )
    .await;
    assert_eq!(elsewhere["list_id"], errands);

    let found = ok(&c, "query_tasks", json!({ "query": "milk" })).await;
    assert_eq!(titles(&found), ["Buy milk"]);
    let found = ok(&c, "query_tasks", json!({ "query": "list:errands" })).await;
    assert_eq!(titles(&found), ["Post office"]);
    let bad = fails(&c, "query_tasks", json!({ "query": "due<<" })).await;
    assert_eq!(bad["code"], "invalid_query");

    let detail = ok(&c, "get_task", json!({ "task_id": task })).await;
    assert_eq!(detail["task"]["title"], "Buy milk");
    assert_eq!(detail["task"]["priority"], "high");
    assert_eq!(detail["task"]["due"], "2030-01-02");
    let note = ok(
        &c,
        "add_note",
        json!({ "task_id": task, "text": "semi-skimmed" }),
    )
    .await;
    assert_eq!(note["events"].as_array().unwrap().len(), 1);
    let detail = ok(&c, "get_task", json!({ "task_id": task })).await;
    assert!(
        detail["task"]["notes"]
            .as_str()
            .unwrap()
            .contains("semi-skimmed")
    );

    // The writes are attributed to the agent.
    let activity = ok(&c, "get_activity", json!({ "limit": 3 })).await;
    let items = activity["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(item["event"]["actor"]["agent_id"], "scout", "{item}");
    }

    // An owner device token works too.
    let owner = env.connect(TOKEN_A).await;
    let lists = ok(&owner, "list_lists", json!({})).await;
    assert_eq!(names(&lists).len(), 2);
    c.cancel().await.unwrap();
    owner.cancel().await.unwrap();
}

#[tokio::test]
async fn a_plan_level_agent_organises_tasks() {
    let env = Env::start().await;
    let inbox = env.t.create_list("Inbox").await;
    let someday = env.t.create_list("Someday").await;
    let (_, token) = env.agent("planner", &Grants::with_default(AccessLevel::Plan), None);
    let c = env.connect(&token).await;

    let a = ok(&c, "create_task", json!({ "title": "Write report" })).await["task_id"].clone();
    let b = ok(&c, "create_task", json!({ "title": "write the report" })).await["task_id"].clone();

    ok(
        &c,
        "update_task",
        json!({ "task_id": a, "title": "Write Q3 report", "due": "2030-03-01", "priority": "normal" }),
    )
    .await;
    let empty = fails(&c, "update_task", json!({ "task_id": a })).await;
    assert_eq!(empty["code"], "invalid_body");
    ok(&c, "update_task", json!({ "task_id": a, "due": null })).await;

    let step = ok(&c, "add_step", json!({ "task_id": a, "title": "Outline" })).await;
    let step_id = step["step_id"].clone();
    let both = ok(
        &c,
        "update_step",
        json!({ "step_id": step_id, "title": "Outline sections", "completed": true }),
    )
    .await;
    assert_eq!(both["events"].as_array().unwrap().len(), 2);

    ok(&c, "set_my_day", json!({ "task_id": a })).await;
    let detail = ok(&c, "get_task", json!({ "task_id": a })).await;
    assert_eq!(detail["task"]["title"], "Write Q3 report");
    assert!(detail["task"]["due"].is_null());
    assert!(detail["task"]["my_day"].is_string());
    assert_eq!(detail["steps"][0]["title"], "Outline sections");
    assert!(detail["steps"][0]["completed_at"].is_string());
    let my_day = ok(&c, "query_tasks", json!({ "query": "my_day" })).await;
    assert_eq!(titles(&my_day), ["Write Q3 report"]);

    // Merge the duplicate into it; the duplicate goes away.
    ok(
        &c,
        "merge_tasks",
        json!({ "source_task_id": b, "target_task_id": a }),
    )
    .await;
    let left = ok(&c, "query_tasks", json!({ "query": "report" })).await;
    assert_eq!(titles(&left), ["Write Q3 report"]);

    ok(
        &c,
        "move_task",
        json!({ "task_id": a, "list_id": someday, "position": "first" }),
    )
    .await;
    let detail = ok(&c, "get_task", json!({ "task_id": a })).await;
    assert_eq!(detail["task"]["list"], someday);

    ok(&c, "complete_task", json!({ "task_id": a })).await;
    let done = ok(&c, "query_tasks", json!({ "query": "done" })).await;
    assert_eq!(titles(&done), ["Write Q3 report"]);
    ok(
        &c,
        "complete_task",
        json!({ "task_id": a, "completed": false }),
    )
    .await;
    ok(
        &c,
        "set_my_day",
        json!({ "task_id": a, "in_my_day": false }),
    )
    .await;
    let my_day = ok(&c, "query_tasks", json!({ "query": "my_day" })).await;
    assert!(titles(&my_day).is_empty());

    let archived = ok(&c, "archive_task", json!({ "task_id": a })).await;
    let all = ok(&c, "query_tasks", json!({})).await;
    assert!(titles(&all).is_empty(), "{all}");
    let kept = ok(
        &c,
        "get_task",
        json!({ "task_id": a, "include_deleted": true }),
    )
    .await;
    assert!(kept["task"]["deleted"].is_object(), "{kept}");
    // Archiving can be undone.
    ok(
        &c,
        "undo_event",
        json!({ "seq": archived["events"][0]["seq"] }),
    )
    .await;
    let all = ok(&c, "query_tasks", json!({})).await;
    assert_eq!(titles(&all), ["Write Q3 report"]);
    let _ = inbox;
}

#[tokio::test]
async fn a_capture_agent_cannot_plan_or_see_outside_its_lists() {
    let env = Env::start().await;
    let inbox = env.t.create_list("Inbox").await;
    let hidden = env.t.create_list("Diary").await;
    let open_task = env.t.create_task(&inbox, "Open task", json!({})).await;
    let secret = env.t.create_task(&hidden, "Secret task", json!({})).await;
    env.t
        .ok(json!({ "cmd": "add_note", "task": secret, "text": "private" }))
        .await;
    let (_, token) = env.agent(
        "scout",
        &grants(AccessLevel::None, &[(&inbox, AccessLevel::Capture)]),
        None,
    );
    let c = env.connect(&token).await;

    // Planning tools are refused with the REST code and details.
    for (tool, a) in [
        ("update_task", json!({ "task_id": open_task, "title": "x" })),
        ("complete_task", json!({ "task_id": open_task })),
        ("set_my_day", json!({ "task_id": open_task })),
        ("archive_task", json!({ "task_id": open_task })),
        ("add_step", json!({ "task_id": open_task, "title": "s" })),
    ] {
        let e = fails(&c, tool, a).await;
        assert_eq!(e["code"], "forbidden", "{tool}: {e}");
        assert_eq!(e["details"]["needed"], "plan", "{tool}: {e}");
        assert_eq!(e["details"]["have"], "capture", "{tool}: {e}");
    }
    let mine = ok(&c, "create_task", json!({ "title": "Mine" })).await["task_id"].clone();
    let e = fails(
        &c,
        "merge_tasks",
        json!({ "source_task_id": mine, "target_task_id": open_task }),
    )
    .await;
    assert_eq!(e["code"], "forbidden");

    // The hidden list does not exist for it.
    assert_eq!(names(&ok(&c, "list_lists", json!({})).await), ["Inbox"]);
    let all = ok(&c, "query_tasks", json!({})).await;
    assert_eq!(titles(&all), ["Open task", "Mine"]);
    assert!(titles(&ok(&c, "query_tasks", json!({ "query": "secret" })).await).is_empty());
    let e = fails(&c, "get_task", json!({ "task_id": secret })).await;
    assert_eq!(e["code"], "not_found");
    let e = fails(&c, "add_note", json!({ "task_id": secret, "text": "hi" })).await;
    assert!(e["code"] == "forbidden" || e["code"] == "not_found", "{e}");
    let e = fails(
        &c,
        "create_task",
        json!({ "title": "x", "list_id": hidden }),
    )
    .await;
    assert_eq!(e["code"], "forbidden");
    let activity = ok(&c, "get_activity", json!({ "limit": 200 })).await;
    let text = activity.to_string();
    assert!(!text.contains(&secret), "{text}");
    assert!(!text.contains(&hidden), "{text}");
    assert!(!text.contains("Secret"), "{text}");
    assert!(text.contains(&open_task));
}

#[tokio::test]
async fn an_agent_undoes_only_its_own_events() {
    let env = Env::start().await;
    let inbox = env.t.create_list("Inbox").await;
    let (_, token) = env.agent("planner", &Grants::with_default(AccessLevel::Plan), None);
    let c = env.connect(&token).await;

    let created = ok(&c, "create_task", json!({ "title": "Oops" })).await;
    let seq = created["events"][0]["seq"].clone();
    let undone = ok(&c, "undo_event", json!({ "seq": seq })).await;
    assert_eq!(undone["events"].as_array().unwrap().len(), 1);
    assert!(titles(&ok(&c, "query_tasks", json!({})).await).is_empty());
    // Undoing the same event again is refused; undoing the undo redoes it.
    let e = fails(&c, "undo_event", json!({ "seq": seq })).await;
    assert_eq!(e["code"], "already_undone");
    ok(
        &c,
        "undo_event",
        json!({ "seq": undone["events"][0]["seq"] }),
    )
    .await;
    assert_eq!(titles(&ok(&c, "query_tasks", json!({})).await), ["Oops"]);

    // The owner's event is not the agent's to undo.
    let owner = env
        .t
        .ok(json!({ "cmd": "create_task", "id": id(), "list": inbox, "title": "Owner's" }))
        .await;
    let e = fails(&c, "undo_event", json!({ "seq": first_seq(&owner) })).await;
    assert_eq!(e["code"], "not_own_event", "{e}");
    let e = fails(&c, "undo_event", json!({ "seq": 9999 })).await;
    assert_eq!(e["code"], "event_not_found");
}

#[tokio::test]
async fn retrying_with_the_same_command_id_does_not_write_twice() {
    let env = Env::start().await;
    env.t.create_list("Inbox").await;
    let (_, token) = env.agent("scout", &Grants::with_default(AccessLevel::Plan), None);
    let c = env.connect(&token).await;

    let cid = id();
    let first = ok(
        &c,
        "create_task",
        json!({ "title": "Once", "command_id": cid }),
    )
    .await;
    let head = env.t.server.engine.head_seq();
    let again = ok(
        &c,
        "create_task",
        json!({ "title": "Once", "command_id": cid }),
    )
    .await;
    assert_eq!(again["replayed"], true);
    assert_eq!(again["task_id"], first["task_id"]);
    assert_eq!(again["events"], first["events"]);
    assert_eq!(again["command_id"], cid);
    assert_eq!(env.t.server.engine.head_seq(), head);
    assert_eq!(titles(&ok(&c, "query_tasks", json!({})).await), ["Once"]);
    // The same ID for a different write is refused.
    let e = fails(
        &c,
        "create_task",
        json!({ "title": "Other", "command_id": cid }),
    )
    .await;
    assert_eq!(e["code"], "command_id_reused");

    // Two-part writes replay both parts.
    let step = ok(
        &c,
        "add_step",
        json!({ "task_id": first["task_id"], "title": "s" }),
    )
    .await;
    let cid = id();
    let a =
        json!({ "step_id": step["step_id"], "title": "t", "completed": true, "command_id": cid });
    let one = ok(&c, "update_step", a.clone()).await;
    let head = env.t.server.engine.head_seq();
    let two = ok(&c, "update_step", a).await;
    assert_eq!(two["replayed"], true);
    assert_eq!(two["events"], one["events"]);
    assert_eq!(env.t.server.engine.head_seq(), head);

    // Without a command ID every call is a new write.
    ok(&c, "create_task", json!({ "title": "Twice" })).await;
    ok(&c, "create_task", json!({ "title": "Twice" })).await;
    assert_eq!(
        titles(&ok(&c, "query_tasks", json!({ "query": "twice" })).await).len(),
        2
    );
}

fn tools_call(tool: &str, a: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": a },
    })
}

#[tokio::test]
async fn missing_wrong_and_revoked_tokens_are_refused() {
    let env = Env::start().await;
    env.t.create_list("Inbox").await;
    let body = tools_call("list_lists", json!({}));
    for token in [None, Some("wrong-token"), Some("tda_not-a-real-token")] {
        let res = env.post(token, &body).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{token:?}");
        assert_eq!(res.headers().get(WWW_AUTHENTICATE).unwrap(), "Bearer");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "unauthorized");
    }
    let (row, token) = env.agent("scout", &Grants::with_default(AccessLevel::Read), None);
    let res = env.post(Some(&token), &body).await;
    assert_eq!(res.status(), StatusCode::OK);
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        v["result"]["structuredContent"]["items"][0]["name"],
        "Inbox"
    );

    // Browsers are turned away; only POST is served.
    let res = env
        .t
        .client
        .post(format!("{}/mcp", env.t.url))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(ACCEPT, "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, "https://evil.example.com")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = env
        .t
        .client
        .get(format!("{}/mcp", env.t.url))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    // A connected client is cut off as soon as its token is revoked.
    let c = env.connect(&token).await;
    ok(&c, "list_lists", json!({})).await;
    assert!(env.t.server.authsvc.revoke_agent(row, None).unwrap());
    let r = c
        .call_tool(CallToolRequestParams::new("list_lists").with_arguments(args(json!({}))))
        .await;
    assert!(r.is_err(), "{r:?}");
    let res = env.post(Some(&token), &body).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(env.t.server.engine.head_seq(), 1);
}

#[tokio::test]
async fn the_write_quota_applies_to_mcp_writes() {
    let env = Env::start().await;
    env.t.create_list("Inbox").await;
    let (_, token) = env.agent("scout", &Grants::with_default(AccessLevel::Plan), Some(2));
    let c = env.connect(&token).await;
    let first = ok(&c, "create_task", json!({ "title": "one" })).await;
    ok(&c, "create_task", json!({ "title": "two" })).await;
    let e = fails(&c, "create_task", json!({ "title": "three" })).await;
    assert_eq!(e["code"], "quota_exceeded");
    assert!(e["retry_after"].as_u64().unwrap() >= 1, "{e}");
    let e = fails(
        &c,
        "undo_event",
        json!({ "seq": first["events"][0]["seq"] }),
    )
    .await;
    assert_eq!(e["code"], "quota_exceeded");
    // Reads are not writes.
    assert_eq!(titles(&ok(&c, "query_tasks", json!({})).await).len(), 2);
}

#[tokio::test]
async fn a_token_has_a_bounded_number_of_requests_in_flight() {
    let env = Env::start_with(|c| c.limits.max_mcp_requests_per_token = 2).await;
    let inbox = env.t.create_list("Inbox").await;
    let (_, token) = env.agent("scout", &Grants::with_default(AccessLevel::Capture), None);
    let engine = env.t.server.engine.clone();
    let hold = engine.hold_writer();
    // Two writes are admitted and wait for the (held) writer.
    let mut waiting = Vec::new();
    for i in 0..2 {
        let (client, url, token) = (env.t.client.clone(), env.t.url.clone(), token.clone());
        let body = tools_call(
            "create_task",
            json!({ "title": format!("t{i}"), "list_id": inbox }),
        );
        waiting.push(tokio::spawn(async move {
            client
                .post(format!("{url}/mcp"))
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(ACCEPT, "application/json, text/event-stream")
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string())
                .send()
                .await
                .unwrap()
                .status()
        }));
    }
    // Let both writes reach the (held) writer before probing: a probe in flight at the same time
    // would take one of the two slots and turn a write away instead.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        waiting.iter().all(|w| !w.is_finished()),
        "a write finished while the writer was held"
    );
    let read = tools_call("list_lists", json!({}));
    let refused = env.post(Some(&token), &read).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers().get("retry-after").unwrap(), "1");
    let v: Value = refused.json().await.unwrap();
    assert_eq!(v["error"]["code"], "too_many_mcp_requests");
    // Other tokens are not affected.
    let res = env.post(Some(TOKEN_A), &read).await;
    assert_eq!(res.status(), StatusCode::OK);

    drop(hold);
    for w in waiting {
        assert_eq!(w.await.unwrap(), StatusCode::OK);
    }
    // The slots are free again.
    let res = env.post(Some(&token), &read).await;
    assert_eq!(res.status(), StatusCode::OK);
}
