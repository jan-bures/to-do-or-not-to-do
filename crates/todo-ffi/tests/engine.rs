//! The Android engine against an in-process `todo-server`: pairing, the outbox, live updates and the
//! SQLite cache. Never a real server.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use todo_client::native::{HttpTransport, OsEntropy, TokioClock};
use todo_client::{ClientConfig, MemoryStorage, SyncClient};
use todo_core::command::NewTask;
use todo_core::{Actor, Command, DeviceId};
use todo_ffi::engine::{EngineConfig, SyncState};
use todo_ffi::labels::LocalNow;
use todo_ffi::pairing::{self, PairPoll};
use todo_ffi::view::{RoomKey, SmartList};
use todo_ffi::{ChangeListener, FfiError, TodoEngine};
use todo_server::auth::hash_token;
use todo_server::config::OwnerToken;
use todo_server::{Config, Options, RunningServer, Server};

/// The tests' server is plain http on 127.0.0.1, as only a debug build of the app may use.
const LOOPBACK: bool = true;

const OTHER_TOKEN: &str = "ffi-test-token-other-0123456789abcdef";
const WAIT: Duration = Duration::from_secs(20);

struct TestServer {
    running: RunningServer,
    _dir: tempfile::TempDir,
}

impl TestServer {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.path().to_owned(),
            public_origin: Some("https://todo.example.com".into()),
            ..Config::default()
        };
        // A second "device" for changes made elsewhere (the development owner tokens, off by default).
        config.auth.allow_config_owner_tokens = true;
        config.auth.owner_tokens = vec![OwnerToken {
            device: DeviceId::new("other").unwrap(),
            sha256: hash_token(OTHER_TOKEN),
        }];
        config.limits.sse_heartbeat_ms = 200;
        let running = Server::bind(&config, Options::default())
            .await
            .unwrap()
            .spawn()
            .unwrap();
        Self { running, _dir: dir }
    }

    fn url(&self) -> String {
        self.running.url()
    }

    /// Pairs a device the way the app does, approving on the server's side.
    async fn pair(&self) -> (String, String) {
        let started = pairing::start(&self.url(), "Test phone", LOOPBACK)
            .await
            .unwrap();
        assert!(started.approval_url.starts_with(&self.url()));
        assert!(matches!(
            pairing::poll(&self.url(), &started.poll_token, LOOPBACK)
                .await
                .unwrap(),
            PairPoll::Pending { .. }
        ));
        let auth = &self.running.authsvc;
        assert!(
            auth.pairings
                .decide(&started.user_code, true, auth.now())
                .is_some()
        );
        match pairing::poll(&self.url(), &started.poll_token, LOOPBACK)
            .await
            .unwrap()
        {
            PairPoll::Approved { device_id, token } => (device_id, token),
            other => panic!("expected approval, got {other:?}"),
        }
    }
}

fn now() -> LocalNow {
    LocalNow {
        epoch_ms: todo_core::time::Timestamp::now().as_millisecond(),
        utc_offset_secs: 0,
    }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < WAIT, "timed out waiting until {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn my_day() -> RoomKey {
    RoomKey::Smart {
        list: SmartList::MyDay,
    }
}

#[derive(Default)]
struct Counter(AtomicUsize);

