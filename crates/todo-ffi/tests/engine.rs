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
use todo_ffi::engine::activity::{ActivityFooter, ActivityLoad, ActivityPager, ActivityView};
use todo_ffi::engine::{Deletion, ListDeletion};
use todo_ffi::engine::{EngineConfig, SyncState};
use todo_ffi::labels::LocalNow;
use todo_ffi::pairing::{self, PairPoll};
use todo_ffi::view::{FieldEdit, RoomKey, SmartList};
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
        let (_, token) = auth.create_device("Approver", None).unwrap();
        let at = auth
            .pairings
            .pending(&started.user_code, auth.now())
            .unwrap()
            .created_at;
        let transport = todo_client::native::HttpTransport::new(&self.url(), &token).unwrap();
        todo_client::device_pairing::review(&transport, &started.user_code, Some(at))
            .await
            .unwrap();
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
        zone_id: "UTC".into(),
        local_day: None,
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
    auth.pairings.deny(&started.user_code, auth.now()).unwrap();
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
fn notes_and_quick_add_preview_use_the_submit_builders_semantics() {
    use todo_ffi::engine::quick_add_date;
    let n = LocalNow {
        epoch_ms: "2026-11-21T12:00:00Z"
            .parse::<todo_core::time::Timestamp>()
            .unwrap()
            .as_millisecond(),
        utc_offset_secs: 0,
        zone_id: "UTC".into(),
        local_day: None,
    };
    let engine = TodoEngine::fixture("blank".into(), n.clone()).unwrap();
    let text = "Sow seeds zítra !!";
    assert_eq!(
        quick_add_date(text.into(), n.clone()).as_deref(),
        Some("Sun 22 Nov")
    );
    assert!(quick_add_date(r"Sow seeds \tomorrow".into(), n.clone()).is_none());
    assert!(quick_add_date("Sow seeds !!".into(), n.clone()).is_none());
    let id = engine.add_task(my_day(), text.into(), n.clone()).unwrap();
    let task = &engine.state().tasks[&id.parse().unwrap()];
    let today = todo_ffi::labels::Now::new(n.clone()).today();
    assert_eq!(task.due, todo_core::time::add_days(today, 1));
    assert_eq!(task.title, "Sow seeds");
    let pending = engine.status().pending;
    for text in ["First line\nSecond line\n", "First line\nSecond line", ""] {
        engine.set_notes(id.clone(), text.into()).unwrap();
        assert_eq!(
            engine.task(id.clone(), n.clone()).unwrap().notes,
            text.trim_end()
        );
    }
    assert_eq!(
        engine.status().pending,
        pending + 2,
        "unchanged notes do not enqueue an edit"
    );
}

