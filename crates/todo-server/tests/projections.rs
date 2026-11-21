#![allow(clippy::unwrap_used)]
//! Projections always equal a fold of the event log: after HTTP traffic, after a restart, and for
//! random command sequences (property test).

mod common;

use common::{TestServer, first_seq, id};
use proptest::prelude::*;
use serde_json::json;
use std::path::Path;
use todo_core::command::{Command, NewTask, Placement, TaskPatch};
use todo_core::ids::{CollectionId, EntityId, ListId, StepId, TagId, TaskId};
use todo_core::model::Priority;
use todo_core::time::Date;
use todo_core::{Actor, State};
use todo_server::db;
use todo_server::engine::{Engine, system_clock};
use uuid::Uuid;

fn rebuilt_dump(path: &Path) -> db::ProjectionDump {
    Engine::rebuild_projections(path).unwrap();
    db::dump_projections(&db::open_reader(path).unwrap()).unwrap()
}

fn assert_projections_match_fold(path: &Path) {
    let conn = db::open_reader(path).unwrap();
    let folded = db::fold_log(&conn).unwrap();
    let projected = db::load_projections(&conn).unwrap();
    assert!(
        projected.data_eq(&folded),
        "projections differ from the fold"
    );
}

#[tokio::test]
async fn rebuilt_projections_equal_live_projections() {
    let t = TestServer::start().await;
    let col = id();
    t.ok(json!({ "cmd": "create_collection", "id": col, "name": "Life" }))
        .await;
    let list = id();
    t.ok(json!({ "cmd": "create_list", "id": list, "collection": col, "name": "Home" }))
        .await;
    let a = t
        .create_task(&list, "A", json!({ "due": "2026-03-01" }))
        .await;
    let b = t.create_task(&list, "B", json!({})).await;
    let tag = id();
    t.ok(json!({ "cmd": "create_tag", "id": tag, "name": "x" }))
        .await;
    t.ok(json!({ "cmd": "add_tag", "task": a, "tag": tag }))
        .await;
    t.ok(json!({ "cmd": "create_step", "id": id(), "task": a, "title": "s" }))
        .await;
    t.ok(json!({ "cmd": "add_reminder", "id": id(), "task": a, "at": "2026-03-01T08:00:00Z" }))
        .await;
    t.ok(
        json!({ "cmd": "add_attachment", "id": id(), "task": b, "file_name": "a.pdf",
                 "media_type": "application/pdf", "size_bytes": 10, "storage_key": "k1" }),
    )
    .await;
    let merged = t
        .ok(json!({ "cmd": "merge_tasks", "source": b, "target": a }))
        .await;
    t.ok(json!({ "cmd": "complete_task", "id": a })).await;
    let undo_me = t.create_task(&list, "Withdrawn", json!({})).await;
    let r = t
        .ok(json!({ "cmd": "update_task", "id": undo_me, "patch": { "title": "W2" } }))
        .await;
    t.undo(first_seq(&r), None).await;
    t.undo(first_seq(&merged), None).await;
    t.ok(json!({ "cmd": "delete", "id": { "kind": "list", "id": list } }))
        .await;

    let live = t.server.engine.dump_projections().unwrap();
    assert!(!live["tasks"].is_empty());
    assert!(!live["task_tags"].is_empty());
    let path = t.db_path();
    let (dir, config) = t.stop().await;
    assert_projections_match_fold(&path);
    assert_eq!(rebuilt_dump(&path), live);
    drop((dir, config));
}

#[tokio::test]
async fn startup_repairs_stale_projections() {
    let t = TestServer::start().await;
    let list = t.create_list("L").await;
    t.create_task(&list, "T", json!({})).await;
    let live = t.server.engine.dump_projections().unwrap();
    let path = t.db_path();
    let (dir, config) = t.stop().await;
    {
        let conn = rusqlite_open(&path);
        conn.execute_batch("DELETE FROM tasks; UPDATE lists SET name = 'tampered';")
            .unwrap();
    }
    let engine = Engine::open(&path, system_clock()).unwrap();
    assert_eq!(engine.dump_projections().unwrap(), live);
    drop((engine, dir, config));
}

fn rusqlite_open(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

#[test]
fn the_event_log_is_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite3");
    let engine = Engine::open(&path, system_clock()).unwrap();
    let actor = Actor::owner("pc").unwrap();
    let list = ListId::from_uuid(Uuid::new_v4());
    engine
        .execute(
            &actor,
            Uuid::new_v4(),
            &Command::CreateList {
                id: list,
                collection: None,
                name: "L".into(),
                color: None,
                placement: Placement::Last,
            },
        )
        .unwrap();
    let conn = rusqlite_open(&path);
    assert!(conn.execute("DELETE FROM events", []).is_err());
    assert!(conn.execute("UPDATE events SET forced = 1", []).is_err());
}

// ---- property test ----------------------------------------------------------------------------------

type Op = (u8, u16, u16, u8);

fn pick<T: Copy>(items: &[T], n: u16) -> Option<T> {
    (!items.is_empty()).then(|| items[usize::from(n) % items.len()])
}

fn live<K: Copy + Into<EntityId>>(s: &State, keys: impl Iterator<Item = K>) -> Vec<K> {
    keys.filter(|k| s.is_live((*k).into())).collect()
}