impl ChangeListener for Counter {
    fn on_change(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_sync_offline_edits_and_live_updates() {
    let server = TestServer::start().await;
    let (device_id, token) = server.pair().await;
    assert!(token.starts_with("tdd_"));
    let db = tempfile::tempdir().unwrap();
    let config = EngineConfig {
        server_url: server.url(),
        token: token.clone(),
        device_id: device_id.clone(),
        database_path: db
            .path()
            .join("cache.sqlite")
            .to_string_lossy()
            .into_owned(),
        allow_loopback_http: LOOPBACK,
    };
    let engine = TodoEngine::open(config.clone()).unwrap();
    let changes = Arc::new(Counter::default());
    engine.set_listener(changes.clone()).unwrap();
    assert_eq!(engine.status().state, SyncState::Paused);

    // Offline (not started): the task shows at once and waits in the outbox.
    let id = engine
        .add_task(my_day(), "Buy oat milk".into(), now())
        .unwrap();
    let room = engine.room(my_day(), now());
    assert_eq!(room.tasks.len(), 1);
    assert_eq!(room.tasks[0].title, "Buy oat milk");
    assert_eq!(engine.status().pending, 2, "a Tasks list and the task");
    // The listener runs on the engine's runtime, so it hears the change a moment later.
    eventually("the listener hears the edit", || {
        changes.0.load(Ordering::SeqCst) > 0
    })
    .await;

    // Foreground: the outbox drains to the server.
    engine.start().unwrap();
    eventually("the outbox is sent", || engine.status().pending == 0).await;
    let task_id: todo_core::ids::TaskId = id.parse().unwrap();
    assert!(server.running.engine.state().tasks.contains_key(&task_id));
    eventually("online", || engine.status().state == SyncState::Online).await;

    // Complete and rename, then see the server confirm both.
    engine
        .rename(id.clone(), "Buy oat milk (2 l)".into())
        .unwrap();
    engine.set_done(id.clone(), true).unwrap();
    eventually("the server has the edits", || {
        server
            .running
            .engine
            .state()
            .tasks
            .get(&task_id)
            .is_some_and(|t| t.is_done() && t.title == "Buy oat milk (2 l)")
    })
    .await;
    let room = engine.room(my_day(), now());
    assert_eq!((room.done, room.total, room.open), (1, 1, 0));
    assert!(room.finished);

    // A change made elsewhere arrives over the live stream.
    let other = SyncClient::new(
        HttpTransport::new(&server.url(), OTHER_TOKEN).unwrap(),
        TokioClock,
        OsEntropy,
        MemoryStorage::new(),
        ClientConfig::new(Actor::owner("other").unwrap()),
    )
    .unwrap();
    let runner = other.clone();
    let handle = tokio::spawn(async move { runner.run().await });
    eventually("the other device caught up", || {
        other.view().tasks.contains_key(&task_id)
    })
    .await;
    let list = other.view().tasks[&task_id].list;
    let elsewhere = todo_core::ids::TaskId::from_uuid(other.new_uuid());
    other
        .submit(Command::CreateTask(Box::new(NewTask::new(
            elsewhere,
            list,
            "Added on the desktop",
        ))))
        .unwrap();
    let before = changes.0.load(Ordering::SeqCst);
    eventually("the live update arrives", || {
        engine.state().tasks.contains_key(&elsewhere)
    })
    .await;
    eventually("the listener heard it", || {
        changes.0.load(Ordering::SeqCst) > before
    })
    .await;
    other.stop();
    handle.await.unwrap();

    // Background, then a fresh start on the same cache: everything is still there.
    engine.stop();
    assert_eq!(engine.status().state, SyncState::Paused);
    drop(engine);
    let again = TodoEngine::open(config).unwrap();
    let home = again.home(now());
    assert_eq!(home.groups.len(), 1);
    assert_eq!(home.groups[0].lists[0].name, "Tasks");
    assert!(again.state().tasks.contains_key(&elsewhere));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_or_unknown_pairing_gives_no_token() {
    let server = TestServer::start().await;
    let started = pairing::start(&server.url(), "Test phone", LOOPBACK)
        .await
        .unwrap();
    let auth = &server.running.authsvc;
    auth.pairings
        .decide(&started.user_code, false, auth.now())
        .unwrap();
    assert_eq!(
        pairing::poll(&server.url(), &started.poll_token, LOOPBACK)
            .await
            .unwrap(),
        PairPoll::Denied
    );
    assert!(matches!(
        pairing::poll(&server.url(), "no-such-token", LOOPBACK).await,
        Err(FfiError::Expired)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_token_reports_unauthorized() {
    let server = TestServer::start().await;
    let db = tempfile::tempdir().unwrap();
    let engine = TodoEngine::open(EngineConfig {
        server_url: server.url(),
        token: "tdd_not-a-real-token".into(),
        device_id: "phone".into(),
        database_path: db
            .path()
            .join("cache.sqlite")
            .to_string_lossy()
            .into_owned(),
        allow_loopback_http: LOOPBACK,
    })
    .unwrap();
    engine.start().unwrap();
    eventually("the token is refused", || {
        engine.status().problem == Some(todo_ffi::engine::ProblemKind::Unauthorized)
    })
    .await;
    engine.stop();
}

#[test]
fn bad_input_is_refused_before_it_reaches_the_outbox() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    assert!(matches!(
        engine.add_task(my_day(), "   ".into(), now()),
        Err(FfiError::Invalid { .. })
    ));
    assert!(matches!(
        engine.set_done("not-an-id".into(), true),
        Err(FfiError::Invalid { .. })
    ));
    assert!(matches!(
        TodoEngine::fixture("nope".into(), now()),
        Err(FfiError::Invalid { .. })
    ));
    assert_eq!(engine.status().pending, 0);
}

#[test]
fn fixture_commands_apply_offline() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let before = engine.room(my_day(), now());
    let first = before.tasks[0].id.clone();
    engine.set_done(first.clone(), true).unwrap();
    let after = engine.room(my_day(), now());
    assert_eq!(after.done, before.done + 1);
    assert!(after.tasks.iter().all(|t| t.id != first));
    engine.set_my_day(first.clone(), false, now()).unwrap();
    assert_eq!(engine.room(my_day(), now()).total, before.total - 1);
    // Nothing leaves a fixture: it has no server.
    assert_eq!(engine.status().pending, 2);
    assert_eq!(engine.status().state, SyncState::Paused);
}

/// An engine on `path` for a server it never reaches (nothing here starts syncing).
fn offline_config(path: &std::path::Path) -> EngineConfig {
    EngineConfig {
        server_url: "https://todo.example.com".into(),
        token: "tdd_offline".into(),
        device_id: "phone".into(),
        database_path: path.to_string_lossy().into_owned(),
        allow_loopback_http: false,
    }
}

/// Makes the cache refuse to open as damaged (a schema from the future) while its rows stay readable.
fn damage(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.pragma_update(None, "user_version", 999).unwrap();
}

#[test]
fn a_damaged_cache_keeps_its_unsent_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("todo.sqlite");
    let pending = {
        let engine = TodoEngine::open(offline_config(&path)).unwrap();
        assert_eq!(engine.recovered(), None);
        engine
            .add_task(my_day(), "Buy oat milk".into(), now())
            .unwrap();
        engine
            .add_task(my_day(), "Call the bank".into(), now())
            .unwrap();
        engine.status().pending
    };
    assert_eq!(pending, 3, "a Tasks list and two tasks");
    damage(&path);

    let engine = TodoEngine::open(offline_config(&path)).unwrap();
    let recovered = engine.recovered().expect("the cache was set aside");
    assert_eq!((recovered.kept, recovered.lost), (3, 0));
    assert_eq!(engine.status().pending, pending, "nothing unsent was lost");
    let titles: Vec<_> = engine
        .room(my_day(), now())
        .tasks
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(titles, ["Buy oat milk", "Call the bank"]);
    // The damaged copy is kept next to the new cache, and no recovery is left pending.
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n.starts_with("todo.damaged-")),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with(".recovery.json")),
        "{names:?}"
    );
    drop(engine);