#[test]
fn only_http_and_https_source_destinations_cross_the_bridge() {
    use todo_core::model::LinkKind;
    use todo_ffi::labels::Now;
    let n = now();
    let engine = TodoEngine::fixture("world".into(), n.clone()).unwrap();
    let mut state = (*engine.state()).clone();
    let (&link, source) = state.links.iter().next().unwrap();
    let task = source.task.to_string();
    let LinkKind::Email(mail) = source.kind.clone() else {
        panic!("fixture mail")
    };
    let n = Now::new(n);
    for (raw, expected) in [
        ("HTTPS://example.com", Some("https://example.com/")),
        ("http://example.com/path", Some("http://example.com/path")),
        ("intent://example.com", None),
        ("mailto:you@example.com", None),
        ("file:///notes", None),
        ("not a URL", None),
    ] {
        let mut mail = mail.clone();
        mail.web_link = Some(raw.into());
        for kind in [
            LinkKind::Email(mail),
            LinkKind::Web {
                title: Some("Page".into()),
                url: raw.into(),
            },
        ] {
            state.links.get_mut(&link).unwrap().kind = kind;
            let d = todo_ffi::view::detail(&state, &task, &n).unwrap();
            assert_eq!(d.sources.len(), 1);
            assert_eq!(d.sources[0].url.as_deref(), expected);
            if d.sources[0].icon == "link" {
                assert_eq!(d.sources[0].subtitle, raw);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_undo_removes_the_next_occurrence_and_reports_a_conflict_in_plain_language() {
    use todo_core::command::TaskPatch;
    use todo_core::recurrence::{Recurrence, Rule};
    let server = TestServer::start().await;
    let db = tempfile::tempdir().unwrap();
    let engine = TodoEngine::open(EngineConfig {
        server_url: server.url(),
        token: OTHER_TOKEN.into(),
        device_id: "other".into(),
        database_path: db.path().join("cache.sqlite").to_string_lossy().into(),
        allow_loopback_http: true,
    })
    .unwrap();
    let id = engine
        .add_task(my_day(), "Water seedlings today".into(), now())
        .unwrap();
    let task = id.parse().unwrap();
    engine.start().unwrap();
    eventually("creation confirmed", || engine.status().pending == 0).await;
    let remote = &server.running.engine;
    let elsewhere = |seq, cmd| {
        remote
            .execute(
                &Actor::owner("desktop").unwrap(),
                uuid::Uuid::from_u128(seq),
                &cmd,
            )
            .unwrap();
    };
    elsewhere(
        1,
        todo_client::intent::update_task(
            task,
            TaskPatch {
                recurrence: Some(Some(Recurrence::new(Rule::Daily { interval: 1 }))),
                ..Default::default()
            },
        ),
    );
    eventually("repeat rule received", || {
        engine.state().tasks[&task].recurrence.is_some()
    })
    .await;
    let target = engine.set_done(id.clone(), true).unwrap().unwrap();
    eventually("repeat confirmed", || {
        engine.status().pending == 0 && remote.state().tasks.len() == 2
    })
    .await;
    engine.undo_completion(target).unwrap();
    eventually("exact inverse confirmed", || {
        engine.status().pending == 0
            && remote.state().tasks.len() == 1
            && !remote.state().tasks[&task].is_done()
    })
    .await;
    let target = engine.set_done(id, true).unwrap().unwrap();
    eventually("second completion confirmed", || {
        engine.status().pending == 0 && remote.state().tasks.len() == 2
    })
    .await;
    for (seq, done) in [(2, false), (3, true)] {
        elsewhere(
            seq,
            todo_client::intent::set_done(&remote.state(), task, done).unwrap(),
        );
    }
    engine.undo_completion(target).unwrap();
    eventually("conflicting undo refused", || !engine.failed().is_empty()).await;
    assert_eq!(
        engine.failed()[0].reason,
        "Couldn't undo: the task was changed again."
    );
    assert!(remote.state().tasks[&task].is_done());
    engine.stop();
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
    assert_eq!(titles, ["Call the bank", "Buy oat milk"]);
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
    assert_eq!(titles, ["Call the bank", "Buy oat milk"]);
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

    let list = engine
        .create_list(" Garden ".into(), RoomKey::default())
        .unwrap();
    let room = engine.room(list.clone(), now());
    assert_eq!(room.key, list);
    assert_eq!(room.title, "Garden");
    assert_eq!(room.empty_line, "Nothing here yet.");
    assert!(matches!(
        engine.create_list("".into(), RoomKey::default()),
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
    use todo_ffi::view::{Holding, Reopening, RowPlacement};
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let before = engine.room(my_day(), now());
    let home_before = engine.home(now());
    let first = before.tasks[0].id.clone();
    assert_eq!(before.tasks[0].completed_at, None);
    engine.set_done(first.clone(), true).unwrap();
    assert_eq!(engine.task_done(first.clone()), Some(true));

    // Held: still the first open row, and the counts and the drawer are as they were.
    let holding = RowPlacement {
        holding: vec![Holding {
            id: first.clone(),
            agent_update: before.tasks[0].agent_update.clone(),
        }],
        reopening: vec![],
    };
    let held = engine.room_placed(my_day(), String::new(), holding.clone(), now());
    assert_eq!(held.tasks[0].id, first);
    assert_eq!(
        (held.done, held.total, held.open),
        (before.done, before.total, before.open)
    );
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

    // Reopened: it keeps its completed place, exactly as it was, until released, and counts as done.
    engine.set_done(first.clone(), false).unwrap();
    let reopening = RowPlacement {
        holding: vec![],
        reopening: vec![Reopening {
            id: first.clone(),
            completed_at: at,
        }],
    };
    let kept = engine.room_placed(my_day(), String::new(), reopening, now());
    assert_eq!(kept.completed, released.completed);
    assert_eq!(kept.done, released.done);
    assert!(kept.tasks.iter().all(|t| t.id != first));

    // Unknown or malformed IDs and times are ignored; a task that's gone answers None.
    let junk = RowPlacement {
        holding: vec![Holding {
            id: "not-an-id".into(),
            agent_update: None,
        }],
        reopening: vec![Reopening {
            id: first.clone(),
            completed_at: "not a time".into(),
        }],
    };
    assert_eq!(
        engine.room_placed(my_day(), String::new(), junk, now()),
        engine.room(my_day(), now())
    );
    assert_eq!(engine.task_done("not-an-id".into()), None);
}

#[test]
fn agent_activity_keeps_membership_order_and_score_until_a_completion_is_released() {
    use todo_ffi::view::{Holding, RowPlacement};
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let agents = RoomKey::Smart {
        list: SmartList::AgentActivity,
    };
    let filter = "updated_by:agent";
    let before = engine.room_matching(agents.clone(), filter.into(), now());
    let row = before
        .tasks
        .iter()
        .find(|t| !t.done)
        .expect("an open agent task");
    assert!(row.agent_update.is_some());
    engine.set_done(row.id.clone(), true).unwrap();
    let held = RowPlacement {
        holding: vec![Holding {
            id: row.id.clone(),
            agent_update: row.agent_update.clone(),
        }],
        reopening: vec![],
    };
    let placed = engine.room_placed(agents.clone(), filter.into(), held, now());
    assert_eq!(placed, before);
    let released = engine.room_matching(agents, filter.into(), now());
    assert!(released.tasks.iter().all(|t| t.id != row.id));
    assert_eq!(
        (released.done, released.total, released.open),
        (before.done, before.total - 1, before.open - 1)
    );
}

/// The placement carries agent updates, but no row carries an owner stamp or device ID.
#[test]
fn rows_carry_no_device_or_owner_stamp() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let agents = RoomKey::Smart {
        list: SmartList::AgentActivity,
    };
    let id = engine.room(my_day(), now()).tasks[0].id.clone();
    // A change on this device: the owner's, made on "fixture-phone".
    engine.set_done(id, true).unwrap();
    for key in [my_day(), agents] {
        let room = engine.room(key, now());
        for t in room.tasks.iter().chain(&room.completed) {
            let shown = format!("{t:?}");
            assert!(!shown.contains("fixture-phone"), "{shown}");
            assert!(!shown.contains("owner"), "{shown}");
        }
    }
}

#[test]
fn a_fixture_task_is_gone_once_deleted_until_restored() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let id = engine.room(my_day(), now()).tasks[0].id.clone();
    engine.fixture_delete_task(id.clone()).unwrap();
    assert_eq!(engine.task_done(id.clone()), None);
    assert!(engine.task(id.clone(), now()).is_none());
    assert!(
        engine
            .room(my_day(), now())
            .tasks
            .iter()
            .all(|t| t.id != id)
    );
    engine.fixture_restore_task(id.clone()).unwrap();
    assert_eq!(engine.task_done(id.clone()), Some(false));
    assert!(
        engine
            .room(my_day(), now())
            .tasks
            .iter()
            .any(|t| t.id == id)
    );
    assert!(engine.fixture_delete_task("not-an-id".into()).is_err());
}

/// Deleting and restoring are test hooks: an engine on a real cache refuses them.
#[test]
fn only_a_fixture_engine_takes_the_test_hooks() {
    let db = tempfile::tempdir().unwrap();
    let engine = TodoEngine::open(EngineConfig {
        server_url: "https://todo.example.com".into(),
        token: "tdd_not-a-real-token".into(),
        device_id: "phone".into(),
        database_path: db
            .path()
            .join("cache.sqlite")
            .to_string_lossy()
            .into_owned(),
        allow_loopback_http: false,
    })
    .unwrap();
    let id = engine
        .add_task(my_day(), "Water the plants".into(), now())
        .unwrap();
    assert!(matches!(
        engine.fixture_delete_task(id.clone()),
        Err(FfiError::Invalid { .. })
    ));
    assert!(matches!(
        engine.fixture_restore_task(id.clone()),
        Err(FfiError::Invalid { .. })
    ));
    assert_eq!(engine.task_done(id), Some(false));
}

/// The due date, the priority's four levels, and the steps' rename, reorder and delete, offline: each
/// lands in the view at once, and a deleted step comes back with Undo exactly as it was.
#[test]
fn due_dates_priorities_and_step_edits() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let id = engine
        .add_task(my_day(), "Renew the passport".into(), now())
        .unwrap();
    let detail = engine.task(id.clone(), now()).unwrap();
    assert_eq!((detail.due.as_deref(), detail.repeats), (None, false));
    let edits: Vec<_> = detail
        .fields
        .iter()
        .map(|f| (f.label.as_str(), f.edit))
        .collect();
    assert_eq!(
        &edits[..2],
        [
            ("Due", Some(FieldEdit::Due)),
            ("Priority", Some(FieldEdit::Priority))
        ]
    );
    assert!(detail.fields[1].empty);

    engine
        .set_due(id.clone(), Some("2026-12-24".into()))
        .unwrap();
    let detail = engine.task(id.clone(), now()).unwrap();
    assert_eq!(detail.due.as_deref(), Some("2026-12-24"));
    assert!(!detail.fields[0].empty);
    assert!(matches!(
        engine.set_due(id.clone(), Some("24. 12.".into())),
        Err(FfiError::Invalid { .. })
    ));
    let pending = engine.status().pending;
    engine
        .set_due(id.clone(), Some("2026-12-24".into()))
        .unwrap();
    assert_eq!(
        engine.status().pending,
        pending,
        "an unchanged date sends nothing"
    );
    engine.set_due(id.clone(), None).unwrap();
    assert_eq!(engine.task(id.clone(), now()).unwrap().due, None);

    for (marks, name) in [(1, "Low"), (2, "Normal"), (3, "High"), (0, "–")] {
        engine.set_priority(id.clone(), marks).unwrap();
        let detail = engine.task(id.clone(), now()).unwrap();
        assert_eq!(detail.priority, marks);
        assert_eq!(detail.fields[1].value, name);
    }
    assert!(matches!(
        engine.set_priority(id.clone(), 4),
        Err(FfiError::Invalid { .. })
    ));

    let first = engine
        .add_step(id.clone(), "Find the old one".into())
        .unwrap();
    let second = engine.add_step(id.clone(), "Book a photo".into()).unwrap();
    let titles = |e: &TodoEngine| -> Vec<String> {
        e.task(id.clone(), now())
            .unwrap()
            .steps
            .into_iter()
            .map(|s| s.title)
            .collect()
    };
    engine
        .rename_step(first.clone(), " Find the old passport ".into())
        .unwrap();
    assert!(matches!(
        engine.rename_step(first.clone(), "  ".into()),
        Err(FfiError::Invalid { .. })
    ));
    assert!(engine.move_step(second.clone(), -1).unwrap());
    assert!(
        !engine.move_step(second.clone(), -1).unwrap(),
        "already first"
    );
    assert_eq!(titles(&engine), ["Book a photo", "Find the old passport"]);
    engine.set_step_done(second.clone(), true).unwrap();
    let deleted = engine.delete_step(second.clone()).unwrap();
    assert_eq!(titles(&engine), ["Find the old passport"]);
    assert!(matches!(
        engine.delete_step(second.clone()),
        Err(FfiError::Invalid { .. })
    ));
    assert_eq!(engine.undo_deletion(deleted).unwrap(), None);
    let steps = engine.task(id, now()).unwrap().steps;
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.title.as_str(), s.done))
            .collect::<Vec<_>>(),
        [("Book a photo", true), ("Find the old passport", false)]
    );
}

