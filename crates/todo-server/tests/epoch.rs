#![allow(clippy::unwrap_used)]
//! Event provenance and the log epoch: every event names the command that produced it, every page
//! and stream names the log it belongs to, and an undo names the event it means, so a client never
//! confuses one history with another after a restore.

mod common;

use common::{TOKEN_A, TestServer, first_seq, id};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use todo_server::Engine;

/// Copies a stopped server's database with its WAL sidecar files (removing stale ones at `to`).
fn copy_db(from: &Path, to: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let (f, t) = (sidecar(from, suffix), sidecar(to, suffix));
        if f.exists() {
            std::fs::copy(&f, &t).unwrap();
        } else if t.exists() {
            std::fs::remove_file(&t).unwrap();
        }
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

async fn epoch(t: &TestServer) -> String {
    t.get_ok("/api/v1/events?since=0").await["epoch"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn events_carry_the_command_that_produced_them_everywhere() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let head = t.server.engine.head_seq();
    let mut sse = t.sse(TOKEN_A, Some(head)).await;
    let cid = id();
    let task = id();
    let (s, _, r) = t
        .command_with_id(
            TOKEN_A,
            &cid,
            json!({ "cmd": "create_task", "id": task, "list": list, "title": "T" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let seq = first_seq(&r);

    let page = t.get_ok(&format!("/api/v1/events?since={head}")).await;
    assert_eq!(page["events"][0]["command_id"], cid);
    assert_eq!(page["events"][0]["command_idx"], 0);
    assert_eq!(page["events"][0]["seq"], seq, "the event fields are inline");

    let f = sse.next_event(Duration::from_secs(5)).await.unwrap();
    assert_eq!(f.json()["command_id"], cid);
    assert_eq!(f.json()["command_idx"], 0);

    let feed = t.get_ok("/api/v1/activity").await;
    assert_eq!(feed["items"][0]["event"]["command_id"], cid);
}

#[tokio::test]
async fn pages_and_streams_name_their_epoch() {
    let t = TestServer::start().await;
    t.create_list("L").await;
    let e = epoch(&t).await;
    assert_eq!(e, t.server.engine.epoch().to_string());
    let mut sse = t.sse(TOKEN_A, Some(0)).await;
    assert_eq!(sse.epoch_header.as_deref(), Some(e.as_str()));
    sse.next_event(Duration::from_secs(5)).await.unwrap();
    let hello = sse.hello.clone().unwrap();
    assert_eq!(hello.id, None, "the hello frame moves no cursor");
    assert_eq!(hello.json()["epoch"], e);
    assert_eq!(hello.json()["head_seq"], 1);
    // The epoch is stable across restarts.
    let t = t.restart().await;
    assert_eq!(epoch(&t).await, e);
}

#[tokio::test]
async fn a_request_for_another_epoch_gets_409_epoch_mismatch() {
    let t = TestServer::start().await;
    t.create_list("L").await;
    let e = epoch(&t).await;
    let page = t.get_ok(&format!("/api/v1/events?since=0&epoch={e}")).await;
    assert_eq!(page["events"].as_array().unwrap().len(), 1);

    let other = id();
    // Checked before the position: a cursor from another log may well be "ahead" of this one.
    for since in [0, 99] {
        let (s, v) = t
            .get(&format!("/api/v1/events?since={since}&epoch={other}"))
            .await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(v["error"]["code"], "epoch_mismatch");
        assert_eq!(v["error"]["details"]["epoch"], e);

        let res = t
            .client
            .get(format!(
                "{}/api/v1/events?since={since}&epoch={other}",
                t.url
            ))
            .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
            .header(ACCEPT, "text/event-stream")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "epoch_mismatch");
    }
}

#[tokio::test]
async fn rotating_the_epoch_after_a_restore_makes_old_cursors_and_undos_fail() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let (dir, config) = t.stop().await;
    let backup = dir.path().join("backup.sqlite");
    copy_db(&config.database_path(), &backup);

    // The log grows past the backup, and a client follows it.
    let t = TestServer::boot(dir, config).await;
    let old = epoch(&t).await;
    t.create_task(&list, "lost", json!({})).await;
    let lost = t.get_ok("/api/v1/events?since=1").await["events"][0].clone();

    // Restore the backup, rotate, and regrow to the same length with another event.
    let (dir, config) = t.stop().await;
    copy_db(&backup, &config.database_path());
    let new = Engine::rotate_log_epoch(&config.database_path()).unwrap();
    assert_ne!(new.to_string(), old);
    let t = TestServer::boot(dir, config).await;
    assert_eq!(epoch(&t).await, new.to_string());
    t.create_task(&list, "other", json!({})).await;
    let seq = lost["seq"].as_u64().unwrap();
    assert_eq!(t.server.engine.head_seq(), seq, "same seq, another event");

    let (s, v) = t.get(&format!("/api/v1/events?since=1&epoch={old}")).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["error"]["code"], "epoch_mismatch");

    // A queued undo of the lost event: refused by its epoch, and by its event ID alone.
    let (s, v) = t
        .undo_raw(
            seq,
            Some(json!({ "command_id": id(), "expected_event_id": lost["id"], "epoch": old })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "epoch_mismatch");
    let (s, v) = t
        .undo_raw(
            seq,
            Some(json!({ "command_id": id(), "expected_event_id": lost["id"] })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "event_mismatch");
    assert_eq!(v["error"]["details"]["expected"], lost["id"]);
    assert_eq!(t.server.engine.head_seq(), seq, "nothing was undone");
}

#[tokio::test]
async fn an_undo_must_name_the_event_it_means() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    let task = t.create_task(&list, "A", json!({})).await;
    let r = t.ok(json!({ "cmd": "complete_task", "id": task })).await;
    let seq = first_seq(&r);
    let head = t.server.engine.head_seq();
    let (s, v) = t.undo_raw(seq, Some(json!({ "command_id": id() }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["code"], "expected_event_id_required");
    // Another existing event's ID is a mismatch too.
    let other = t.event_id_at(seq - 1);
    let (s, v) = t
        .undo_raw(
            seq,
            Some(json!({ "command_id": id(), "expected_event_id": other })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "event_mismatch");
    assert_eq!(t.server.engine.head_seq(), head, "nothing was undone");

    let e = epoch(&t).await;
    let cid = id();
    let body = json!({ "command_id": cid, "expected_event_id": t.event_id_at(seq), "epoch": e });
    let (s, u1) = t.undo_raw(seq, Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK, "{u1}");
    let (s, u2) = t.undo_raw(seq, Some(body)).await;
    assert_eq!(s, StatusCode::OK, "a retry replays");
    assert_eq!(u1, u2);
}