    // The next open is an ordinary one.
    let engine = TodoEngine::open(offline_config(&path)).unwrap();
    assert_eq!(engine.recovered(), None);
    assert_eq!(engine.status().pending, pending);
}

/// A cache with three unsent changes (a Tasks list and two tasks), damaged so it won't open.
fn damaged_cache_with_unsent_work(path: &std::path::Path) {
    let engine = TodoEngine::open(offline_config(path)).unwrap();
    engine
        .add_task(my_day(), "Buy oat milk".into(), now())
        .unwrap();
    engine
        .add_task(my_day(), "Call the bank".into(), now())
        .unwrap();
    assert_eq!(engine.status().pending, 3);
    drop(engine);
    damage(path);
}

fn write_record(dir: &std::path::Path, phase: &str) {
    std::fs::write(
        dir.join("todo.sqlite.recovery.json"),
        format!(r#"{{"quarantined":"todo.damaged-1.sqlite","phase":"{phase}"}}"#),
    )
    .unwrap();
}

fn assert_recovered_everything(path: &std::path::Path) {
    let engine = TodoEngine::open(offline_config(path)).unwrap();
    assert_eq!(
        engine.status().pending,
        3,
        "nothing unsent was lost or doubled"
    );
    let titles: Vec<_> = engine
        .room(my_day(), now())
        .tasks
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(titles, ["Buy oat milk", "Call the bank"]);
    assert!(!path.with_file_name("todo.sqlite.recovery.json").exists());
    drop(engine);
    assert_eq!(
        TodoEngine::open(offline_config(path)).unwrap().recovered(),
        None
    );
}

/// A recovery killed at each step boundary is finished by the next open, with every unsent change.
#[test]
fn a_recovery_cut_short_at_any_step_is_finished_by_the_next_open() {
    // Killed while copying: a partial copy, the original untouched.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("todo.sqlite");
    damaged_cache_with_unsent_work(&path);
    write_record(dir.path(), "copying");
    std::fs::write(dir.path().join("todo.damaged-1.sqlite"), b"half a copy").unwrap();
    assert_recovered_everything(&path);

    // Killed after copying, before the original was removed.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("todo.sqlite");
    damaged_cache_with_unsent_work(&path);
    std::fs::copy(&path, dir.path().join("todo.damaged-1.sqlite")).unwrap();
    write_record(dir.path(), "copied");
    assert_recovered_everything(&path);

    // Killed after removing the original, while building the new cache.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("todo.sqlite");
    damaged_cache_with_unsent_work(&path);
    std::fs::rename(&path, dir.path().join("todo.damaged-1.sqlite")).unwrap();
    std::fs::write(dir.path().join("todo.sqlite.recovering"), b"half built").unwrap();
    write_record(dir.path(), "cleared");
    assert_recovered_everything(&path);

    // Killed after the new cache was moved in place, before the record was removed.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("todo.sqlite");
    damaged_cache_with_unsent_work(&path);
    let quarantined = {
        let engine = TodoEngine::open(offline_config(&path)).unwrap();
        assert!(engine.recovered().is_some());
        // The copy itself, not the `-wal` and `-shm` that reading it leaves beside it: directory order
        // is arbitrary (alphabetical on Windows, not on Linux).
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("todo.damaged-") && n.ends_with(".sqlite"))
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        names.into_iter().next().unwrap()
    };
    // Renamed with its sidecars, as one database.
    for suffix in ["", "-wal", "-shm"] {
        let from = dir.path().join(format!("{quarantined}{suffix}"));
        if from.exists() {
            std::fs::rename(
                from,
                dir.path().join(format!("todo.damaged-1.sqlite{suffix}")),
            )
            .unwrap();
        }
    }
    write_record(dir.path(), "cleared");
    assert_recovered_everything(&path);
}