/// Confirms deleting a list with the question the owner saw, expecting it to go.
fn confirm_delete(engine: &TodoEngine, id: &str, asked: String) -> Arc<Deletion> {
    match engine.delete_list(id.to_owned(), asked).unwrap() {
        ListDeletion::Deleted { undo } => undo,
        ListDeletion::Changed { question } => panic!("asked again: {question:?}"),
    }
}

/// A question confirmed after it changed deletes nothing and asks again, with what goes with it now.
#[test]
fn a_stale_delete_question_asks_again_instead_of_deleting() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let RoomKey::List { id } = engine
        .create_list("Garden".into(), RoomKey::default())
        .unwrap()
    else {
        panic!("a list")
    };
    let room = RoomKey::List { id: id.clone() };
    let asked = engine.delete_list_question(id.clone()).unwrap();
    assert_eq!(asked, "Delete the list “Garden”? Undo brings it back.");
    // A task syncs into the list while the question is open.
    engine
        .add_task(room.clone(), "Plant bulbs".into(), now())
        .unwrap();
    let ListDeletion::Changed {
        question: Some(again),
    } = engine.delete_list(id.clone(), asked).unwrap()
    else {
        panic!("deleted with a stale question")
    };
    assert_eq!(
        again,
        "Delete the list “Garden” and its task? Undo brings them back."
    );
    assert_eq!(engine.room(room.clone(), now()).tasks.len(), 1);
    // Renamed meanwhile: asked again too.
    engine.rename_list(id.clone(), "Allotment".into()).unwrap();
    let ListDeletion::Changed {
        question: Some(renamed),
    } = engine.delete_list(id.clone(), again).unwrap()
    else {
        panic!("deleted with a stale name")
    };
    assert!(renamed.contains("“Allotment”"), "{renamed}");
    // Confirmed as it reads now: deleted.
    confirm_delete(&engine, &id, renamed);
    assert!(engine.delete_list_question(id).is_none());
}