fn new_uuid() -> Uuid {
    Uuid::new_v4()
}

/// A command (or an undo of a random seq) built from a random op against the current state.
enum Step {
    Run(Command),
    Undo { seq: u64, force: bool },
}

fn build(s: &State, head: u64, (kind, a, b, c): Op) -> Option<Step> {
    let tasks = live(s, s.tasks.keys().copied());
    let lists = live(s, s.lists.keys().copied());
    let cols = live(s, s.collections.keys().copied());
    let tags = live(s, s.tags.keys().copied());
    let steps = live(s, s.steps.keys().copied());
    let cmd = match kind % 16 {
        0 => Command::CreateCollection {
            id: CollectionId::from_uuid(new_uuid()),
            name: format!("c{c}"),
            color: None,
            placement: Placement::Last,
        },
        1 => Command::CreateList {
            id: ListId::from_uuid(new_uuid()),
            collection: if c % 2 == 0 { None } else { pick(&cols, a) },
            name: format!("l{c}"),
            color: None,
            placement: if b % 2 == 0 {
                Placement::First
            } else {
                Placement::Last
            },
        },
        2 | 3 => {
            let mut n = NewTask::new(
                TaskId::from_uuid(new_uuid()),
                pick(&lists, a)?,
                format!("t{c}"),
            );
            if c % 3 == 0 {
                n.due = Date::new(2026, 1 + (c % 12) as i8, 1 + (b % 28) as i8).ok();
            }
            n.priority = [Priority::None, Priority::Low, Priority::High][usize::from(b % 3)];
            Command::CreateTask(Box::new(n))
        }
        4 => Command::UpdateTask {
            id: pick(&tasks, a)?,
            patch: TaskPatch {
                title: (c % 2 == 0).then(|| format!("title {b}")),
                notes: (c % 3 == 0).then(|| format!("notes {b}")),
                ..TaskPatch::default()
            },
        },
        5 => Command::CompleteTask {
            id: pick(&tasks, a)?,
        },
        6 => Command::UncompleteTask {
            id: pick(&tasks, a)?,
        },
        7 => Command::CreateTag {
            id: TagId::from_uuid(new_uuid()),
            name: format!("tag{}", c % 5),
            color: None,
        },
        8 => Command::AddTag {
            task: pick(&tasks, a)?,
            tag: pick(&tags, b)?,
        },
        9 => Command::CreateStep {
            id: StepId::from_uuid(new_uuid()),
            task: pick(&tasks, a)?,
            title: format!("s{c}"),
            placement: Placement::Last,
        },
        10 => {
            let all: Vec<EntityId> = tasks
                .iter()
                .map(|x| (*x).into())
                .chain(lists.iter().map(|x| (*x).into()))
                .chain(tags.iter().map(|x| (*x).into()))
                .chain(steps.iter().map(|x| (*x).into()))
                .collect();
            Command::Delete { id: pick(&all, a)? }
        }
        11 => {
            let dead: Vec<EntityId> = s
                .tasks
                .values()
                .filter(|t| t.deleted.is_some())
                .map(|t| t.id.into())
                .chain(
                    s.lists
                        .values()
                        .filter(|l| l.deleted.is_some())
                        .map(|l| l.id.into()),
                )
                .collect();
            Command::Restore {
                id: pick(&dead, a)?,
            }
        }
        12 => Command::MoveTask {
            id: pick(&tasks, a)?,
            list: pick(&lists, b)?,
            placement: Placement::First,
        },
        13 => Command::MergeTasks {
            source: pick(&tasks, a)?,
            target: pick(&tasks, b)?,
        },
        _ => {
            if head == 0 {
                return None;
            }
            return Some(Step::Undo {
                seq: 1 + u64::from(a) % head,
                force: c % 4 == 0,
            });
        }
    };
    Some(Step::Run(cmd))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn random_histories_rebuild_to_identical_projections(
        ops in prop::collection::vec((any::<u8>(), any::<u16>(), any::<u16>(), any::<u8>()), 1..60)
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite3");
        let engine = Engine::open(&path, system_clock()).unwrap();
        let actor = Actor::owner("pc").unwrap();
        for op in ops {
            let step = {
                let s = engine.state();
                build(&s, engine.head_seq(), op)
            };
            // Rejected commands and refused undos are expected; they must leave no trace.
            match step {
                Some(Step::Run(cmd)) => { let _ = engine.execute(&actor, Uuid::new_v4(), &cmd); }
                Some(Step::Undo { seq, force }) => { let _ = engine.undo(&actor, seq, force, None); }
                None => {}
            }
        }
        let live = engine.dump_projections().unwrap();
        let in_memory = engine.state().clone();
        drop(engine);
        {
            let conn = db::open_reader(&path).unwrap();
            let folded = db::fold_log(&conn).unwrap();
            prop_assert!(folded.data_eq(&in_memory), "in-memory state differs from the fold");
            prop_assert!(db::load_projections(&conn).unwrap().data_eq(&folded));
        }
        prop_assert_eq!(rebuilt_dump(&path), live);
        // Re-opening yields the same state again.
        let reopened = Engine::open(&path, system_clock()).unwrap();
        prop_assert!(reopened.state().data_eq(&in_memory));
    }
}