#[test]
fn an_unreadable_cache_is_kept_and_retried_never_replaced_by_an_empty_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("todo.sqlite");
    std::fs::write(
        &path,
        b"this is not an SQLite database, not even its header",
    )
    .unwrap();
    for _ in 0..2 {
        assert!(matches!(
            TodoEngine::open(offline_config(&path)),
            Err(FfiError::Storage { .. })
        ));
        // The recovery is recorded and the damaged bytes are kept, so a retry (or a later fix) can
        // still read them; no empty cache was put in their place.
        assert!(dir.path().join("todo.sqlite.recovery.json").exists());
        assert!(!path.exists());
        let copies = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                let n = e
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .into_owned();
                n.starts_with("todo.damaged-") && n.ends_with(".sqlite")
            })
            .count();
        assert_eq!(copies, 1);
    }
}

#[test]
fn a_release_engine_refuses_plain_http_even_to_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = offline_config(&dir.path().join("todo.sqlite"));
    config.server_url = "http://localhost:47390".into();
    assert!(matches!(
        TodoEngine::open(config.clone()),
        Err(FfiError::BadAddress { .. })
    ));
    config.allow_loopback_http = true;
    assert!(TodoEngine::open(config.clone()).is_ok());
    config.server_url = "http://10.0.2.2:47390".into();
    assert!(matches!(
        TodoEngine::open(config),
        Err(FfiError::BadAddress { .. })
    ));
}