fn collection_named(engine: &TodoEngine, name: &str) -> (String, Vec<todo_ffi::view::NavItem>) {
    let group = engine
        .home(now())
        .groups
        .into_iter()
        .find(|g| g.collection.as_ref().is_some_and(|c| c.name == name))
        .unwrap();
    let RoomKey::Collection { id } = group.collection.unwrap().key else {
        panic!("a collection")
    };
    (id, group.lists)
}

fn collection_names(engine: &TodoEngine) -> Vec<String> {
    engine
        .home(now())
        .groups
        .into_iter()
        .filter_map(|g| g.collection.map(|c| c.name))
        .collect()
}

/// Lists and collections: rename, reorder, and a list's delete (asked first, in full) with Undo.
#[test]
fn lists_and_collections_rename_reorder_and_delete() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let (personal, lists) = collection_named(&engine, "Personal");
    let errands = lists.iter().find(|l| l.name == "Errands").unwrap();
    let RoomKey::List { id: errands } = errands.key.clone() else {
        panic!("a list")
    };

    engine
        .rename_list(errands.clone(), " Shopping ".into())
        .unwrap();
    engine
        .rename_collection(personal.clone(), "Home".into())
        .unwrap();
    assert!(matches!(
        engine.rename_list(errands.clone(), "".into()),
        Err(FfiError::Invalid { .. })
    ));
    assert!(matches!(
        engine.rename_collection(personal.clone(), " ".into()),
        Err(FfiError::Invalid { .. })
    ));
    let (_, lists) = collection_named(&engine, "Home");
    assert!(lists.iter().any(|l| l.name == "Shopping"));

    // A collection moves among the collections, a list among its collection's lists, one place at a
    // time, and nothing moves past either end.
    let collection = RoomKey::Collection {
        id: personal.clone(),
    };
    while engine.move_room(collection.clone(), -1).unwrap() {}
    assert_eq!(collection_names(&engine)[0], "Home");
    assert!(engine.move_room(collection.clone(), 1).unwrap());
    assert_eq!(collection_names(&engine)[1], "Home");
    let list = RoomKey::List {
        id: errands.clone(),
    };
    while engine.move_room(list.clone(), 1).unwrap() {}
    let (_, lists) = collection_named(&engine, "Home");
    assert_eq!(lists.last().unwrap().name, "Shopping");
    assert!(engine.move_room(list.clone(), -1).unwrap());
    let (_, lists) = collection_named(&engine, "Home");
    assert_eq!(lists[lists.len() - 2].name, "Shopping");
    assert!(
        !engine.move_room(my_day(), 1).unwrap(),
        "a smart list stays put"
    );

    // Deleting asks in full, then takes the list and its tasks; Undo brings them back.
    let open = engine.room(list.clone(), now());
    let tasks = open.tasks.len() + open.completed.len();
    assert!(tasks > 1);
    let question = engine.delete_list_question(errands.clone()).unwrap();
    assert_eq!(
        question,
        format!("Delete the list “Shopping” and its {tasks} tasks? Undo brings them back.")
    );
    let deleted = confirm_delete(&engine, &errands, question.clone());
    assert!(engine.delete_list_question(errands.clone()).is_none());
    let (_, lists) = collection_named(&engine, "Home");
    assert!(lists.iter().all(|l| l.name != "Shopping"));
    assert!(matches!(
        engine.delete_list(errands.clone(), question).unwrap(),
        ListDeletion::Changed { question: None }
    ));
    assert_eq!(engine.undo_deletion(deleted).unwrap(), None);
    let back = engine.room(list, now());
    assert_eq!(back.tasks.len() + back.completed.len(), tasks);

    // An empty list is asked about without tasks.
    let RoomKey::List { id: empty } = engine
        .create_list("Line\nbreak".into(), RoomKey::default())
        .unwrap()
    else {
        panic!("a list")
    };
    assert_eq!(
        engine.delete_list_question(empty).unwrap(),
        "Delete the list “Line break”? Undo brings it back."
    );
}

