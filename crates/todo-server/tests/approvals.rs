#![allow(clippy::unwrap_used)]

mod common;

use common::web::{Body, Env, SITE};
use common::{TOKEN_A, TOKEN_B, TestServer};
use reqwest::{Method, StatusCode, header::HeaderMap};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use todo_core::actor::ASSISTANT_AGENT_ID;
use todo_core::time::Timestamp;
use todo_core::{AccessLevel, Actor, AgentId, State, apply, ids::ListId};
use todo_tools::assistant::approval::{ApprovalRequest, Batch, Revision};
use uuid::Uuid;

const PATH: &str = "/api/v1/assistant/approvals";

fn review(t: &TestServer, list: &str, commands: &[Value]) -> ApprovalRequest {
    let e = &t.server.engine;
    let command_id = Uuid::new_v4();
    let mut batch = Batch::new(
        command_id,
        Revision {
            epoch: e.epoch(),
            cursor: e.head_seq(),
        },
        7,
        &e.state(),
        BTreeSet::from([list.parse::<ListId>().unwrap()]),
        e.now(),
    )
    .unwrap();
    for (i, command) in commands.iter().enumerate() {
        // Display indices need not coincide with command ordinals used to seed IDs.
        assert!(
            batch
                .push(serde_json::from_value(command.clone()).unwrap(), i * 2 + 1)
                .unwrap()
        );
    }
    ApprovalRequest {
        command_id,
        batch: batch.freeze().unwrap().unwrap().approval().unwrap(),
    }
}

async fn post(
    t: &TestServer,
    token: Option<&str>,
    body: &ApprovalRequest,
) -> (StatusCode, HeaderMap, Value) {
    t.request(Method::POST, PATH, token, Some(json!(body)))
        .await
}

async fn two_creates(t: &TestServer) -> (ApprovalRequest, String, String) {
    let list = t.create_list("Inbox").await;
    let a = common::id();
    let b = common::id();
    let req = review(
        t,
        &list,
        &[
            json!({"cmd":"create_task","list":list,"id":a,"title":"First"}),
            json!({"cmd":"create_task","list":list,"id":b,"title":"Second"}),
        ],
    );
    (req, a, b)
}