/// The detail's star, steps and a new list, offline: each lands in the view at once and waits in the
/// outbox.
#[test]
fn important_steps_and_new_lists() {
    let dir = tempfile::tempdir().unwrap();
    let engine = TodoEngine::open(offline_config(&dir.path().join("todo.sqlite"))).unwrap();
    let id = engine
        .add_task(my_day(), "Renew the passport".into(), now())
        .unwrap();

    engine.set_important(id.clone(), true).unwrap();
    assert_eq!(engine.task(id.clone(), now()).unwrap().priority, 3);
    let important = RoomKey::Smart {
        list: SmartList::Important,
    };
    assert_eq!(engine.room(important.clone(), now()).tasks.len(), 1);
    engine.set_important(id.clone(), false).unwrap();
    assert_eq!(engine.task(id.clone(), now()).unwrap().priority, 0);
    assert!(engine.room(important, now()).tasks.is_empty());

    let step = engine
        .add_step(id.clone(), "  Find the old one ".into())
        .unwrap();
    engine.add_step(id.clone(), "Book a photo".into()).unwrap();
    assert!(matches!(
        engine.add_step(id.clone(), "   ".into()),
        Err(FfiError::Invalid { .. })
    ));
    engine.set_step_done(step.clone(), true).unwrap();
    let steps = engine.task(id.clone(), now()).unwrap().steps;
    let titles: Vec<_> = steps.iter().map(|s| (s.title.as_str(), s.done)).collect();
    assert_eq!(
        titles,
        [("Find the old one", true), ("Book a photo", false)]
    );
    engine.set_step_done(step, false).unwrap();
    assert!(!engine.task(id, now()).unwrap().steps[0].done);

    let list = engine.create_list(" Garden ".into()).unwrap();
    let room = engine.room(list.clone(), now());
    assert_eq!(room.key, list);
    assert_eq!(room.title, "Garden");
    assert_eq!(room.empty_line, "Nothing here yet.");
    assert!(matches!(
        engine.create_list("".into()),
        Err(FfiError::Invalid { .. })
    ));
    // A Tasks list, the task, its star twice, two steps, a step twice and the new list.
    assert_eq!(engine.status().pending, 9);
}

/// The filter takes the query language, else words in the title; the progress stays the room's.
#[test]
fn a_room_can_be_filtered_with_the_query_language_or_words() {
    let dir = tempfile::tempdir().unwrap();
    let engine = TodoEngine::open(offline_config(&dir.path().join("todo.sqlite"))).unwrap();
    let milk = engine
        .add_task(my_day(), "Buy oat milk".into(), now())
        .unwrap();
    engine
        .add_task(my_day(), "Call the bank".into(), now())
        .unwrap();
    engine.set_important(milk, true).unwrap();

    let all = engine.room_matching(my_day(), "".into(), now());
    assert!(!all.filtered);
    assert_eq!(all.tasks.len(), 2);

    let words = engine.room_matching(my_day(), "OAT".into(), now());
    assert!(words.filtered);
    let titles: Vec<_> = words.tasks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Buy oat milk"]);
    assert_eq!((words.done, words.total, words.open), (0, 2, 2));

    let query = engine.room_matching(my_day(), "priority>=high".into(), now());
    let titles: Vec<_> = query.tasks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Buy oat milk"]);

    let none = engine.room_matching(my_day(), "dentist".into(), now());
    assert!(none.tasks.is_empty());
    assert_eq!(none.empty_line, "Nothing matches the filter.");
}