/// Undoing a deleted step whose task was deleted meanwhile says why it still doesn't show.
#[test]
fn a_restored_step_under_a_deleted_task_says_why_it_is_hidden() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let id = engine
        .add_task(my_day(), "Sort the receipts".into(), now())
        .unwrap();
    let step = engine.add_step(id.clone(), "By month".into()).unwrap();
    let deleted = engine.delete_step(step).unwrap();
    engine.fixture_delete_task(id).unwrap();
    assert_eq!(
        engine.undo_deletion(deleted).unwrap().as_deref(),
        Some("Step restored, but its task was deleted elsewhere.")
    );
}

#[test]
fn the_drawer_lists_collection_groups_before_loose_lists() {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    engine.create_list("Loose list".into(), my_day()).unwrap();
    let home = engine.home(now());
    let groups: Vec<_> = home
        .groups
        .iter()
        .map(|g| g.collection.as_ref().map(|c| c.name.as_str()))
        .collect();
    assert_eq!(
        groups,
        [Some("Work"), Some("Personal"), Some("Projects"), None]
    );
    assert_eq!(home.groups[0].lists[0].name, "Finance");
    assert_eq!(home.groups[3].lists[0].name, "Loose list");
}

/// Only agents active during the device's local day appear, never an idle row.
#[test]
fn the_drawer_lists_only_agents_active_today() {
    let at = |s: &str| {
        s.parse::<todo_core::time::Timestamp>()
            .unwrap()
            .as_millisecond()
    };
    let local = LocalNow {
        epoch_ms: at("2026-03-29T10:00:00Z"),
        utc_offset_secs: 7200,
        zone_id: "+02:00".into(),
        local_day: None,
    };
    let engine = TodoEngine::fixture("world".into(), local.clone()).unwrap();
    let agents = engine.home(local.clone()).agents;
    let names: Vec<_> = agents
        .iter()
        .map(|a| (a.mark.name.as_str(), a.mark.letter.as_str()))
        .collect();
    assert_eq!(names, [("Claude", "C"), ("Codex", "X"), ("Pi", "π")]);
    assert!(
        agents.iter().all(|a| a.today.ends_with(" edited today")),
        "{agents:?}"
    );
    // After the spring clock change in Prague, the day's actual start used +01:00,
    // not the current +02:00 offset. At 22:30Z on the 28th it was still yesterday.
    use todo_ffi::labels::LocalDay;
    let early = LocalNow {
        epoch_ms: at("2026-03-28T23:15:00Z"),
        ..local.clone()
    };
    let engine = TodoEngine::fixture("world".into(), early).unwrap();
    assert_eq!(engine.home(local.clone()).agents.len(), 3); // fixed +02:00 would include the 22:30Z edits
    let local = LocalNow {
        local_day: Some(LocalDay {
            start_ms: at("2026-03-28T23:00:00Z"),
            end_ms: at("2026-03-29T22:00:00Z"),
        }),
        ..local
    };
    assert!(engine.home(local).agents.is_empty());
}