#[tokio::test]
async fn owner_only_reserved_identity_and_strict_approval_body() {
    let t = TestServer::start().await;
    let (req, id, _) = two_creates(&t).await;
    let svc = &t.server.authsvc;
    let reserved = AgentId::new(ASSISTANT_AGENT_ID).unwrap();
    assert_eq!(
        svc.create_agent(&reserved, AccessLevel::Plan, None, None, None)
            .unwrap_err()
            .code,
        "reserved_agent"
    );
    let (.., token) = svc
        .create_agent(
            &AgentId::new("external").unwrap(),
            AccessLevel::Plan,
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(post(&t, None, &req).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(post(&t, Some(&token), &req).await.0, StatusCode::FORBIDDEN);
    // Authorization runs before extraction, even if the authenticated agent has no body.
    assert_eq!(
        t.request(Method::POST, PATH, Some(&token), None).await.2["error"]["code"],
        "owner_required"
    );
    for (path, value) in [
        ("/actor", json!({"type":"owner","device":"alpha"})),
        ("/batch/grants", json!({"default":"plan"})),
        ("/batch/commands/0/actor", json!("owner")),
        ("/batch/commands/0/notes", Value::Null),
        ("/batch/commands/0/cmd", json!("delete")),
        (
            "/batch/commands/0",
            json!({"cmd":"update_task","id":id,"patch":{"title":"New", "notes":null}}),
        ),
    ] {
        let mut body = json!(req);
        if let Some(field) = body.pointer_mut(path) {
            *field = value;
        } else {
            let (parent, key) = path.rsplit_once('/').unwrap();
            body.pointer_mut(parent).unwrap()[key] = value;
        }
        let (status, _, error) = t
            .request(Method::POST, PATH, Some(TOKEN_A), Some(body))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {error}");
        assert_eq!(error["error"]["message"], "invalid approval body");
    }
    assert_eq!(t.server.engine.head_seq(), req.batch.revision.cursor);
    let (status, _, _) = post(&t, Some(TOKEN_A), &req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !t.server.engine.state().grants.contains_key(&reserved),
        "scratch grants never commit"
    );
}

#[tokio::test]
async fn browser_owner_requires_csrf_and_the_exact_write_origin() {
    let env = Env::start().await;
    let mut browser = env.owner().await;
    let (req, _, _) = two_creates(&env.t).await;
    let csrf = browser.csrf.take();
    for origin in [Some(SITE), None] {
        let r = browser
            .send(Method::POST, PATH, origin, Body::Json(json!(req)))
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        assert_eq!(r.json()["error"]["code"], "csrf");
    }
    browser.csrf = csrf;
    for origin in [Some("https://other.example.com"), None] {
        let r = browser
            .send(Method::POST, PATH, origin, Body::Json(json!(req)))
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        assert_eq!(r.json()["error"]["code"], "csrf");
    }
    assert_eq!(env.t.server.engine.head_seq(), req.batch.revision.cursor);
    let r = browser
        .send(Method::POST, PATH, Some(SITE), Body::Json(json!(req)))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
}

#[tokio::test]
async fn writer_redecides_scope_time_identity_order_and_every_command() {
    let t = TestServer::start().await;
    let (mut req, _, _) = two_creates(&t).await;
    let private = t.create_list("Excluded name").await;
    let hidden = t.create_task(&private, "Excluded title", json!({})).await;
    // Rebuild the review at the latest head. The second command must not learn the hidden title.
    let commands: Vec<_> = req.batch.commands.iter().map(|c| json!(c)).collect();
    let list = commands[0]["list"].as_str().unwrap();
    req = review(&t, list, &commands);
    let head = req.batch.revision.cursor;
    let base = t.server.engine.state();

    for id in [&hidden, &common::id()] {
        let mut forged = req.clone();
        forged.batch.commands[1] =
            serde_json::from_value(json!({"cmd":"complete_task","id":id})).unwrap();
        let (status, _, error) = post(&t, Some(TOKEN_A), &forged).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error["error"]["code"], "invalid_approval");
        assert!(!error.to_string().contains("Excluded"));
        assert!(!error.to_string().contains(id));
    }
    for changed in [0, 1, 2, 3, 4] {
        let mut forged = req.clone();
        match changed {
            0 => forged.batch.at = Timestamp::UNIX_EPOCH,
            1 => forged.command_id = Uuid::new_v4(),
            2 => forged.batch.commands.swap(0, 1),
            3 => {
                forged.batch.allowed_lists.insert(private.parse().unwrap());
            }
            _ => forged.batch.digest[0] ^= 1,
        }
        let (status, _, error) = post(&t, Some(TOKEN_A), &forged).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["error"]["code"], "approval_digest_mismatch");
    }
    assert_eq!(t.server.engine.head_seq(), head);
    assert_eq!(*t.server.engine.state(), *base);
    assert_eq!(post(&t, Some(TOKEN_A), &req).await.0, StatusCode::OK);
}

