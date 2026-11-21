//! Visibility is decided inside the writer (see `todo_server::api::hidden`): access lost while a write
//! waits for the writer refuses it exactly like a missing target, and nothing is stored.
//!
//! Each race is made deterministic: the test holds the writer, sends an agent write, waits until it is
//! queued for the writer (so it got past every check made before), revokes the grant that made its
//! target visible while still holding the writer, and only then lets it run.
#![allow(clippy::unwrap_used)]

mod common;

use common::mcp::{agent, connect, fails, grants, list_id, same_shape};
use common::{TestServer, first_seq, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;
use todo_core::{AccessLevel, AgentId};
use todo_server::engine::Engine;

/// An owner's list "Shared" with a task tagged with a tag, and a list "Work" with a task. The agent
/// `scout` (default level none) can read "Shared", which is how it sees the tag, and plan in "Work".
struct Setup {
    t: TestServer,
    shared: String,
    work_list: String,
    work_task: String,
    tag: String,
    token: String,
}

impl Setup {
    async fn new() -> Self {
        let t = TestServer::start().await;
        let shared = t.create_list("Shared").await;
        let work = t.create_list("Work").await;
        let shared_task = t.create_task(&shared, "Shared task", json!({})).await;
        let work_task = t.create_task(&work, "Work task", json!({})).await;
        let tag = id();
        t.ok(json!({ "cmd": "create_tag", "id": tag, "name": "Errand" }))
            .await;
        t.ok(json!({ "cmd": "add_tag", "task": shared_task, "tag": tag }))
            .await;
        let (_, token) = agent(
            &t,
            "scout",
            &grants(
                AccessLevel::None,
                &[(&shared, AccessLevel::Read), (&work, AccessLevel::Plan)],
            ),
            None,
        );
        Self {
            t,
            shared,
            work_list: work,
            work_task,
            tag,
            token,
        }
    }

    fn engine(&self) -> &Engine {
        &self.t.server.engine
    }

    /// The agent's `add_tag` of `tag` on the work task, as a REST command.
    fn send_add_tag(
        &self,
        command_id: &str,
        tag: &str,
    ) -> tokio::task::JoinHandle<(StatusCode, Value)> {
        let (client, url, token) = (
            self.t.client.clone(),
            self.t.url.clone(),
            self.token.clone(),
        );
        let body = json!({
            "command_id": command_id,
            "command": { "cmd": "add_tag", "task": self.work_task, "tag": tag },
        });
        tokio::spawn(async move {
            let res = client
                .request(Method::POST, format!("{url}/api/v1/commands"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            (res.status(), res.json().await.unwrap())
        })
    }

    async fn agent_request(&self, method: Method, path: &str, body: Value) -> (StatusCode, Value) {
        let (s, _, v) = self
            .t
            .request(method, path, Some(&self.token), Some(body))
            .await;
        (s, v)
    }

    /// The IDs of the work task's tags, as the owner sees them.
    async fn work_tags(&self) -> Vec<String> {
        let v = self
            .t
            .get_ok(&format!("/api/v1/tasks/{}", self.work_task))
            .await;
        v["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Takes "Shared" out of the agent's grants. Called while the test holds the writer, so it goes
    /// through the hold rather than the auth service (which would wait for the writer).
    fn revoke_shared(&self, hold: &todo_server::engine::WriterHold<'_>) {
        let mut all = self.engine().state().grants.clone();
        let scout = all.get_mut(&AgentId::new("scout").unwrap()).unwrap();
        scout.lists.remove(&list_id(&self.shared));
        hold.set_grants(all);
    }
}

/// Waits until `n` writes are queued for the (held) writer.
async fn queued(engine: &Engine, n: usize) {
    for _ in 0..1000 {
        if engine.waiting_writes() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the write never reached the writer");
}

#[tokio::test]
async fn a_tag_hidden_while_the_command_waits_is_not_found_and_nothing_is_stored() {
    let s = Setup::new().await;
    // What a tag that never existed gets, to compare with.
    let missing = id();
    let (status, expected) = s.send_add_tag(&id(), &missing).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND, "{expected}");
    let head = s.engine().head_seq();

    let command_id = id();
    let hold = s.engine().hold_writer();
    let pending = s.send_add_tag(&command_id, &s.tag);
    queued(s.engine(), 1).await;
    s.revoke_shared(&hold);
    drop(hold);
    let (status, v) = pending.await.unwrap();

    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
    assert_eq!(v["error"]["code"], "not_found");
    assert_eq!(
        same_shape(&v, &s.tag),
        same_shape(&expected, &missing),
        "a tag hidden during the race must look missing"
    );
    assert_eq!(s.engine().head_seq(), head, "nothing may be committed");
    assert!(s.work_tags().await.is_empty());
    // A retry with the same ID still finds nothing: no record was stored.
    let (status, again) = s.send_add_tag(&command_id, &s.tag).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND, "{again}");
    assert_eq!(again, v);
    assert_eq!(s.engine().head_seq(), head);
}

#[tokio::test]
async fn the_same_queued_command_succeeds_when_nothing_is_revoked() {
    let s = Setup::new().await;
    let head = s.engine().head_seq();
    let hold = s.engine().hold_writer();
    let pending = s.send_add_tag(&id(), &s.tag);
    queued(s.engine(), 1).await;
    drop(hold);
    let (status, v) = pending.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(s.engine().head_seq() > head);
    assert_eq!(s.work_tags().await, vec![s.tag.clone()]);
}

#[tokio::test]
async fn an_mcp_tag_task_hidden_while_it_waits_is_not_found_and_nothing_is_stored() {
    let s = Setup::new().await;
    let c = connect(&s.t, &s.token).await;
    let missing = id();
    let expected = fails(
        &c,
        "tag_task",
        json!({ "task_id": s.work_task, "tag_id": missing }),
    )
    .await;
    let head = s.engine().head_seq();

    let hold = s.engine().hold_writer();
    let args = json!({ "task_id": s.work_task, "tag_id": s.tag });
    let pending = tokio::spawn(async move { (fails(&c, "tag_task", args).await, c) });
    queued(s.engine(), 1).await;
    s.revoke_shared(&hold);
    drop(hold);
    let (e, _c) = pending.await.unwrap();

    assert_eq!(e["code"], "not_found", "{e}");
    assert_eq!(same_shape(&e, &s.tag), same_shape(&expected, &missing));
    assert_eq!(s.engine().head_seq(), head, "nothing may be committed");
    assert!(s.work_tags().await.is_empty());
}

#[tokio::test]
async fn an_undo_whose_event_is_hidden_while_it_waits_is_event_not_found() {
    let s = Setup::new().await;
    // The agent's own note on the work task, which it may undo at plan level in "Work".
    let (status, r) = s
        .agent_request(
            Method::POST,
            "/api/v1/commands",
            json!({ "command_id": id(), "command": { "cmd": "add_note", "task": s.work_task, "text": "mine" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    let seq = first_seq(&r);
    let event_id = r["events"][0]["id"].clone();
    // What a sequence number with no event gets, to compare with.
    let nowhere = s.engine().head_seq() + 1000;
    let (status, expected) = s
        .agent_request(
            Method::POST,
            &format!("/api/v1/events/{nowhere}/undo"),
            json!({ "command_id": id(), "expected_event_id": event_id }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{expected}");
    let head = s.engine().head_seq();

    let hold = s.engine().hold_writer();
    let (client, url, token) = (s.t.client.clone(), s.t.url.clone(), s.token.clone());
    let body = json!({ "command_id": id(), "expected_event_id": event_id });
    let pending = tokio::spawn(async move {
        let res = client
            .post(format!("{url}/api/v1/events/{seq}/undo"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        (res.status(), res.json::<Value>().await.unwrap())
    });
    queued(s.engine(), 1).await;
    // Revoke every list, "Work" included, while the undo waits.
    let mut all = s.engine().state().grants.clone();
    all.get_mut(&AgentId::new("scout").unwrap())
        .unwrap()
        .lists
        .clear();
    hold.set_grants(all);
    drop(hold);
    let (status, v) = pending.await.unwrap();

    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
    assert_eq!(v["error"]["code"], "event_not_found");
    assert_eq!(
        v.to_string().replace(&seq.to_string(), "<seq>"),
        expected.to_string().replace(&nowhere.to_string(), "<seq>"),
        "an event hidden during the race must look missing"
    );
    assert_eq!(s.engine().head_seq(), head, "nothing may be committed");
}

#[tokio::test]
async fn a_command_over_the_tag_cap_is_refused_before_the_writer() {
    let s = Setup::new().await;
    let head = s.engine().head_seq();
    let over = todo_core::decide::MAX_TASK_TAGS + 1;
    let create = |tags: Vec<String>| {
        json!({
            "command_id": id(),
            "command": { "cmd": "create_task", "id": id(), "list": s.work_list, "title": "t", "tags": tags },
        })
    };
    // The writer stays held: a command that reached it would never answer.
    let hold = s.engine().hold_writer();
    let answer = |body: Value| {
        tokio::time::timeout(
            Duration::from_secs(10),
            s.agent_request(Method::POST, "/api/v1/commands", body),
        )
    };
    let (status, visible) = answer(create(vec![s.tag.clone(); over]))
        .await
        .expect("refused before the writer");
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{visible}");
    assert_eq!(visible["error"]["code"], "invalid");
    assert_eq!(s.engine().waiting_writes(), 0);
    // The same answer for tags that do not exist: it depends on nothing hidden.
    let (status, missing) = answer(create((0..over).map(|_| id()).collect()))
        .await
        .expect("refused before the writer");
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{missing}");
    assert_eq!(missing, visible);
    drop(hold);
    assert_eq!(s.engine().head_seq(), head);
}

impl Setup {
    /// The agent's upload of `a.txt` to the work task with attachment ID `att`.
    fn send_upload(
        &self,
        att: &str,
    ) -> tokio::task::JoinHandle<(StatusCode, Option<String>, Value)> {
        let (client, url, token) = (
            self.t.client.clone(),
            self.t.url.clone(),
            self.token.clone(),
        );
        let (task, att) = (self.work_task.clone(), att.to_owned());
        tokio::spawn(async move {
            let res = client
                .post(format!("{url}/api/v1/tasks/{task}/attachments"))
                .bearer_auth(token)
                .query(&[("file_name", "a.txt"), ("attachment_id", att.as_str())])
                .body("once")
                .send()
                .await
                .unwrap();
            let replayed = res
                .headers()
                .get("idempotent-replayed")
                .map(|v| v.to_str().unwrap().to_owned());
            (res.status(), replayed, res.json().await.unwrap())
        })
    }
}

#[tokio::test]
async fn an_upload_retry_whose_task_is_hidden_while_it_waits_is_not_found() {
    let s = Setup::new().await;
    let att = id();
    let (status, _, first) = s.send_upload(&att).await.unwrap();
    assert_eq!(status, StatusCode::CREATED, "{first}");
    // What an upload to a task that never existed gets, to compare with.
    let (client, url, token) = (s.t.client.clone(), s.t.url.clone(), s.token.clone());
    let nowhere = id();
    let res = client
        .post(format!("{url}/api/v1/tasks/{nowhere}/attachments"))
        .bearer_auth(token)
        .query(&[("file_name", "a.txt")])
        .body("once")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let expected: Value = res.json().await.unwrap();
    let head = s.engine().head_seq();

    let hold = s.engine().hold_writer();
    let retry = s.send_upload(&att);
    // The retry found its stored result and blob, and waits to confirm the replay.
    queued(s.engine(), 1).await;
    let mut all = s.engine().state().grants.clone();
    all.get_mut(&AgentId::new("scout").unwrap())
        .unwrap()
        .lists
        .clear();
    hold.set_grants(all);
    drop(hold);
    let (status, replayed, v) = retry.await.unwrap();

    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
    assert_eq!(replayed, None);
    assert_eq!(v["error"]["code"], expected["error"]["code"]);
    assert_eq!(
        same_shape(&v, &s.work_task),
        same_shape(&expected, &nowhere),
        "a task hidden during the race must look missing"
    );
    assert_eq!(s.engine().head_seq(), head);
}

#[tokio::test]
async fn an_upload_retry_that_waits_is_replayed_when_nothing_is_revoked() {
    let s = Setup::new().await;
    let att = id();
    let (status, _, first) = s.send_upload(&att).await.unwrap();
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let hold = s.engine().hold_writer();
    let retry = s.send_upload(&att);
    queued(s.engine(), 1).await;
    drop(hold);
    let (status, replayed, v) = retry.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(replayed.as_deref(), Some("true"));
    assert_eq!(v["attachment"], first["attachment"]);
}