/// A sent task move, reorder and delete each have a checked inverse. An out-of-band reorder
/// after a move prevents its inverse from overwriting the newer position.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_actions_and_move_conflict_on_the_server() {
    let server = TestServer::start().await;
    let db = tempfile::tempdir().unwrap();
    let e = TodoEngine::open(EngineConfig {
        server_url: server.url(),
        token: OTHER_TOKEN.into(),
        device_id: "other".into(),
        database_path: db.path().join("cache.sqlite").to_string_lossy().into(),
        allow_loopback_http: true,
    })
    .unwrap();
    let source = e.create_list("Garden".into(), my_day()).unwrap();
    let target = e.create_list("Weekend".into(), my_day()).unwrap();
    let RoomKey::List { id: list } = target.clone() else {
        panic!("list")
    };
    let id = e.add_task(source, "Plant bulbs".into(), now()).unwrap();
    let other = e.add_task(target, "Buy tools".into(), now()).unwrap();
    let task: todo_core::ids::TaskId = id.parse().unwrap();
    let before = e.state().tasks[&task].clone();
    let remote = &server.running.engine;
    e.start().unwrap();
    let settled = || eventually("outbox drained", || e.status().pending == 0);
    settled().await;
    let deleted = e.delete_task(id.clone()).unwrap();
    settled().await;
    assert!(!remote.state().is_live(task.into()));
    e.undo_deletion(deleted).unwrap();
    settled().await;
    assert!(remote.state().is_live(task.into()));
    let from = before.list.to_string();
    let moved = e
        .move_task_to(id.clone(), from.clone(), list.clone())
        .unwrap()
        .unwrap();
    settled().await;
    assert_eq!(remote.state().tasks[&task].list.to_string(), list);
    e.undo_task_move(moved).unwrap();
    settled().await;
    assert_eq!(remote.state().tasks[&task].list, before.list);
    assert_eq!(remote.state().tasks[&task].position, before.position);
    let moved = e
        .move_task_to(id.clone(), from, list.clone())
        .unwrap()
        .unwrap();
    settled().await;
    let first = remote.state().tasks[&task].position.clone();
    let reorder = e.reorder_task(list, id, other, 1).unwrap().unwrap();
    settled().await;
    assert_ne!(remote.state().tasks[&task].position, first);
    e.undo_task_move(reorder).unwrap();
    settled().await;
    assert_eq!(remote.state().tasks[&task].position, first);
    let reorder = todo_client::intent::nudge(&remote.state(), task.into(), 1).unwrap();
    remote
        .execute(
            &Actor::owner("desktop").unwrap(),
            uuid::Uuid::from_u128(1),
            &reorder,
        )
        .unwrap();
    let latest = remote.state().tasks[&task].clone();
    e.undo_task_move(moved).unwrap();
    eventually("conflict", || !e.failed().is_empty()).await;
    assert_eq!(e.failed()[0].what, "Undo moving a task");
    assert_eq!(
        e.failed()[0].reason,
        "Couldn't undo: the task was changed again."
    );
    assert_eq!(remote.state().tasks[&task], latest);
    e.stop();
}