#[tokio::test]
async fn failure_inserting_second_event_rolls_everything_back_and_same_id_can_retry() {
    let t = TestServer::start().await;
    let (req, _, _) = two_creates(&t).await;
    let engine = &t.server.engine;
    let before = engine.dump_projections().unwrap();
    let db = rusqlite::Connection::open(t.db_path()).unwrap();
    db.execute_batch("CREATE TRIGGER fail_second BEFORE INSERT ON events WHEN NEW.command_idx = 1 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    let (status, _, error) = post(&t, Some(TOKEN_A), &req).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{error}");
    assert_eq!(engine.head_seq(), req.batch.revision.cursor);
    assert_eq!(engine.dump_projections().unwrap(), before);
    assert!(engine.state().tasks.is_empty());
    let receipt_count: i64 = db
        .query_row(
            "SELECT count(*) FROM commands WHERE command_id = ?1",
            [req.command_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt_count, 0);
    let persisted_count: i64 = db
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        u64::try_from(persisted_count).unwrap(),
        req.batch.revision.cursor
    );
    db.execute_batch("DROP TRIGGER fail_second").unwrap();
    let (status, headers, result) = post(&t, Some(TOKEN_A), &req).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert!(!headers.contains_key("idempotent-replayed"));
    assert_eq!(result["events"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn two_reviewers_one_commit_replay_after_unrelated_write_and_never_after_restore() {
    let t = TestServer::start().await;
    let (a, _, _) = two_creates(&t).await;
    let list = json!(a)["batch"]["commands"][0]["list"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = review(
        &t,
        &list,
        &[json!({"cmd":"create_task","list":list,"id":common::id(),"title":"Other review"})],
    );
    let (r1, r2) = tokio::join!(post(&t, Some(TOKEN_A), &a), post(&t, Some(TOKEN_B), &b));
    let (winner, receipt, loser) = if r1.0 == StatusCode::OK {
        (&a, r1.2, &r2)
    } else {
        (&b, r2.2, &r1)
    };
    assert_eq!(loser.0, StatusCode::CONFLICT);
    assert_eq!(loser.2["error"]["code"], "approval_stale");
    assert_eq!(receipt["command_id"], json!(winner.command_id));
    let stale = review(
        &t,
        &list,
        &[
            json!({"cmd":"create_task","list":list,"id":common::id(),"title":"Preview before intervening event"}),
        ],
    );
    t.create_list("Unrelated").await;
    assert_eq!(
        post(&t, Some(TOKEN_A), &stale).await.2["error"]["code"],
        "approval_stale"
    );
    let (status, headers, replay) = post(&t, Some(TOKEN_B), winner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["idempotent-replayed"], "true");
    assert_eq!(replay, receipt);
    let mut wrong = winner.clone();
    wrong.batch.at = Timestamp::UNIX_EPOCH;
    assert_eq!(
        post(&t, Some(TOKEN_B), &wrong).await.2["error"]["code"],
        "command_id_reused"
    );

    // Leave the receipt present, as when a restore includes the original approval.
    let t = t
        .restart_with(|p| {
            todo_server::db::set_log_epoch(&rusqlite::Connection::open(p).unwrap(), Uuid::new_v4())
                .unwrap();
        })
        .await;
    assert_eq!(
        post(&t, Some(TOKEN_B), winner).await.2["error"]["code"],
        "epoch_mismatch"
    );
    assert_eq!(
        post(&t, Some(TOKEN_A), &stale).await.2["error"]["code"],
        "epoch_mismatch"
    );
}

#[tokio::test]
async fn one_activity_group_replays_and_undoes_even_repeated_edits_but_conflicts_atomically() {
    let t = TestServer::start().await;
    let list = t.create_list("Inbox").await;
    let task = t.create_task(&list, "Original", json!({})).await;
    let other = t.create_task(&list, "Other", json!({})).await;
    let req = review(
        &t,
        &list,
        &[
            json!({"cmd":"update_task","id":task,"patch":{"title":"Temporary"}}),
            json!({"cmd":"update_task","id":task,"patch":{"title":"New title"}}),
            json!({"cmd":"complete_task","id":other}),
        ],
    );
    let (status, _, result) = post(&t, Some(TOKEN_A), &req).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let engine = &t.server.engine;
    let events = engine.events_since(req.batch.revision.cursor, 50).unwrap();
    assert_eq!(events.len(), 3);
    assert!(
        events.iter().all(
            |e| e.group == events[0].id && e.actor == Actor::agent(ASSISTANT_AGENT_ID).unwrap()
        )
    );
    let page = t.get_ok("/api/v1/activity?agent=assistant").await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert!(
        items
            .iter()
            .all(|item| item["event"]["actor"]["agent_id"] == ASSISTANT_AGENT_ID)
    );
    let mut replayed = State::new();
    for event in engine.events_since(0, 50).unwrap() {
        apply(&mut replayed, &event).unwrap();
    }
    assert!(replayed.data_eq(&engine.state()));

    let third = result["events"][2]["seq"].as_u64().unwrap();
    let edit = t
        .ok(json!({"cmd":"update_task","id":task,"patch":{"title":"Edited elsewhere"}}))
        .await;
    let before = engine.state();
    let (status, error) = t
        .undo(third, Some(json!({"epoch":req.batch.revision.epoch})))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "undo_conflict");
    assert!(engine.state().data_eq(&before));
    assert_eq!(
        t.undo(common::first_seq(&edit), None).await.0,
        StatusCode::OK
    );
    let (status, undone) = t
        .undo(third, Some(json!({"epoch":req.batch.revision.epoch})))
        .await;
    assert_eq!(status, StatusCode::OK, "{undone}");
    assert_eq!(undone["events"].as_array().unwrap().len(), 3);
    assert_eq!(
        engine.state().tasks[&task.parse().unwrap()].title,
        "Original"
    );
    assert!(!engine.state().tasks[&other.parse().unwrap()].is_done());
}
