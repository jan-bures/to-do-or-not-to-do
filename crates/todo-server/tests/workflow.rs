//! Wire compatibility with replicas deployed before workflow fields existed.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{TOKEN_A, TOKEN_B, TestServer, first_seq, id};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use todo_core::event::Event;
use todo_core::{Actor, State};

/// The old update decoder rejected unfamiliar field kinds. Additive fields in created entities
/// were already ignored. This models the legacy types for the events this test produces.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum LegacyEvent {
    Created,
    Updated { changes: Vec<LegacyChange> },
}

#[derive(Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
enum LegacyChange {
    Name,
    Title,
    Touch,
    Due,
    CompletedAt,
}

fn legacy_changes(event: &Value) -> usize {
    match serde_json::from_value::<LegacyEvent>(event["payload"].clone()).unwrap() {
        LegacyEvent::Created => 0,
        LegacyEvent::Updated { changes } => changes.len(),
    }
}

#[tokio::test]
async fn pre_workflow_events_and_command_hashes_keep_their_original_shape() {
    let t = TestServer::start().await;
    let list = t.create_list("Inbox").await;
    let task = id();
    let command_id = id();
    let command = json!({ "cmd": "create_task", "id": task, "list": list, "title": "Historical" });
    let (code, _, first) = t
        .command_with_id(TOKEN_A, &command_id, command.clone())
        .await;
    assert_eq!(code, StatusCode::OK);
    let epoch = t.server.engine.epoch();
    let t = t.restart_with(|db_path| {
        let conn = todo_server::db::open_reader(db_path).unwrap();
        let stored = todo_server::db::find_command(&conn, &command_id).unwrap().unwrap();
        // Frozen pre-feature canonical command shape (field order matters for idempotency).
        let body = format!(r#"{{"cmd":"create_task","id":"{task}","list":"{list}","title":"Historical","notes":"","due":null,"priority":"none","my_day":null,"recurrence":null,"tags":[],"placement":{{"at":"last"}}}}"#);
        let mut h = Sha256::new();
        h.update(b"command\0");
        h.update(serde_json::to_vec(&Actor::owner("alpha").unwrap()).unwrap());
        h.update(b"\0");
        h.update(body);
        assert_eq!(stored.request_hash, hex::encode(h.finalize()));
    }).await;
    let (code, headers, retry) = t.command_with_id(TOKEN_A, &command_id, command).await;
    assert_eq!(code, StatusCode::OK, "{retry}");
    assert_eq!(headers["idempotent-replayed"], "true");
    assert_eq!(first["events"], retry["events"]);
    assert_eq!(t.server.engine.epoch(), epoch);
    let page = t.get_ok("/api/v1/events?workflow_fields=true").await;
    let entity = &page["events"][1]["payload"]["entity"]["data"];
    assert_eq!(entity["id"], task);
    assert!(entity.get("status").is_none() && entity.get("start_date").is_none());
    let state = t.server.engine.state();
    let id: todo_core::ids::TaskId = task.parse().unwrap();
    assert_eq!(
        state.tasks[&id].status,
        todo_core::model::WorkflowStatus::Todo
    );
    assert!(state.tasks[&id].start_date.is_none());
    let (code, _) = t.undo(first_seq(&first), None).await;
    assert_eq!(code, StatusCode::OK);
}

#[tokio::test]
async fn projected_event_pages_streams_and_activity_keep_legacy_reads_and_edits_working() {
    let t = TestServer::start().await;
    let list = t.create_list("Project").await;
    let task = t
        .create_task(&list, "Draft", json!({ "due": "2030-01-05" }))
        .await;
    let head = t.server.engine.head_seq();
    let mut live = t.sse(TOKEN_B, Some(head)).await;
    let settings = t
        .ok(json!({ "cmd": "update_list", "id": list,
        "features": { "workflow": true, "timeline": true, "default_view": "board" } }))
        .await;
    let edit = t
        .ok(json!({ "cmd": "update_task", "id": task,
        "patch": { "title": "Outline", "status": "in_progress", "start_date": "2030-01-02" } }))
        .await;
    let wait = Duration::from_secs(5);
    for (result, count) in [(&settings, 0), (&edit, 2)] {
        let frame = live.next_event(wait).await.unwrap();
        assert_eq!(frame.seq(), first_seq(result));
        assert_eq!(legacy_changes(&frame.json()), count);
    }
    drop(live);

    let old = t.get_ok("/api/v1/events?since=0").await;
    let full = t
        .get_ok("/api/v1/events?since=0&workflow_fields=true")
        .await;
    let old_items = old["events"].as_array().unwrap();
    let full_items = full["events"].as_array().unwrap();
    assert_eq!(old["next_since"], full["next_since"]);
    assert_eq!(old_items.len(), full_items.len());
    for (old, full) in old_items.iter().zip(full_items) {
        legacy_changes(old);
        for key in ["seq", "id", "command_len", "command_idx", "command_id"] {
            assert_eq!(old[key], full[key]);
        }
    }
    assert!(
        serde_json::from_value::<LegacyEvent>(full_items.last().unwrap()["payload"].clone())
            .is_err()
    );
    let mut replay = t.sse(TOKEN_B, Some(head)).await;
    assert_eq!(replay.next_event(wait).await.unwrap().json(), old_items[2]);
    assert_eq!(replay.next_event(wait).await.unwrap().json(), old_items[3]);
    drop(replay);
    let activity = t.get_ok("/api/v1/activity").await;
    for item in activity["items"].as_array().unwrap() {
        legacy_changes(&item["event"]);
    }

    // An older client submits only fields it knows: not even a stale rename of the list can
    // reset the flags, and a due edit or clear cannot reset the start date or stored stage.
    for command in [
        json!({ "cmd": "update_list", "id": list, "name": "Renamed", "color": null }),
        json!({ "cmd": "update_task", "id": task, "patch": { "title": "Edited offline", "notes": null, "priority": null, "due": null } }),
        json!({ "cmd": "complete_task", "id": task }),
        json!({ "cmd": "uncomplete_task", "id": task }),
    ] {
        let (code, _, result) = t.command_with_id(TOKEN_B, &id(), command).await;
        assert_eq!(code, StatusCode::OK, "{result}");
    }
    let (code, _) = t
        .command(json!({"cmd":"update_task", "id":task, "patch":{"due":"2030-01-01"}}))
        .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    let before = t.server.engine.state();
    // A restart folds the same authoritative event log, never the legacy projection.
    let t = t.restart().await;
    assert_eq!(t.server.engine.state(), before);
    let detail = t.get_ok(&format!("/api/v1/tasks/{task}")).await;
    let task = &detail["task"];
    assert_eq!(task["title"], "Edited offline");
    assert_eq!(task["status"], "in_progress");
    assert_eq!(task["start_date"], "2030-01-02");
    assert!(task["due"].is_null() && task["completed_at"].is_null());
    let lists = t.get_ok("/api/v1/lists").await;
    assert_eq!(lists["items"][0]["features"]["default_view"], "board");
    let full = t
        .get_ok("/api/v1/events?since=0&workflow_fields=true")
        .await;
    let mut state = State::new();
    for event in full["events"].as_array().unwrap() {
        todo_core::apply(
            &mut state,
            &serde_json::from_value::<Event>(event.clone()).unwrap(),
        )
        .unwrap();
    }
    assert!(state.data_eq(&before));
}