/// A finished day with nothing due tomorrow still offers *Plan <weekday>*, without a hand-over.
#[test]
fn a_finished_day_offers_planning_even_with_nothing_due_tomorrow() {
    let dir = tempfile::tempdir().unwrap();
    let engine = TodoEngine::open(offline_config(&dir.path().join("todo.sqlite"))).unwrap();
    let id = engine
        .add_task(my_day(), "Water the plants".into(), now())
        .unwrap();
    assert_eq!(engine.room(my_day(), now()).plan_weekday, None);
    engine.set_done(id, true).unwrap();
    let room = engine.room(my_day(), now());
    assert!(room.finished);
    assert!(room.tomorrow.is_none());
    let weekday = room.plan_weekday.expect("the next day's name");
    assert!(weekday.ends_with("day"), "{weekday}");
    // Not while filtering: the hand-over and its button wait.
    let filtered = engine.room_matching(my_day(), "plants".into(), now());
    assert_eq!(filtered.plan_weekday, None);
}

#[test]
fn a_placement_keeps_toggled_rows_where_they_were_until_released() {
    use todo_ffi::view::{Pin, Reopening, RowPlacement};
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let before = engine.room(my_day(), now());
    let home_before = engine.home(now());
    let first = before.tasks[0].id.clone();
    assert_eq!(before.tasks[0].completed_at, None);
    engine.set_done(first.clone(), true).unwrap();
    assert_eq!(engine.task_done(first.clone()), Some(true));

    // Held: still the first open row, and the counts and the drawer are as they were.
    let holding = RowPlacement {
        holding: vec![first.clone()],
        reopening: vec![],
        pinned: vec![],
    };
    let held = engine.room_placed(my_day(), String::new(), holding.clone(), now());
    assert_eq!(held.tasks[0].id, first);
    assert_eq!(
        (held.done, held.total, held.open),
        (before.done, before.total, before.open)
    );
    assert!(held.pinned_only.is_empty());
    assert_eq!(engine.home_placed(holding, now()), home_before);
    // A filter still applies.
    let filtered = engine.room_placed(
        my_day(),
        "zzz-nothing".into(),
        RowPlacement::default(),
        now(),
    );
    assert!(filtered.tasks.is_empty() && filtered.filtered);

    // Released: it's in Completed, with when it was completed.
    let released = engine.room_placed(my_day(), String::new(), RowPlacement::default(), now());
    assert_eq!(released, engine.room(my_day(), now()));
    let done = released
        .completed
        .iter()
        .find(|t| t.id == first)
        .expect("completed");
    let at = done.completed_at.clone().expect("a completion time");

    // Reopened: it keeps its completed place, exactly as it was, until released, and counts as done. My
    // Day selects it anyway, so its pin keeps nothing that the room wouldn't show.
    engine.set_done(first.clone(), false).unwrap();
    let reopening = RowPlacement {
        holding: vec![],
        reopening: vec![Reopening {
            id: first.clone(),
            completed_at: at,
        }],
        pinned: vec![Pin {
            id: first.clone(),
            changed_at: done.changed_at.clone(),
        }],
    };
    let kept = engine.room_placed(my_day(), String::new(), reopening, now());
    assert_eq!(kept.completed, released.completed);
    assert_eq!(kept.done, released.done);
    assert!(kept.tasks.iter().all(|t| t.id != first));
    assert!(kept.pinned_only.is_empty());

    // Unknown or malformed IDs and times are ignored; a task that's gone answers None.
    let junk = RowPlacement {
        holding: vec!["not-an-id".into()],
        reopening: vec![],
        pinned: vec![
            Pin {
                id: "not-an-id".into(),
                changed_at: "not a time".into(),
            },
            Pin {
                id: first.clone(),
                changed_at: "not a time".into(),
            },
        ],
    };
    assert_eq!(
        engine.room_placed(my_day(), String::new(), junk, now()),
        engine.room(my_day(), now())
    );
    assert_eq!(engine.task_done("not-an-id".into()), None);
}

