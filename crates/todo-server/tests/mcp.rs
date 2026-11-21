//! The MCP server at `/mcp`, end to end: `rmcp`'s streamable HTTP client against an in-process server.
//! Agent tokens are real ones, created and revoked through the auth service.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::mcp::{ALL_TOOLS, Client, args, fails, grants, ok, same_shape};
use common::{TOKEN_A, TestServer, first_seq, id};
use reqwest::Method;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, ORIGIN, WWW_AUTHENTICATE};
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use todo_core::time::Timestamp;
use todo_core::{AccessLevel, Grants};
use todo_server::{Config, Options};

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

    /// A server whose clock runs `offset` milliseconds ahead of the real one.
    async fn start_with_clock(offset: Arc<AtomicI64>) -> Self {
        let options = Options {
            clock: Arc::new(move || {
                Timestamp::from_millisecond(
                    Timestamp::now().as_millisecond() + offset.load(Ordering::SeqCst),
                )
                .unwrap()
            }),
            ..Options::default()
        };
        Self {
            t: TestServer::start_with_options(|_| {}, options).await,
        }
    }

    /// A REST call with `token`: `(status, body)`.
    async fn rest(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (s, _, v) = self.t.request(method, path, Some(token), body).await;
        (s, v)
    }

    /// A real agent token with `grants`; returns `(token row id, token)`.
    fn agent(&self, name: &str, grants: &Grants, writes_per_hour: Option<u32>) -> (i64, String) {
        common::mcp::agent(&self.t, name, grants, writes_per_hour)
    }

    async fn connect(&self, token: &str) -> Client {
        common::mcp::connect(&self.t, token).await
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

/// `undo_event` arguments for the first event of a write result.
fn undo_args(result: &Value) -> Value {
    let e = &result["events"][0];
    json!({ "seq": e["seq"], "event_id": e["id"] })
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
    // One change per call: core has no command that retitles and completes at once.
    let head = env.t.server.engine.head_seq();
    let both = fails(
        &c,
        "update_step",
        json!({ "step_id": step_id, "title": "Outline sections", "completed": true }),
    )
    .await;
    assert_eq!(both["code"], "invalid_body");
    assert_eq!(env.t.server.engine.head_seq(), head, "nothing was written");
    ok(
        &c,
        "update_step",
        json!({ "step_id": step_id, "title": "Outline sections" }),
    )
    .await;
    ok(
        &c,
        "update_step",
        json!({ "step_id": step_id, "completed": true }),
    )
    .await;

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
    ok(&c, "undo_event", undo_args(&archived)).await;
    let all = ok(&c, "query_tasks", json!({})).await;
    assert_eq!(titles(&all), ["Write Q3 report"]);
    let _ = inbox;
}

#[tokio::test]
async fn a_plan_level_agent_tags_links_reminds_and_repeats() {
    // A zone without daylight saving time, so local reminder times convert the same all year.
    let env = Env::start_with(|c| c.time_zone = "Asia/Tokyo".into()).await;
    env.t.create_list("Inbox").await;
    let (_, token) = env.agent("planner", &Grants::with_default(AccessLevel::Plan), None);
    let c = env.connect(&token).await;
    // Every write names its event and says what it did.
    let wrote = |r: &Value| {
        assert_eq!(r["events"].as_array().unwrap().len(), 1, "{r}");
        assert!(!r["summary"].as_str().unwrap().is_empty(), "{r}");
    };

    // Tags: create, list, put on a task, find by, take off.
    let tag = ok(
        &c,
        "create_tag",
        json!({ "name": "work", "color": "#336699" }),
    )
    .await;
    wrote(&tag);
    let tag_id = tag["tag_id"].clone();
    let dup = fails(&c, "create_tag", json!({ "name": "Work" })).await;
    assert_eq!(dup["code"], "duplicate_name", "{dup}");
    let tags = ok(&c, "list_tags", json!({})).await;
    assert_eq!(names(&tags), ["work"]);
    assert_eq!(tags["items"][0]["id"], tag_id);
    let task = ok(&c, "create_task", json!({ "title": "Report" })).await["task_id"].clone();
    let tagged = ok(&c, "tag_task", json!({ "task_id": task, "tag_id": tag_id })).await;
    wrote(&tagged);
    let again = ok(&c, "tag_task", json!({ "task_id": task, "tag_id": tag_id })).await;
    assert!(again["events"].as_array().unwrap().is_empty(), "{again}");
    let detail = ok(&c, "get_task", json!({ "task_id": task })).await;
    assert_eq!(detail["tags"][0]["name"], "work");
    assert_eq!(
        titles(&ok(&c, "query_tasks", json!({ "query": "tag:work" })).await),
        ["Report"]
    );
    wrote(
        &ok(
            &c,
            "untag_task",
            json!({ "task_id": task, "tag_id": tag_id }),
        )
        .await,
    );
    assert!(titles(&ok(&c, "query_tasks", json!({ "query": "tag:work" })).await).is_empty());
    let e = fails(&c, "tag_task", json!({ "task_id": task, "tag_id": id() })).await;
    assert_eq!(e["code"], "not_found");

    // Links: web only, validated by the domain.
    let link = ok(
        &c,
        "add_link",
        json!({ "task_id": task, "url": "https://todo.example.com/spec", "title": "Spec" }),
    )
    .await;
    wrote(&link);
    let detail = ok(&c, "get_task", json!({ "task_id": task })).await;
    assert_eq!(detail["links"][0]["id"], link["link_id"]);
    assert_eq!(
        detail["links"][0]["kind"]["url"],
        "https://todo.example.com/spec"
    );
    assert_eq!(detail["links"][0]["kind"]["title"], "Spec");
    let e = fails(&c, "add_link", json!({ "task_id": task, "url": "ftp://x" })).await;
    assert_eq!(e["code"], "invalid", "{e}");

    // Reminders: an RFC 3339 time, or a local one in the owner's zone.
    let r = ok(
        &c,
        "add_reminder",
        json!({ "task_id": task, "at": "2030-01-02T09:00:00+01:00" }),
    )
    .await;
    wrote(&r);
    assert_eq!(r["at"], "2030-01-02T08:00:00Z");
    let reminder = r["reminder_id"].clone();
    let local = ok(
        &c,
        "add_reminder",
        json!({ "task_id": task, "at": "2030-01-03T09:00" }),
    )
    .await;
    assert_eq!(local["at"], "2030-01-03T00:00:00Z", "09:00 in Tokyo");
    let moved = ok(
        &c,
        "update_reminder",
        json!({ "reminder_id": reminder, "at": "2030-01-04T12:30" }),
    )
    .await;
    wrote(&moved);
    let detail = ok(&c, "get_task", json!({ "task_id": task })).await;
    let ats: Vec<&str> = detail["reminders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["at"].as_str().unwrap())
        .collect();
    assert_eq!(ats, ["2030-01-03T00:00:00Z", "2030-01-04T03:30:00Z"]);
    for bad in ["tomorrow", "2030-13-01T09:00", ""] {
        let e = fails(&c, "add_reminder", json!({ "task_id": task, "at": bad })).await;
        assert_eq!(e["code"], "invalid_body", "{bad}: {e}");
    }
    let e = fails(
        &c,
        "update_reminder",
        json!({ "reminder_id": id(), "at": "2030-01-04T12:30" }),
    )
    .await;
    assert_eq!(e["code"], "not_found");
    // A retry replays, with the same reminder.
    let cid = id();
    let first = ok(
        &c,
        "add_reminder",
        json!({ "task_id": task, "at": "2030-02-01T08:00", "command_id": cid }),
    )
    .await;
    let retry = ok(
        &c,
        "add_reminder",
        json!({ "task_id": task, "at": "2030-02-01T08:00", "command_id": cid }),
    )
    .await;
    assert_eq!(retry["replayed"], true);
    assert_eq!(retry["reminder_id"], first["reminder_id"]);
    assert_eq!(retry["events"], first["events"]);

    // Repeat rules: on create, changed and cleared by update, checked by the domain.
    let weekly =
        json!({ "rule": { "freq": "weekly", "days": ["mon", "thu"] }, "until": "2030-12-31" });
    let gym = ok(
        &c,
        "create_task",
        json!({ "title": "Gym", "due": "2030-01-07", "recurrence": weekly }),
    )
    .await["task_id"]
        .clone();
    let detail = ok(&c, "get_task", json!({ "task_id": gym })).await;
    assert_eq!(detail["task"]["recurrence"]["rule"]["freq"], "weekly");
    assert_eq!(detail["task"]["recurrence"]["rule"]["interval"], 1);
    assert_eq!(detail["task"]["recurrence"]["until"], "2030-12-31");
    // Completing it creates the next occurrence, on the next rule date after the due date.
    ok(&c, "complete_task", json!({ "task_id": gym })).await;
    let next = ok(&c, "query_tasks", json!({ "query": "gym and not done" })).await;
    assert_eq!(next["items"][0]["due"], "2030-01-10", "{next}");
    let next = next["items"][0]["id"].clone();
    for (bad, why) in [
        (
            json!({ "rule": { "freq": "weekly", "days": [] } }),
            "no weekdays",
        ),
        (
            json!({ "rule": { "freq": "daily", "interval": 0 } }),
            "interval 0",
        ),
        (
            json!({ "rule": { "freq": "monthly", "day": 32 } }),
            "day 32",
        ),
    ] {
        let e = fails(
            &c,
            "update_task",
            json!({ "task_id": next, "recurrence": bad }),
        )
        .await;
        assert_eq!(e["code"], "invalid_recurrence", "{why}: {e}");
    }
    let e = fails(
        &c,
        "update_task",
        json!({ "task_id": next, "recurrence": { "rule": { "freq": "daily" } }, "due": null }),
    )
    .await;
    assert_eq!(e["code"], "invalid", "a repeat rule needs a due date: {e}");
    let e = fails(
        &c,
        "create_task",
        json!({ "title": "x", "recurrence": { "rule": { "freq": "daily" } } }),
    )
    .await;
    assert_eq!(e["code"], "invalid", "{e}");
    let bad = common::mcp::call(
        &c,
        "update_task",
        json!({ "task_id": next, "recurrence": { "rule": { "freq": "weekly", "byday": ["mon"] } } }),
    )
    .await;
    // Arguments that don't fit the schema never reach the server's handlers.
    assert_eq!(bad.is_error, Some(true));
    assert!(
        format!("{:?}", bad.content).contains("unknown field `byday`"),
        "{bad:?}"
    );
    ok(
        &c,
        "update_task",
        json!({ "task_id": next, "recurrence": { "rule": { "freq": "monthly", "interval": 2, "day": 31 } } }),
    )
    .await;
    let detail = ok(&c, "get_task", json!({ "task_id": next })).await;
    assert_eq!(detail["task"]["recurrence"]["rule"]["day"], 31);
    ok(
        &c,
        "update_task",
        json!({ "task_id": next, "recurrence": null }),
    )
    .await;
    let detail = ok(&c, "get_task", json!({ "task_id": next })).await;
    assert!(detail["task"]["recurrence"].is_null(), "{detail}");
    // A task gets a rule and a due date in one call.
    ok(
        &c,
        "update_task",
        json!({ "task_id": task, "due": "2030-03-01", "recurrence": { "rule": { "freq": "yearly" } } }),
    )
    .await;
    let detail = ok(&c, "get_task", json!({ "task_id": task })).await;
    assert_eq!(detail["task"]["recurrence"]["rule"]["freq"], "yearly");
    c.cancel().await.unwrap();
}

#[tokio::test]
async fn read_and_capture_agents_cannot_tag_remind_or_repeat() {
    let env = Env::start().await;
    let inbox = env.t.create_list("Inbox").await;
    let task = env.t.create_task(&inbox, "Open task", json!({})).await;
    let tag = id();
    env.t
        .ok(json!({ "cmd": "create_tag", "id": tag, "name": "home" }))
        .await;
    env.t
        .ok(json!({ "cmd": "add_tag", "task": task, "tag": tag }))
        .await;
    let reminder = id();
    env.t
        .ok(json!({ "cmd": "add_reminder", "id": reminder, "task": task, "at": "2030-01-01T09:00:00Z" }))
        .await;
    let head = env.t.server.engine.head_seq();
    let plan_tools = [
        ("tag_task", json!({ "task_id": task, "tag_id": tag })),
        ("untag_task", json!({ "task_id": task, "tag_id": tag })),
        (
            "add_reminder",
            json!({ "task_id": task, "at": "2030-01-01T10:00:00Z" }),
        ),
        (
            "update_reminder",
            json!({ "reminder_id": reminder, "at": "2030-01-01T10:00:00Z" }),
        ),
        (
            "update_task",
            json!({ "task_id": task, "due": "2030-01-01", "recurrence": { "rule": { "freq": "daily" } } }),
        ),
        (
            "create_task",
            json!({ "title": "Daily", "list_id": inbox, "due": "2030-01-01", "recurrence": { "rule": { "freq": "daily" } } }),
        ),
        ("create_tag", json!({ "name": "agent-tag" })),
    ];
    for level in [AccessLevel::Read, AccessLevel::Capture] {
        let label = format!("{level:?}");
        let (_, token) = env.agent(&label.to_lowercase(), &Grants::with_default(level), None);
        let c = env.connect(&token).await;
        // Reading tags is fine.
        assert_eq!(names(&ok(&c, "list_tags", json!({})).await), ["home"]);
        for (tool, a) in &plan_tools {
            let e = fails(&c, tool, a.clone()).await;
            assert_eq!(e["code"], "forbidden", "{label} {tool}: {e}");
            // Creating a task needs capture first, and plan on top to make it repeat.
            let needed = if *tool == "create_task" && level == AccessLevel::Read {
                "capture"
            } else {
                "plan"
            };
            assert_eq!(e["details"]["needed"], needed, "{label} {tool}: {e}");
        }
        let link = json!({ "task_id": task, "url": "https://todo.example.com/" });
        if level == AccessLevel::Read {
            let e = fails(&c, "add_link", link).await;
            assert_eq!(e["code"], "forbidden", "{e}");
            assert_eq!(e["details"]["needed"], "capture", "{e}");
            assert_eq!(env.t.server.engine.head_seq(), head, "nothing was written");
        } else {
            ok(&c, "add_link", link).await;
        }
        c.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn creating_a_tag_needs_read_by_default_and_never_reveals_hidden_names() {
    let env = Env::start().await;
    let inbox = env.t.create_list("Inbox").await;
    let diary = env.t.create_list("Diary").await;
    let secret = env.t.create_task(&diary, "Secret", json!({})).await;
    let hidden_tag = id();
    env.t
        .ok(json!({ "cmd": "create_tag", "id": hidden_tag, "name": "therapy" }))
        .await;
    env.t
        .ok(json!({ "cmd": "add_tag", "task": secret, "tag": hidden_tag }))
        .await;
    let head = env.t.server.engine.head_seq();

    // Plan on the Inbox only, nothing by default: it can't see the tag, and can't learn its name.
    let (_, token) = env.agent(
        "scoped",
        &grants(AccessLevel::None, &[(&inbox, AccessLevel::Plan)]),
        None,
    );
    let c = env.connect(&token).await;
    assert!(names(&ok(&c, "list_tags", json!({})).await).is_empty());
    let taken = fails(&c, "create_tag", json!({ "name": "Therapy" })).await;
    let free = fails(&c, "create_tag", json!({ "name": "errands" })).await;
    assert_eq!(taken["code"], "forbidden", "{taken}");
    assert_eq!(taken["details"]["needed"], "read", "{taken}");
    assert_eq!(taken["details"]["have"], "none", "{taken}");
    assert_eq!(taken, free);
    let rest_tag = |name: &str| json!({ "command_id": id(), "command": { "cmd": "create_tag", "id": id(), "name": name } });
    let (hs, hb) = env
        .rest(
            &token,
            Method::POST,
            "/api/v1/commands",
            Some(rest_tag("Therapy")),
        )
        .await;
    let (ms, mb) = env
        .rest(
            &token,
            Method::POST,
            "/api/v1/commands",
            Some(rest_tag("errands")),
        )
        .await;
    assert_eq!((hs, ms), (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN));
    assert_eq!(hb, mb);
    assert_eq!(hb["error"]["details"]["needed"], "read", "{hb}");
    assert_eq!(env.t.server.engine.head_seq(), head, "nothing was written");
    c.cancel().await.unwrap();

    // With read by default it sees every tag, so a taken name is one it can see, and the tag it
    // creates is one it can use.
    let (_, token) = env.agent(
        "reader-planner",
        &grants(AccessLevel::Read, &[(&inbox, AccessLevel::Plan)]),
        None,
    );
    let c = env.connect(&token).await;
    assert_eq!(names(&ok(&c, "list_tags", json!({})).await), ["therapy"]);
    let dup = fails(&c, "create_tag", json!({ "name": "Therapy" })).await;
    assert_eq!(dup["code"], "duplicate_name", "{dup}");
    let tag = ok(&c, "create_tag", json!({ "name": "errands" })).await["tag_id"].clone();
    let task = ok(&c, "create_task", json!({ "title": "Post office" })).await["task_id"].clone();
    ok(&c, "tag_task", json!({ "task_id": task, "tag_id": tag })).await;
    assert_eq!(
        names(&ok(&c, "list_tags", json!({})).await),
        ["errands", "therapy"]
    );
    c.cancel().await.unwrap();

    // Renaming a tag is the owner's at any level, so no agent reaches the rename's name check either.
    let (_, token) = env.agent("planner", &Grants::with_default(AccessLevel::Plan), None);
    let rename = |name: &str| json!({ "command_id": id(), "command": { "cmd": "update_tag", "id": tag, "name": name } });
    let (ts, tb) = env
        .rest(
            &token,
            Method::POST,
            "/api/v1/commands",
            Some(rename("therapy")),
        )
        .await;
    let (fs, fb) = env
        .rest(
            &token,
            Method::POST,
            "/api/v1/commands",
            Some(rename("chores")),
        )
        .await;
    assert_eq!((ts, fs), (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN));
    assert_eq!(tb, fb);
    assert_eq!(tb["error"]["details"]["owner_only"], true, "{tb}");
}

#[tokio::test]
async fn a_plan_grant_that_reaches_no_list_creates_no_tag() {
    let env = Env::start().await;
    env.t.create_list("Inbox").await;
    let empty = id();
    env.t
        .ok(json!({ "cmd": "create_collection", "id": empty, "name": "Empty" }))
        .await;
    let full = id();
    env.t
        .ok(json!({ "cmd": "create_collection", "id": full, "name": "Full" }))
        .await;
    let mut lists = Vec::new();
    for name in ["A", "B"] {
        let l = id();
        env.t
            .ok(json!({ "cmd": "create_list", "id": l, "collection": full, "name": name }))
            .await;
        lists.push(l);
    }
    env.t
        .ok(json!({ "cmd": "create_tag", "id": id(), "name": "taken" }))
        .await;
    let head = env.t.server.engine.head_seq();
    let collection = |c: &str| serde_json::from_value(json!(c)).unwrap();
    let mut on_empty = Grants::with_default(AccessLevel::Read);
    on_empty
        .collections
        .insert(collection(&empty), AccessLevel::Plan);
    let mut overridden = grants(
        AccessLevel::Read,
        &[
            (&lists[0], AccessLevel::Read),
            (&lists[1], AccessLevel::Read),
        ],
    );
    overridden
        .collections
        .insert(collection(&full), AccessLevel::Plan);
    for (name, g) in [("on-empty", on_empty), ("overridden", overridden)] {
        let (_, token) = env.agent(name, &g, None);
        let c = env.connect(&token).await;
        let taken = fails(&c, "create_tag", json!({ "name": "Taken" })).await;
        let free = fails(&c, "create_tag", json!({ "name": "free" })).await;
        assert_eq!(taken["code"], "forbidden", "{name}: {taken}");
        assert_eq!(taken["details"]["needed"], "plan", "{name}: {taken}");
        assert_eq!(taken["details"]["have"], "read", "{name}: {taken}");
        assert_eq!(taken, free, "{name}");
        let body = |n: &str| json!({ "command_id": id(), "command": { "cmd": "create_tag", "id": id(), "name": n } });
        let (hs, hb) = env
            .rest(
                &token,
                Method::POST,
                "/api/v1/commands",
                Some(body("Taken")),
            )
            .await;
        let (ms, mb) = env
            .rest(&token, Method::POST, "/api/v1/commands", Some(body("free")))
            .await;
        assert_eq!(
            (hs, ms),
            (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN),
            "{name}"
        );
        assert_eq!(hb, mb, "{name}");
        c.cancel().await.unwrap();
    }
    assert_eq!(env.t.server.engine.head_seq(), head, "nothing was written");
}

/// Review finding: an agent that sees a tag only through one task, and takes it off that task, no
/// longer sees the tag. A retry of that write (its response was lost) then gets `not_found`, like any
/// write naming something the agent can't see, and never writes again. The task, still visible, shows
/// the write went through.
#[tokio::test]
async fn a_lost_untag_response_is_confirmed_through_the_task() {
    let env = Env::start().await;
    let inbox = env.t.create_list("Inbox").await;
    let task = env.t.create_task(&inbox, "Call the bank", json!({})).await;
    let tag = id();
    env.t
        .ok(json!({ "cmd": "create_tag", "id": tag, "name": "finance" }))
        .await;
    env.t
        .ok(json!({ "cmd": "add_tag", "task": task, "tag": tag }))
        .await;
    let (_, token) = env.agent(
        "scoped",
        &grants(AccessLevel::None, &[(&inbox, AccessLevel::Plan)]),
        None,
    );
    let c = env.connect(&token).await;
    assert_eq!(names(&ok(&c, "list_tags", json!({})).await), ["finance"]);

    let args = json!({ "task_id": task, "tag_id": tag, "command_id": id() });
    let first = ok(&c, "untag_task", args.clone()).await;
    assert_eq!(first["events"].as_array().unwrap().len(), 1);
    let head = env.t.server.engine.head_seq();
    // The response is lost; the retry can't be confirmed, because the tag is now hidden.
    assert!(names(&ok(&c, "list_tags", json!({})).await).is_empty());
    let retry = fails(&c, "untag_task", args).await;
    let missing = id();
    let m = fails(
        &c,
        "untag_task",
        json!({ "task_id": task, "tag_id": missing }),
    )
    .await;
    assert_eq!(retry["code"], "not_found", "{retry}");
    assert_eq!(
        same_shape(&retry, &tag.to_string()),
        same_shape(&m, &missing)
    );
    assert_eq!(
        env.t.server.engine.head_seq(),
        head,
        "the retry wrote nothing"
    );
    // The task says it went through.
    let detail = ok(&c, "get_task", json!({ "task_id": task })).await;
    assert_eq!(detail["tags"], json!([]), "{detail}");
    c.cancel().await.unwrap();
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
    // Writes on hidden items answer exactly like writes on missing ones.
    let e = fails(&c, "add_note", json!({ "task_id": secret, "text": "hi" })).await;
    let missing = id();
    let m = fails(&c, "add_note", json!({ "task_id": missing, "text": "hi" })).await;
    assert_eq!(e["code"], "not_found", "{e}");
    assert_eq!(same_shape(&e, &secret), same_shape(&m, &missing));
    let e = fails(
        &c,
        "create_task",
        json!({ "title": "x", "list_id": hidden }),
    )
    .await;
    assert_eq!(e["code"], "not_found", "{e}");
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
    let undone = ok(&c, "undo_event", undo_args(&created)).await;
    assert_eq!(undone["events"].as_array().unwrap().len(), 1);
    assert!(titles(&ok(&c, "query_tasks", json!({})).await).is_empty());
    // Undoing the same event again is refused; undoing the undo redoes it.
    let e = fails(&c, "undo_event", undo_args(&created)).await;
    assert_eq!(e["code"], "already_undone");
    ok(&c, "undo_event", undo_args(&undone)).await;
    assert_eq!(titles(&ok(&c, "query_tasks", json!({})).await), ["Oops"]);

    // The owner's event is not the agent's to undo.
    let owner = env
        .t
        .ok(json!({ "cmd": "create_task", "id": id(), "list": inbox, "title": "Owner's" }))
        .await;
    let e = fails(&c, "undo_event", undo_args(&owner)).await;
    assert_eq!(e["code"], "not_own_event", "{e}");
    let e = fails(&c, "undo_event", json!({ "seq": 9999, "event_id": id() })).await;
    assert_eq!(e["code"], "event_not_found");

    // The event ID must match the one at `seq`, and a mismatch does not reveal the actual one.
    let created = ok(&c, "create_task", json!({ "title": "Keep" })).await;
    let mut wrong = undo_args(&created);
    wrong["event_id"] = json!(id());
    let e = fails(&c, "undo_event", wrong).await;
    assert_eq!(e["code"], "event_mismatch", "{e}");
    let actual = created["events"][0]["id"].as_str().unwrap();
    assert!(!e.to_string().contains(actual), "{e}");
    // So does the epoch, which every write result carries.
    assert!(created["epoch"].is_string(), "{created}");
    let mut stale = undo_args(&created);
    stale["epoch"] = json!(id());
    let e = fails(&c, "undo_event", stale).await;
    assert_eq!(e["code"], "epoch_mismatch", "{e}");
    let mut current = undo_args(&created);
    current["epoch"] = created["epoch"].clone();
    ok(&c, "undo_event", current).await;
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

    // Other writes replay too.
    let step = ok(
        &c,
        "add_step",
        json!({ "task_id": first["task_id"], "title": "s" }),
    )
    .await;
    let cid = id();
    let a = json!({ "step_id": step["step_id"], "completed": true, "command_id": cid });
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
    let e = fails(&c, "undo_event", undo_args(&first)).await;
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

#[tokio::test]
async fn hidden_targets_answer_exactly_like_missing_ones() {
    let env = Env::start().await;
    let open = env.t.create_list("Inbox").await;
    let hidden = env.t.create_list("Diary").await;
    let mine = env.t.create_task(&open, "Visible", json!({})).await;
    let secret = env.t.create_task(&hidden, "Secret", json!({})).await;
    let owner_visible = env
        .t
        .ok(json!({ "cmd": "update_task", "id": mine, "patch": { "title": "Visible!" } }))
        .await;
    let owner_hidden = env
        .t
        .ok(json!({ "cmd": "update_task", "id": secret, "patch": { "title": "Secret!" } }))
        .await;
    let tag = id();
    env.t
        .ok(json!({ "cmd": "create_tag", "id": tag, "name": "visible" }))
        .await;
    let secret_reminder = id();
    env.t
        .ok(json!({ "cmd": "add_reminder", "id": secret_reminder, "task": secret, "at": "2030-01-01T09:00:00Z" }))
        .await;
    // Plan everywhere except the diary.
    let (_, token) = env.agent(
        "planner",
        &grants(AccessLevel::Plan, &[(&hidden, AccessLevel::None)]),
        None,
    );
    let c = env.connect(&token).await;
    let missing = id();
    let head = env.t.server.engine.head_seq();

    // MCP: every tool that targets a task.
    for (tool, a) in [
        ("update_task", json!({ "title": "x" })),
        ("complete_task", json!({})),
        ("set_my_day", json!({ "date": "2030-01-01" })),
        ("archive_task", json!({})),
        ("add_note", json!({ "text": "x" })),
        ("add_step", json!({ "title": "x" })),
        ("move_task", json!({ "list_id": open })),
        ("tag_task", json!({ "tag_id": tag })),
        ("untag_task", json!({ "tag_id": tag })),
        ("add_reminder", json!({ "at": "2030-01-01T09:00:00Z" })),
        ("add_link", json!({ "url": "https://todo.example.com/" })),
    ] {
        let with = |task: &str| {
            let mut a = a.clone();
            a["task_id"] = json!(task);
            a
        };
        let h = fails(&c, tool, with(&secret)).await;
        let m = fails(&c, tool, with(&missing)).await;
        assert_eq!(h["code"], "not_found", "{tool}: {h}");
        assert_eq!(same_shape(&h, &secret), same_shape(&m, &missing), "{tool}");
    }
    // A hidden destination, a hidden merge source.
    let h = fails(
        &c,
        "move_task",
        json!({ "task_id": mine, "list_id": hidden }),
    )
    .await;
    let m = fails(
        &c,
        "move_task",
        json!({ "task_id": mine, "list_id": missing }),
    )
    .await;
    assert_eq!(same_shape(&h, &hidden), same_shape(&m, &missing));
    let h = fails(
        &c,
        "merge_tasks",
        json!({ "source_task_id": secret, "target_task_id": mine }),
    )
    .await;
    let m = fails(
        &c,
        "merge_tasks",
        json!({ "source_task_id": missing, "target_task_id": mine }),
    )
    .await;
    assert_eq!(same_shape(&h, &secret), same_shape(&m, &missing));
    // A reminder on a hidden task.
    let at = "2030-01-02T09:00:00Z";
    let h = fails(
        &c,
        "update_reminder",
        json!({ "reminder_id": secret_reminder, "at": at }),
    )
    .await;
    let m = fails(
        &c,
        "update_reminder",
        json!({ "reminder_id": missing, "at": at }),
    )
    .await;
    assert_eq!(h["code"], "not_found", "{h}");
    assert_eq!(same_shape(&h, &secret_reminder), same_shape(&m, &missing));

    // REST goes through the same path.
    let cmd = |task: &str| json!({ "command_id": id(), "command": { "cmd": "complete_task", "id": task } });
    let (hs, hb) = env
        .rest(&token, Method::POST, "/api/v1/commands", Some(cmd(&secret)))
        .await;
    let (ms, mb) = env
        .rest(
            &token,
            Method::POST,
            "/api/v1/commands",
            Some(cmd(&missing)),
        )
        .await;
    assert_eq!((hs, ms), (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND));
    assert_eq!(same_shape(&hb, &secret), same_shape(&mb, &missing));

    // Undo: an event the agent cannot see is a missing one.
    let hidden_seq = first_seq(&owner_hidden);
    let missing_seq = 9999;
    // Neither the right event ID nor a wrong one (which would otherwise get `event_mismatch`) tells
    // the agent anything.
    let real = undo_args(&owner_hidden);
    let h = fails(&c, "undo_event", real).await;
    assert_eq!(h["code"], "event_not_found", "{h}");
    let h = fails(
        &c,
        "undo_event",
        json!({ "seq": hidden_seq, "event_id": id() }),
    )
    .await;
    let m = fails(
        &c,
        "undo_event",
        json!({ "seq": missing_seq, "event_id": id() }),
    )
    .await;
    assert_eq!(h["code"], "event_not_found", "{h}");
    assert_eq!(
        h.to_string().replace(&hidden_seq.to_string(), "<seq>"),
        m.to_string().replace(&missing_seq.to_string(), "<seq>")
    );
    let undo = |seq: u64| {
        (
            format!("/api/v1/events/{seq}/undo"),
            json!({ "command_id": id(), "expected_event_id": id() }),
        )
    };
    let (p, b) = undo(hidden_seq);
    let (hs, _) = env.rest(&token, Method::POST, &p, Some(b)).await;
    let (p, b) = undo(missing_seq);
    let (ms, _) = env.rest(&token, Method::POST, &p, Some(b)).await;
    assert_eq!((hs, ms), (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND));
    // A visible event of someone else is still refused as such.
    let e = fails(&c, "undo_event", undo_args(&owner_visible)).await;
    assert_eq!(e["code"], "not_own_event");
    // A visible target with too low a level is still `forbidden`.
    let (_, reader) = env.agent("reader", &Grants::with_default(AccessLevel::Read), None);
    let r = env.connect(&reader).await;
    let e = fails(&r, "complete_task", json!({ "task_id": mine })).await;
    assert_eq!(e["code"], "forbidden");
    assert_eq!(env.t.server.engine.head_seq(), head, "nothing was written");
}

#[tokio::test]
async fn agents_cannot_import_anywhere_and_learn_nothing_from_trying() {
    let env = Env::start().await;
    let open = env.t.create_list("Inbox").await;
    let hidden = env.t.create_list("Diary").await;
    // Plan everywhere except the diary: core alone would report `have: none` only there.
    let (_, token) = env.agent(
        "planner",
        &grants(AccessLevel::Plan, &[(&hidden, AccessLevel::None)]),
        None,
    );
    let head = env.t.server.engine.head_seq();
    let import = |list: &str| {
        let stamp =
            json!({ "at": "2020-01-02T03:04:05Z", "by": { "type": "owner", "device": "alpha" } });
        json!({ "command_id": id(), "command": { "cmd": "import_entity", "entity": {
            "kind": "task", "data": {
                "id": id(), "list": list, "title": "Imported", "notes": "",
                "due": null, "priority": "none", "my_day": null, "completed_at": null,
                "recurrence": null, "tags": [], "position": "a", "created": stamp,
                "updated": stamp, "deleted": null, "merged_into": null
            }
        } } })
    };
    let mut answers = Vec::new();
    for list in [open.clone(), hidden.clone(), id()] {
        let (status, body) = env
            .rest(
                &token,
                Method::POST,
                "/api/v1/commands",
                Some(import(&list)),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"]["code"], "forbidden");
        assert_eq!(body["error"]["details"]["owner_only"], true);
        answers.push(body);
    }
    assert!(answers.windows(2).all(|w| w[0] == w[1]), "{answers:?}");
    assert_eq!(env.t.server.engine.head_seq(), head, "nothing was written");
    // No MCP tool takes a raw command: the tool list is exactly the fixed set.
    let c = env.connect(&token).await;
    let mut tools: Vec<String> = c
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    tools.sort();
    assert_eq!(tools, ALL_TOOLS);
}

#[tokio::test]
async fn a_retry_after_losing_access_gets_not_found_and_never_writes_twice() {
    let env = Env::start().await;
    let work = env.t.create_list("Work").await;
    let plan = grants(AccessLevel::None, &[(&work, AccessLevel::Plan)]);
    let (row, token) = env.agent("planner", &plan, None);
    let c = env.connect(&token).await;
    let cid = id();
    let a = json!({ "title": "Draft", "list_id": work, "command_id": cid });
    let first = ok(&c, "create_task", a.clone()).await;
    let undo_cid = id();
    let mut retried = undo_args(&first);
    retried["command_id"] = json!(undo_cid);
    ok(&c, "undo_event", retried.clone()).await;
    let head = env.t.server.engine.head_seq();

    // The owner takes the list away: retries no longer show the stored results.
    let svc = &env.t.server.authsvc;
    assert!(
        svc.set_grants(row, &grants(AccessLevel::None, &[]), None)
            .unwrap()
    );
    let e = fails(&c, "create_task", a.clone()).await;
    assert_eq!(e["code"], "not_found", "{e}");
    let e = fails(&c, "undo_event", retried.clone()).await;
    assert_eq!(e["code"], "event_not_found", "{e}");
    assert_eq!(env.t.server.engine.head_seq(), head);

    // Given back, the same retries replay: the records were kept and nothing ran twice.
    assert!(svc.set_grants(row, &plan, None).unwrap());
    let again = ok(&c, "create_task", a).await;
    assert_eq!(again["replayed"], true);
    assert_eq!(again["events"], first["events"]);
    let again = ok(&c, "undo_event", retried).await;
    assert_eq!(again["replayed"], true);
    assert_eq!(env.t.server.engine.head_seq(), head);
}

#[tokio::test]
async fn retries_replay_even_after_their_defaults_changed() {
    let offset = Arc::new(AtomicI64::new(0));
    let env = Env::start_with_clock(offset.clone()).await;
    let inbox = env.t.create_list("Inbox").await;
    let (_, token) = env.agent("planner", &Grants::with_default(AccessLevel::Plan), None);
    let c = env.connect(&token).await;

    // The implicit Inbox: renamed, and another list called Inbox appears, before the retry.
    let cid = id();
    let a = json!({ "title": "Call back", "command_id": cid });
    let first = ok(&c, "create_task", a.clone()).await;
    assert_eq!(first["list_id"], inbox);
    env.t
        .ok(json!({ "cmd": "update_list", "id": inbox, "name": "Old inbox" }))
        .await;
    env.t.create_list("Inbox").await;
    let head = env.t.server.engine.head_seq();
    let again = ok(&c, "create_task", a).await;
    assert_eq!(again["replayed"], true, "{again}");
    assert_eq!(again["list_id"], inbox);
    assert_eq!(again["events"], first["events"]);
    // Different arguments under the same ID are still refused.
    let e = fails(
        &c,
        "create_task",
        json!({ "title": "Other", "command_id": cid }),
    )
    .await;
    assert_eq!(e["code"], "command_id_reused");

    // "Today" in set_my_day: the retry comes after midnight.
    let task = first["task_id"].clone();
    let cid = id();
    let a = json!({ "task_id": task, "command_id": cid });
    let first = ok(&c, "set_my_day", a.clone()).await;
    offset.store(36 * 3_600_000, Ordering::SeqCst);
    let again = ok(&c, "set_my_day", a).await;
    assert_eq!(again["replayed"], true, "{again}");
    assert_eq!(again["date"], first["date"]);
    assert_eq!(again["events"], first["events"]);
    assert_eq!(
        env.t.server.engine.head_seq(),
        head + first["events"].as_array().unwrap().len() as u64
    );
}

#[tokio::test]
async fn the_implicit_inbox_is_one_the_agent_can_capture_into() {
    let env = Env::start().await;
    let read_only = env.t.create_list("Inbox").await;
    let coll = id();
    env.t
        .ok(json!({ "cmd": "create_collection", "id": coll, "name": "Home" }))
        .await;
    let writable = id();
    env.t
        .ok(json!({ "cmd": "create_list", "id": writable, "collection": coll, "name": "inbox" }))
        .await;
    let (_, token) = env.agent(
        "scout",
        &grants(
            AccessLevel::None,
            &[
                (&read_only, AccessLevel::Read),
                (&writable, AccessLevel::Capture),
            ],
        ),
        None,
    );
    let c = env.connect(&token).await;
    // The top-level Inbox comes first, but the agent can only read it.
    let created = ok(&c, "create_task", json!({ "title": "x" })).await;
    assert_eq!(created["list_id"], writable);

    // With only a read-only Inbox there is no implicit target.
    let (_, token) = env.agent(
        "reader",
        &grants(AccessLevel::None, &[(&read_only, AccessLevel::Read)]),
        None,
    );
    let c = env.connect(&token).await;
    let e = fails(&c, "create_task", json!({ "title": "y" })).await;
    assert_eq!(e["code"], "inbox_not_found", "{e}");
}