async fn load_activity(
    engine: &Arc<TodoEngine>,
    pager: &Arc<ActivityPager>,
    load: ActivityLoad,
) -> ActivityView {
    let (engine, pager) = (engine.clone(), pager.clone());
    tokio::task::spawn_blocking(move || {
        let request = engine.begin_activity(&pager, load).expect("page to read");
        assert!(engine.read_activity(&pager, request).is_none());
        engine.activity_view(&pager, now())
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn organize_and_page_activity_with_exact_event_undo() {
    use todo_ffi::engine::organize::{ColorTarget, OrganizeDeletion, RepeatChoice};
    let server = TestServer::start().await;
    let db = tempfile::tempdir().unwrap();
    let e = TodoEngine::open(EngineConfig {
        server_url: server.url(),
        token: OTHER_TOKEN.into(),
        device_id: "other".into(),
        database_path: db.path().join("cache.sqlite").to_string_lossy().into(),
        allow_loopback_http: true,
    })
    .unwrap();
    let RoomKey::Collection { id: c } = e.create_collection("Garden".into()).unwrap() else {
        panic!("collection")
    };
    let target = ColorTarget::Collection { id: c.clone() };
    let asked = e.organize_question(target.clone()).unwrap();
    let list = e
        .create_list("Beds".into(), RoomKey::Collection { id: c })
        .unwrap();
    let id = e.add_task(list, "Water herbs".into(), now()).unwrap();
    assert!(
        matches!(e.confirm_organize_delete(target,asked).unwrap(),OrganizeDeletion::Changed{question:Some(q)} if q.contains("its list and its task"))
    );
    e.set_repeat(
        id.clone(),
        RepeatChoice::Preset {
            preset: todo_client::intent::repeat::Preset::Weekdays,
        },
        now(),
    )
    .unwrap();
    e.tag_task_named(id.clone(), "herbs".into()).unwrap();
    let change = e
        .add_web_link(id.clone(), "todo.example.com".into())
        .unwrap()
        .unwrap();
    e.start().unwrap();
    eventually("organize sync", || e.status().pending == 0).await;
    let remote = &server.running.engine;
    let task = id.parse().unwrap();
    assert!(remote.state().tasks[&task].recurrence.is_some());
    e.undo_organize(change).unwrap();
    eventually("link undone", || {
        e.status().pending == 0 && e.task(id.clone(), now()).unwrap().sources.is_empty()
    })
    .await;
    // Enough confirmed events for a second page. Only one belongs to the filtered agent.
    remote.set_grants(
        [(
            todo_core::actor::AgentId::new("gardener").unwrap(),
            todo_core::Grants::with_default(todo_core::AccessLevel::Plan),
        )]
        .into(),
    );
    for i in 0..54 {
        let actor = if i == 52 {
            Actor::agent("gardener")
        } else {
            Actor::owner("other")
        }
        .unwrap();
        let cmd = todo_client::intent::retitle(&remote.state(), task, &format!("Water herbs {i}"))
            .unwrap();
        remote
            .execute(&actor, uuid::Uuid::from_u128(i + 1), &cmd)
            .unwrap();
    }
    let all = ActivityPager::new(None);
    let first = load_activity(&e, &all, ActivityLoad::Initial).await;
    let oldest = first
        .days
        .iter()
        .flat_map(|d| &d.rows)
        .map(|r| r.seq)
        .min()
        .unwrap();
    let second = load_activity(&e, &all, ActivityLoad::Older).await;
    assert!(
        second
            .days
            .iter()
            .flat_map(|d| &d.rows)
            .any(|r| r.seq < oldest)
    );
    // A live change marks Refresh but doesn't replace either page. An explicit refresh reads it.
    let before = second
        .days
        .iter()
        .flat_map(|d| &d.rows)
        .map(|r| r.seq)
        .collect::<Vec<_>>();
    let title = "Water the seedlings";
    let update = todo_client::intent::retitle(&remote.state(), task, title).unwrap();
    remote
        .execute(
            &Actor::owner("other").unwrap(),
            uuid::Uuid::from_u128(55),
            &update,
        )
        .unwrap();
    eventually("newer event applied", || {
        e.task(id.clone(), now()).unwrap().title == title
    })
    .await;
    let stale = e.activity_view(&all, now());
    assert_eq!(stale.refresh_label, "Refresh activity •");
    assert_eq!(
        before,
        stale
            .days
            .iter()
            .flat_map(|d| &d.rows)
            .map(|r| r.seq)
            .collect::<Vec<_>>()
    );
    let latest = load_activity(&e, &all, ActivityLoad::Refresh).await;
    assert_eq!(latest.refresh_label, "Refresh activity");
    assert!(latest.days[0].rows[0].seq > before[0]);
    let agent = ActivityPager::new(Some("gardener".into()));
    let filtered = load_activity(&e, &agent, ActivityLoad::Initial).await;
    assert_eq!(filtered.days.len(), 1);
    assert_eq!(filtered.days[0].rows.len(), 1);
    let row = &filtered.days[0].rows[0];
    assert_eq!(row.actor, "Gardener");
    e.stop();
    e.undo_activity_event(&agent, row.seq, row.id.clone())
        .unwrap();
    let view = e.activity_view(&agent, now());
    let pending = &view.days[0].rows[0];
    assert_eq!(pending.note.as_deref(), Some("Undoing…"));
    assert!(pending.undo_label.is_none());
    e.start().unwrap();
    eventually("event undo refused", || !e.failed().is_empty()).await;
    let view = e.activity_view(&agent, now());
    let refused = &view.days[0].rows[0];
    assert_eq!(
        refused.note.as_deref(),
        Some(todo_client::failure::undo_message("undo_conflict"))
    );
    assert_eq!(refused.undo_label.as_deref(), Some("Undo"));
    // Undo the later title edits first, so a checked retry can succeed.
    let owner = Actor::owner("other").unwrap();
    for later in latest
        .days
        .iter()
        .flat_map(|d| &d.rows)
        .filter(|r| r.seq > row.seq)
    {
        remote.undo(&owner, later.seq, false, None).unwrap();
    }
    eventually("conflicting edits undone", || {
        e.task(id.clone(), now()).unwrap().title == "Water herbs 52"
    })
    .await;
    e.undo_activity_event(&agent, row.seq, row.id.clone())
        .unwrap();
    eventually("retry applied", || {
        e.status().pending == 0 && e.task(id.clone(), now()).unwrap().title == "Water herbs 51"
    })
    .await;
    assert_eq!(
        e.failed().len(),
        1,
        "the first refusal is still in the failure list"
    );
    for _ in 0..2 {
        let settled = e.activity_view(&agent, now());
        assert_eq!(settled.days[0].rows[0].note.as_deref(), Some("Undone"));
        assert!(settled.days[0].rows[0].undo_label.is_none());
    }
    e.stop();
}

/// A too-large change and a page with no visible items both return an exclusive server cursor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skipping_an_oversized_change_does_not_stop_at_an_empty_page() {
    use axum::{
        Router,
        http::{StatusCode, Uri},
        routing::get,
    };
    let router = Router::new().route("/api/v1/activity",get(|uri:Uri|async move {match uri.query() {
        None => (StatusCode::INTERNAL_SERVER_ERROR,r#"{"error":{"code":"item_too_large","message":"internal detail","details":{"next_before":15}}}"#),
        Some("before=15") => (StatusCode::OK,r#"{"items":[],"next_before":7}"#),
        Some("before=7") => (StatusCode::OK,r#"{"items":[],"next_before":null}"#),
        _ => (StatusCode::BAD_REQUEST,""),
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let db = tempfile::tempdir().unwrap();
    let e = TodoEngine::open(EngineConfig {
        server_url: format!("http://{addr}"),
        token: OTHER_TOKEN.into(),
        device_id: "other".into(),
        database_path: db.path().join("cache.sqlite").to_string_lossy().into(),
        allow_loopback_http: true,
    })
    .unwrap();
    let pager = ActivityPager::new(None);
    let blocked = load_activity(&e, &pager, ActivityLoad::Initial).await;
    assert_eq!(
        blocked.footer,
        ActivityFooter::Error {
            message: "One change was too large to show.".into(),
            action: "Skip this change".into()
        }
    );
    let older = load_activity(&e, &pager, ActivityLoad::Retry).await;
    assert!(older.days.is_empty());
    assert_eq!(older.footer, ActivityFooter::Older { cursor: 7 });
    let end = load_activity(&e, &pager, ActivityLoad::Older).await;
    assert_eq!(
        end.footer,
        ActivityFooter::Empty {
            label: "No activity yet.".into()
        }
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_restore_tag_name_collision_and_batch_undo() {
    use todo_ffi::engine::organize::{ColorTarget, OrganizeDeletion, RepeatChoice};
    let server = TestServer::start().await;
    let db = tempfile::tempdir().unwrap();
    let e = TodoEngine::open(EngineConfig {
        server_url: server.url(),
        token: OTHER_TOKEN.into(),
        device_id: "other".into(),
        database_path: db.path().join("cache.sqlite").to_string_lossy().into(),
        allow_loopback_http: true,
    })
    .unwrap();
    let collection = e.create_collection("Work".into()).unwrap();
    let RoomKey::Collection { id: c } = &collection else {
        panic!("collection")
    };
    let list = e.create_list("Office".into(), collection.clone()).unwrap();
    let RoomKey::List { id: l } = &list else {
        panic!("list")
    };
    let id = e
        .add_task(list.clone(), "File receipts".into(), now())
        .unwrap();
    e.tag_task_named(id.clone(), "tax".into()).unwrap();
    e.set_repeat(
        id.clone(),
        RepeatChoice::Preset {
            preset: todo_client::intent::repeat::Preset::Daily,
        },
        now(),
    )
    .unwrap();
    let tag = e.task(id.clone(), now()).unwrap().tags[0].id.clone();
    let settle = || eventually("sent", || e.status().pending == 0);
    let delete = |target: ColorTarget| {
        let asked = e.organize_question(target.clone()).unwrap();
        let OrganizeDeletion::Deleted { change } =
            e.confirm_organize_delete(target, asked).unwrap()
        else {
            panic!("deleted")
        };
        change
    };
    e.start().unwrap();
    settle().await;
    let change = delete(ColorTarget::Collection { id: c.clone() });
    assert_eq!(change.toast_text(), "Deleted collection “Work”");
    settle().await;
    assert!(e.task(id.clone(), now()).is_none());
    e.undo_organize(change).unwrap();
    settle().await;
    let remote = server.running.engine.state();
    let list_id = l.parse().unwrap();
    assert_eq!(remote.lists[&list_id].collection, Some(c.parse().unwrap()));
    let task_id = id.parse().unwrap();
    assert!(remote.is_live(todo_core::ids::EntityId::Task(task_id)));
    assert_eq!(remote.tasks[&task_id].list, list_id);
    let change = delete(ColorTarget::Tag { id: tag.clone() });
    assert_eq!(change.toast_text(), "Deleted tag “tax”");
    settle().await;
    e.tag_task_named(id.clone(), "tax".into()).unwrap();
    settle().await;
    e.undo_organize(change).unwrap();
    settle().await;
    assert_eq!(
        e.failed()[0].reason,
        todo_client::failure::undo_message("duplicate_name")
    );
    let tags = e.task(id.clone(), now()).unwrap().tags;
    assert_eq!(tags.len(), 1);
    assert_ne!(tags[0].id, tag);
    e.set_done(id, true).unwrap();
    settle().await;
    e.stop();
    let pager = ActivityPager::new(None);
    let page = load_activity(&e, &pager, ActivityLoad::Initial).await;
    let rows = &page.days[0].rows;
    assert_ne!(rows[0].id, rows[1].id);
    assert!(rows[..2].iter().all(|r| r.undo_label.is_some()));
    for row in &rows[..2] {
        e.undo_activity_event(&pager, row.seq, row.id.clone())
            .unwrap();
    }
    let view = e.activity_view(&pager, now());
    assert!(
        view.days[0].rows[..2]
            .iter()
            .all(|r| r.note.as_deref() == Some("Undoing…"))
    );
    assert_eq!(e.status().pending, 1);
}

/// Deleting a step and a list online, then undoing both: the server has them back, exactly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undoing_a_deletion_restores_it_on_the_server() {
    let server = TestServer::start().await;
    let db = tempfile::tempdir().unwrap();
    let engine = TodoEngine::open(EngineConfig {
        server_url: server.url(),
        token: OTHER_TOKEN.into(),
        device_id: "other".into(),
        database_path: db.path().join("cache.sqlite").to_string_lossy().into(),
        allow_loopback_http: true,
    })
    .unwrap();
    let RoomKey::List { id: list } = engine
        .create_list("Garden".into(), RoomKey::default())
        .unwrap()
    else {
        panic!("a list")
    };
    let id = engine
        .add_task(
            RoomKey::List { id: list.clone() },
            "Plant bulbs".into(),
            now(),
        )
        .unwrap();
    let step = engine.add_step(id.clone(), "Tulips".into()).unwrap();
    engine.start().unwrap();
    eventually("created", || engine.status().pending == 0).await;
    let remote = &server.running.engine;
    let step_id: todo_core::ids::StepId = step.parse().unwrap();
    let list_id: todo_core::ids::ListId = list.parse().unwrap();

    let deleted = engine.delete_step(step.clone()).unwrap();
    eventually("step deleted", || {
        engine.status().pending == 0 && !remote.state().is_live(step_id.into())
    })
    .await;
    engine.undo_deletion(deleted).unwrap();
    eventually("step restored", || {
        engine.status().pending == 0 && remote.state().is_live(step_id.into())
    })
    .await;

    let question = engine.delete_list_question(list.clone()).unwrap();
    let deleted = confirm_delete(&engine, &list, question);
    eventually("list deleted", || {
        engine.status().pending == 0 && !remote.state().is_live(list_id.into())
    })
    .await;
    engine.undo_deletion(deleted).unwrap();
    eventually("list restored", || {
        engine.status().pending == 0 && remote.state().is_live(list_id.into())
    })
    .await;
    assert!(engine.failed().is_empty());
    assert_eq!(engine.task(id, now()).unwrap().steps[0].title, "Tulips");
    engine.stop();
}