#[test]
fn a_pinned_agent_task_stays_in_agent_activity_in_order_and_no_actor_crosses() {
    use todo_ffi::view::{Pin, Reopening, RoomView, RowPlacement};
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let agents = RoomKey::Smart {
        list: SmartList::AgentActivity,
    };
    let before = engine.room(agents.clone(), now());
    let ids = |r: &RoomView| r.tasks.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
    // An open agent task that isn't the most recent change: a toggle would move it to the top too.
    let row = before
        .tasks
        .iter()
        .skip(1)
        .find(|t| !t.done)
        .expect("an open agent task below the first")
        .clone();
    let pin = Pin {
        id: row.id.clone(),
        changed_at: row.changed_at.clone(),
    };
    engine.set_done(row.id.clone(), true).unwrap();

    // The owner's toggle took it out of Agent activity...
    let truth = engine.room(agents.clone(), now());
    assert!(truth.tasks.iter().all(|t| t.id != row.id));
    assert!(truth.pinned_only.is_empty());
    // ...but held and pinned, the row stays where it was, open, and the room says only the pin keeps it.
    let held = RowPlacement {
        holding: vec![row.id.clone()],
        reopening: vec![],
        pinned: vec![pin.clone()],
    };
    let placed = engine.room_placed(agents.clone(), String::new(), held, now());
    assert_eq!(ids(&placed), ids(&before));
    assert!(!placed.tasks.iter().find(|t| t.id == row.id).unwrap().done);
    assert_eq!(placed.pinned_only, vec![row.id.clone()]);
    assert_eq!(placed.open, before.open);
    // Held without its pin, the room drops it: the pin is what keeps it.
    let unpinned = RowPlacement {
        holding: vec![row.id.clone()],
        reopening: vec![],
        pinned: vec![],
    };
    let placed = engine.room_placed(agents.clone(), String::new(), unpinned, now());
    assert!(placed.tasks.iter().all(|t| t.id != row.id));

    // The completion is reversed, and that is written too: pinned alone (its choreography settling), it
    // stays where it was, open.
    engine.set_done(row.id.clone(), false).unwrap();
    let settling = RowPlacement {
        holding: vec![],
        reopening: vec![],
        pinned: vec![pin.clone()],
    };
    let placed = engine.room_placed(agents.clone(), String::new(), settling, now());
    assert_eq!(ids(&placed), ids(&before));
    assert_eq!(placed.pinned_only, vec![row.id.clone()]);

    // Completed again and reopened from the Completed room: back among Agent activity's rows, done,
    // where it was.
    engine.set_done(row.id.clone(), true).unwrap();
    let done = engine
        .room(
            RoomKey::Smart {
                list: SmartList::Completed,
            },
            now(),
        )
        .tasks
        .into_iter()
        .find(|t| t.id == row.id)
        .expect("completed");
    engine.set_done(row.id.clone(), false).unwrap();
    let reopening = RowPlacement {
        holding: vec![],
        reopening: vec![Reopening {
            id: row.id.clone(),
            completed_at: done.completed_at.clone().unwrap(),
        }],
        pinned: vec![pin],
    };
    let placed = engine.room_placed(agents.clone(), String::new(), reopening, now());
    assert_eq!(ids(&placed), ids(&before));
    assert!(placed.tasks.iter().find(|t| t.id == row.id).unwrap().done);

    // Nothing about who changed a task, or from which device, is in its row: only the time.
    for room in [before, truth, placed] {
        for t in room.tasks.iter().chain(&room.completed) {
            let shown = format!("{t:?}");
            assert!(!shown.contains("fixture-phone"), "{shown}");
            assert!(!shown.contains("owner"), "{shown}");
            assert!(
                t.changed_at.parse::<todo_core::time::Timestamp>().is_ok(),
                "{}",
                t.changed_at
            );
        }
    }
}

#[test]
fn a_deleted_task_is_gone_until_restored() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let id = engine.room(my_day(), now()).tasks[0].id.clone();
    engine.delete_task(id.clone()).unwrap();
    assert_eq!(engine.task_done(id.clone()), None);
    assert!(engine.task(id.clone(), now()).is_none());
    assert!(
        engine
            .room(my_day(), now())
            .tasks
            .iter()
            .all(|t| t.id != id)
    );
    engine.restore_task(id.clone()).unwrap();
    assert_eq!(engine.task_done(id.clone()), Some(false));
    assert!(
        engine
            .room(my_day(), now())
            .tasks
            .iter()
            .any(|t| t.id == id)
    );
    assert!(engine.delete_task("not-an-id".into()).is_err());
}
