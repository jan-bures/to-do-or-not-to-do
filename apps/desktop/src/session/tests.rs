//! The desktop's session against an in-process `todo-server` over real HTTP and SSE: pairing two devices
//! the way the app does, then creating, completing and reordering through the view model's commands,
//! with each device seeing the other's changes, and a restart showing the cache before the network.

use std::time::{Duration, Instant};

use todo_client::Connection;
use todo_client::native::HttpTransport;
use todo_client::pairing::{self, Polled};
use todo_core::State;
use todo_core::ids::{AttachmentId, ListId, TaskId};
use todo_server::{Config as ServerConfig, Options, RunningServer, Server};

use super::*;
use crate::config::{self, Config, Pairing, Paths};
use crate::model::commands;
use crate::model::quick_add::Parsed;
use crate::model::{RoomKey, Smart};

const WAIT: Duration = Duration::from_secs(20);

struct TestServer {
    rt: tokio::runtime::Runtime,
    running: Option<RunningServer>,
    _dir: tempfile::TempDir,
}

impl TestServer {
    fn start() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let dir = tempfile::tempdir().expect("dir");
        let mut cfg = ServerConfig {
            bind: "127.0.0.1:0".parse().expect("addr"),
            data_dir: dir.path().to_owned(),
            ..ServerConfig::default()
        };
        cfg.public_origin = Some("https://todo.example.com".into());
        cfg.limits.sse_heartbeat_ms = 200;
        let running = rt.block_on(async {
            Server::bind(&cfg, Options::default())
                .await
                .expect("bind")
                .spawn()
                .expect("spawn")
        });
        Self {
            rt,
            running: Some(running),
            _dir: dir,
        }
    }

    fn running(&self) -> &RunningServer {
        self.running.as_ref().expect("running")
    }

    fn url(&self) -> String {
        self.running().url()
    }

    fn state(&self) -> std::sync::Arc<State> {
        self.running().engine.state()
    }

    /// Pairs a device the way the pairing screen does, with the owner approving on the server.
    fn pair(&self, net: &Runtime, paths: &Paths) -> Pairing {
        let anon = HttpTransport::anonymous(&self.url()).expect("transport");
        let started = net
            .block_on(pairing::start(&anon, &config::device_name()))
            .expect("start");
        let auth = &self.running().authsvc;
        assert!(
            auth.pairings
                .decide(&started.user_code, true, auth.now())
                .is_some()
        );
        let Polled::Approved { device_id, token } = net
            .block_on(pairing::poll(&anon, &started.poll_token))
            .expect("poll")
        else {
            panic!("expected approval");
        };
        let pairing = Pairing {
            config: Config::new(self.url(), device_id),
            token,
        };
        config::save(paths, &pairing).expect("save");
        config::load(paths).expect("load").expect("paired")
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(r) = self.running.take() {
            let _ = self.rt.block_on(r.shutdown());
        }
    }
}

fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < WAIT, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn submit(s: &Session, cmds: Vec<todo_core::Command>) {
    for c in cmds {
        s.client().submit(c).expect("submit");
    }
}

fn today() -> todo_core::time::Date {
    crate::model::Now::system().today()
}

fn order(state: &State, list: ListId) -> Vec<TaskId> {
    let mut tasks: Vec<_> = state
        .tasks
        .values()
        .filter(|t| t.list == list && t.deleted.is_none())
        .collect();
    tasks.sort_by(|a, b| a.position.cmp(&b.position));
    tasks.iter().map(|t| t.id).collect()
}

#[test]
fn two_paired_devices_create_complete_and_reorder_in_sync() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let homes = (
        tempfile::tempdir().expect("a"),
        tempfile::tempdir().expect("b"),
    );
    let (pa, pb) = (
        Paths::in_dir(homes.0.path().to_owned()),
        Paths::in_dir(homes.1.path().to_owned()),
    );
    let (pair_a, pair_b) = (server.pair(&net, &pa), server.pair(&net, &pb));
    assert_ne!(pair_a.config.device, pair_b.config.device);

    let a = Session::open(net.clone(), &pa, &pair_a).expect("open a");
    let b = Session::open(net.clone(), &pb, &pair_b).expect("open b");
    a.start();
    b.start();

    // A adds two tasks from My Day's quick-add: the default list is created on the way.
    let parsed = |t: &str| Parsed {
        title: t.into(),
        due: None,
        priority: None,
    };
    let room = RoomKey::Smart(Smart::MyDay);
    let (milk, cmds) =
        commands::add_task(&a.client().view(), room, &parsed("Milk"), today(), || {
            a.client().new_uuid()
        });
    submit(&a, cmds);
    let (bread, cmds) =
        commands::add_task(&a.client().view(), room, &parsed("Bread"), today(), || {
            a.client().new_uuid()
        });
    submit(&a, cmds);
    let (milk, bread) = (milk.expect("milk"), bread.expect("bread"));
    // Optimistic: A shows both at once, before the server confirms.
    assert!(a.client().view().tasks.contains_key(&bread));

    eventually("B sees A's tasks", || {
        let v = b.client().view();
        v.tasks.contains_key(&milk) && v.tasks.contains_key(&bread)
    });
    let list = b.client().view().tasks[&milk].list;
    assert_eq!(b.client().view().lists[&list].name, commands::DEFAULT_LIST);
    // Both landed at the top: the newer first.
    assert_eq!(order(&b.client().view(), list), vec![bread, milk]);

    // B completes milk; A sees it.
    let cmd = commands::toggle_complete(&b.client().view(), milk).expect("cmd");
    b.client().submit(cmd).expect("submit");
    eventually("A sees the completion", || {
        a.client().view().tasks[&milk].is_done()
    });

    // A moves milk above bread; B sees the new order.
    let cmd = commands::nudge(&a.client().view(), milk.into(), -1).expect("cmd");
    a.client().submit(cmd).expect("submit");
    eventually("B sees the new order", || {
        order(&b.client().view(), list) == vec![milk, bread]
    });

    // Lists too: B renames the list, A sees it.
    let cmd = commands::rename_list(&b.client().view(), list, "Groceries").expect("cmd");
    b.client().submit(cmd).expect("submit");
    eventually("A sees the rename", || {
        a.client().view().lists[&list].name == "Groceries"
    });

    eventually("both online and converged", || {
        let (sa, sb) = (a.client().status(), b.client().status());
        sa.connection == Connection::Online
            && sb.connection == Connection::Online
            && a.client().view().data_eq(&server.state())
            && b.client().view().data_eq(&server.state())
    });
    // Changes carry the device that made them.
    let device = |p: &Pairing| todo_core::Actor::Owner {
        device: p.config.device.clone(),
    };
    assert_eq!(server.state().tasks[&milk].created.by, device(&pair_a));
    assert_eq!(server.state().tasks[&milk].updated.by, device(&pair_a));

    // A restarts: the cache shows everything before the network is involved.
    drop(a);
    let reopened = Session::open(net.clone(), &pa, &pair_a).expect("reopen");
    let v = reopened.client().view();
    assert_eq!(v.lists[&list].name, "Groceries");
    assert_eq!(order(&v, list), vec![milk, bread]);
    assert!(v.tasks[&milk].is_done());
    drop(reopened);
    drop(b);

    drop(server);
    if let Ok(rt) = std::sync::Arc::try_unwrap(net) {
        rt.shutdown_timeout(Duration::from_secs(5));
    }
}

#[test]
fn edits_made_offline_sync_once_the_server_is_back() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let mut pairing = server.pair(&net, &paths);
    // Point the device at a port nothing listens on: it is offline.
    let real = pairing.config.server.clone();
    pairing.config.server = "http://127.0.0.1:9".into();
    let offline = Session::open(net.clone(), &paths, &pairing).expect("open");
    offline.start();
    let id = ListId::from_uuid(offline.client().new_uuid());
    offline
        .client()
        .submit(commands::create_list(id, None, "Offline list"))
        .expect("queued");
    eventually("offline", || {
        offline.client().status().connection == Connection::Offline
    });
    assert_eq!(offline.client().status().pending, 1);
    drop(offline);

    // Back online (the same cache, the real address): the queued command goes through.
    pairing.config.server = real;
    let online = Session::open(net.clone(), &paths, &pairing).expect("open");
    assert_eq!(
        online.client().status().pending,
        1,
        "the outbox survived the restart"
    );
    online.start();
    eventually("the server has the list", || {
        server.state().lists.contains_key(&id)
    });
    eventually("nothing pending", || online.client().status().pending == 0);
    drop(online);
    drop(server);
    if let Ok(rt) = std::sync::Arc::try_unwrap(net) {
        rt.shutdown_timeout(Duration::from_secs(5));
    }
}

/// Opens a file so nothing else can open or delete it until the handle is dropped, the way another
/// program holding it would.
fn lock(path: &std::path::Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .expect("lock")
}

fn shut_down(net: Runtime) {
    if let Ok(rt) = std::sync::Arc::try_unwrap(net) {
        rt.shutdown_timeout(Duration::from_secs(5));
    }
}

/// A paired session that synced one list, then queued another while offline. Returns the pairing (with
/// the real server address) and the queued list's ID; the session is closed again.
fn queued_offline(server: &TestServer, net: &Runtime, paths: &Paths) -> (Pairing, ListId) {
    let mut pairing = server.pair(net, paths);
    let online = Session::open(net.clone(), paths, &pairing).expect("open");
    online.start();
    let synced = ListId::from_uuid(online.client().new_uuid());
    submit(&online, vec![commands::create_list(synced, None, "Synced")]);
    eventually("synced", || {
        server.state().lists.contains_key(&synced) && online.client().status().pending == 0
    });
    net.block_on(online.shutdown());
    drop(online);
    let real = pairing.config.server.clone();
    pairing.config.server = "http://127.0.0.1:9".into();
    let offline = Session::open(net.clone(), paths, &pairing).expect("open");
    let queued = ListId::from_uuid(offline.client().new_uuid());
    submit(
        &offline,
        vec![commands::create_list(queued, None, "Queued offline")],
    );
    assert_eq!(offline.client().status().pending, 1);
    drop(offline);
    pairing.config.server = real;
    (pairing, queued)
}

#[test]
fn a_cache_that_cant_be_opened_now_is_left_alone() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let (pairing, queued) = queued_offline(&server, &net, &paths);
    let data = paths.data(&pairing.config.pairing);

    // Another program holds the cache: opening fails, and nothing is moved or deleted.
    let held = lock(&data.cache());
    let Err(err) = Session::open_or_recover(net.clone(), &paths, &pairing) else {
        panic!("a locked cache can't be opened");
    };
    assert!(matches!(err, OpenError::Unavailable(_)), "{err}");
    drop(held);
    let names: Vec<_> = std::fs::read_dir(data.path())
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains("damaged")), "{names:?}");

    // Once it's free, the queued edit is still there and reaches the server.
    let (session, recovered) =
        Session::open_or_recover(net.clone(), &paths, &pairing).expect("open");
    assert_eq!(recovered, None);
    assert_eq!(session.client().status().pending, 1);
    session.start();
    eventually("the queued list reached the server", || {
        server.state().lists.contains_key(&queued)
    });
    drop(session);
    drop(server);
    shut_down(net);
}

#[test]
fn a_damaged_cache_is_set_aside_and_its_unsent_edits_kept() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let (pairing, queued) = queued_offline(&server, &net, &paths);
    let data = paths.data(&pairing.config.pairing);

    // Damage the confirmed state; the outbox is intact.
    let conn = rusqlite::Connection::open(data.cache()).expect("sqlite");
    let damaged = conn
        .execute("UPDATE snapshot SET state = 'not json'", [])
        .expect("snapshot")
        + conn
            .execute("UPDATE events SET event = 'not json'", [])
            .expect("events");
    assert!(damaged > 0);
    drop(conn);
    assert!(matches!(
        Session::open_cache(net.clone(), &paths, &pairing),
        Err(OpenError::Corrupt(_))
    ));

    let (session, recovered) =
        Session::open_or_recover(net.clone(), &paths, &pairing).expect("recover");
    let recovered = recovered.expect("recovered");
    assert_eq!((recovered.kept, recovered.lost), (1, 0));
    assert!(
        recovered.quarantined.exists(),
        "the damaged file is kept aside"
    );
    assert_eq!(
        session.client().status().pending,
        1,
        "the queued edit survived"
    );
    session.start();
    eventually("the queued list reached the server", || {
        server.state().lists.contains_key(&queued) && session.client().status().pending == 0
    });
    eventually("the rest came back from the server", || {
        session.client().view().lists.len() == server.state().lists.len()
    });
    drop(session);
    drop(server);
    shut_down(net);
}

#[test]
fn signing_out_removes_the_sign_in_then_the_data() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    session.start();
    let data = session.data().clone();
    std::fs::create_dir_all(data.attachments().join("x")).expect("dir");
    std::fs::write(data.attachments().join("x").join("a.pdf"), b"%PDF").expect("file");

    let stopped = session.shutdown();
    let result = net.block_on(sign_out(
        stopped,
        &paths,
        &data,
        move || drop(session),
        || tokio::time::sleep(Duration::from_millis(100)),
    ));
    assert_eq!(result.expect("signed out"), SignedOut::Clean);
    assert_eq!(config::load(&paths).expect("load"), None);
    assert!(!paths.token().exists() && !paths.config().exists());
    assert!(!data.path().exists(), "cache and downloads are gone");

    // A new pairing starts with nothing of the old one.
    let again = server.pair(&net, &paths);
    assert_ne!(again.config.pairing, pairing.config.pairing);
    let fresh = Session::open(net.clone(), &paths, &again).expect("open");
    assert!(fresh.client().view().lists.is_empty());
    drop(fresh);
    drop(server);
    shut_down(net);
}

#[test]
fn a_sign_in_that_cant_be_removed_keeps_everything() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    let data = session.data().clone();

    let held = lock(&paths.token());
    let released = std::cell::Cell::new(false);
    let result = net.block_on(sign_out(
        session.shutdown(),
        &paths,
        &data,
        || released.set(true),
        || async {},
    ));
    assert!(result.is_err(), "still signed in");
    assert!(!released.get(), "the session is kept");
    assert!(
        std::fs::read_dir(paths.cache_dir.join("pairings"))
            .expect("pairings")
            .all(|e| !e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".signed-out")),
        "the sign-out record was withdrawn"
    );
    assert!(data.cache().exists(), "and so is its data");
    drop(held);
    assert_eq!(config::load(&paths).expect("load"), Some(pairing));
    drop(session);
    drop(server);
    shut_down(net);
}

#[test]
fn a_sign_out_that_cant_be_recorded_removes_nothing() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    let data = session.data().clone();

    // A directory where the record goes: writing the record fails.
    let pairings = paths.cache_dir.join("pairings");
    let record = pairings.join(format!("{}.signed-out", pairing.config.pairing.as_str()));
    std::fs::create_dir(&record).expect("blocker");
    let released = std::cell::Cell::new(false);
    let result = net.block_on(sign_out(
        session.shutdown(),
        &paths,
        &data,
        || released.set(true),
        || async {},
    ));
    let err = result.expect_err("still signed in");
    let notice = err.notice();
    assert!(
        notice.contains("couldn't be recorded") && notice.contains("still signed in"),
        "the status bar says why: {notice}"
    );
    assert!(!released.get(), "the session is kept");
    assert!(paths.token().exists(), "the token is still there");
    assert_eq!(config::load(&paths).expect("load"), Some(pairing.clone()));
    assert!(data.cache().exists(), "and so is the data");
    assert!(
        std::fs::read_dir(&pairings).expect("pairings").all(|e| !e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")),
        "no temporary record is left"
    );

    // Once the record can be written, signing out again works.
    std::fs::remove_dir(&record).expect("unblock");
    let result = net.block_on(sign_out(
        session.shutdown(),
        &paths,
        &data,
        move || drop(session),
        || tokio::time::sleep(Duration::from_millis(100)),
    ));
    assert_eq!(result.expect("signed out"), SignedOut::Clean);
    assert_eq!(config::load(&paths).expect("load"), None);
    assert!(!data.path().exists() && !record.exists());
    drop(server);
    shut_down(net);
}

#[test]
fn data_still_in_use_at_sign_out_is_removed_at_the_next_start() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    let data = session.data().clone();

    // The cache stays open (nothing releases it), so the directory can't go yet.
    let result = net.block_on(sign_out(
        session.shutdown(),
        &paths,
        &data,
        || {},
        || async {},
    ));
    assert!(matches!(result, Ok(SignedOut::DataLeft(_))), "{result:?}");
    assert_eq!(config::load(&paths).expect("load"), None, "signed out");
    drop(session);

    // The next start removes it, and never a current pairing's data.
    let current = server.pair(&net, &paths);
    let kept = Session::open(net.clone(), &paths, &current).expect("open");
    drop(kept);
    assert_eq!(
        config::sweep_signed_out(&paths, Some(&current.config.pairing)),
        1
    );
    assert!(!data.path().exists());
    assert!(paths.data(&current.config.pairing).cache().exists());
    drop(server);
    shut_down(net);
}

#[test]
fn downloads_carry_the_mark_of_the_web_and_only_documents_open() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    session.start();
    let list = ListId::from_uuid(session.client().new_uuid());
    submit(&session, vec![commands::create_list(list, None, "Files")]);
    let (task, cmds) = commands::add_task(
        &session.client().view(),
        RoomKey::List(list),
        &Parsed {
            title: "Has a file".into(),
            ..Parsed::default()
        },
        today(),
        || session.client().new_uuid(),
    );
    let task = task.expect("a task");
    submit(&session, cmds);
    eventually("synced", || server.state().tasks.contains_key(&task));

    // Upload two files the way email ingest would attach them.
    let transport = HttpTransport::new(&pairing.config.server, &pairing.token).expect("transport");
    let upload = |name: &str, body: &[u8]| {
        let res = net
            .block_on(transport.send(Request::post(
                format!("/api/v1/tasks/{task}/attachments?file_name={name}"),
                body.to_vec(),
            )))
            .expect("upload");
        assert!(
            res.status == 200 || res.status == 201,
            "upload: {}",
            res.status
        );
        let v: serde_json::Value = serde_json::from_slice(&res.body).expect("json");
        let id = v["attachment"]["id"].as_str().expect("id").to_string();
        AttachmentId::from_uuid(id.parse().expect("uuid"))
    };
    let pdf = upload("report.pdf", b"%PDF-1.7 test");
    let exe = upload("report.pdf.exe", b"MZ test");

    let opened = net
        .block_on(session.download_to_open(pdf, "report.pdf"))
        .expect("download");
    assert!(opened.starts_with(session.data().attachments()));
    assert_eq!(std::fs::read(&opened).expect("read"), b"%PDF-1.7 test");
    let zone = |p: &std::path::Path| {
        let mut s = p.as_os_str().to_owned();
        s.push(":Zone.Identifier");
        std::fs::read_to_string(std::path::PathBuf::from(s)).expect("zone identifier")
    };
    assert!(zone(&opened).contains("ZoneId=3"));

    // An executable can't be downloaded to be opened (nothing is even fetched), only saved.
    let refused = net.block_on(session.download_to_open(exe, "report.pdf.exe"));
    assert!(refused.is_err());
    let saved_to = home.path().join("saved.pdf.exe");
    std::fs::write(&saved_to, b"an older file").expect("old");
    let saved = net
        .block_on(session.download_to(exe, saved_to.clone()))
        .expect("save");
    assert_eq!(
        saved,
        Saved {
            path: saved_to.clone(),
            marked: true
        }
    );
    assert_eq!(
        std::fs::read(&saved_to).expect("read"),
        b"MZ test",
        "replaced"
    );
    assert!(zone(&saved_to).contains("ZoneId=3"));

    // A download that fails leaves the previous file as it was, and no temporary file.
    let missing = AttachmentId::from_uuid(session.client().new_uuid());
    assert!(
        net.block_on(session.download_to(missing, saved_to.clone()))
            .is_err()
    );
    assert_eq!(std::fs::read(&saved_to).expect("read"), b"MZ test");
    assert!(
        std::fs::read_dir(home.path()).expect("dir").all(|e| !e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .ends_with(".download")),
        "no temporary file left"
    );

    // Every open gets a new owner-only directory; nothing is reused.
    let again = net
        .block_on(session.download_to_open(pdf, "report.pdf"))
        .expect("again");
    assert_ne!(again.parent(), opened.parent());
    todo_fs::verify_private(opened.parent().expect("dir"), todo_fs::Private::Dir)
        .expect("private dir");
    todo_fs::verify_private(&opened, todo_fs::Private::AnyFile).expect("private file");

    drop(session);
    drop(server);
    shut_down(net);
}

fn damage(data: &config::DataDir) {
    let conn = rusqlite::Connection::open(data.cache()).expect("sqlite");
    let damaged = conn
        .execute("UPDATE snapshot SET state = 'not json'", [])
        .expect("snapshot")
        + conn
            .execute("UPDATE events SET event = 'not json'", [])
            .expect("events");
    assert!(damaged > 0);
}

#[test]
fn an_interrupted_recovery_still_keeps_the_unsent_edit() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    for at in [
        "recorded",
        "copied",
        "cleared",
        "salvaging",
        "staged",
        "swapped",
    ] {
        let home = tempfile::tempdir().expect("home");
        let paths = Paths::in_dir(home.path().to_owned());
        let (pairing, queued) = queued_offline(&server, &net, &paths);
        let data = paths.data(&pairing.config.pairing);
        damage(&data);

        // A crash or a full disk at this step: opening fails, and nothing is lost.
        FAIL_AT.with(|f| f.set(Some(at)));
        let failed = Session::open_or_recover(net.clone(), &paths, &pairing);
        FAIL_AT.with(|f| f.set(None));
        assert!(failed.is_err(), "{at}: the injected failure stops it");

        // The next start finishes the job: the queued edit is in the cache, then on the server.
        let (session, _) = Session::open_or_recover(net.clone(), &paths, &pairing)
            .unwrap_or_else(|e| panic!("{at}: {e}"));
        assert_eq!(
            session.client().status().pending,
            1,
            "{at}: the queued edit survived"
        );
        assert!(
            !data.path().join("recovery.json").exists(),
            "{at}: finished"
        );
        session.start();
        eventually("the queued list reached the server", || {
            server.state().lists.contains_key(&queued)
        });
        net.block_on(session.shutdown());
        drop(session);
    }
    drop(server);
    shut_down(net);
}

#[test]
fn a_copy_that_cant_be_read_yet_is_retried_not_taken_as_empty() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let (pairing, queued) = queued_offline(&server, &net, &paths);
    let data = paths.data(&pairing.config.pairing);
    damage(&data);

    // Stop once the damaged cache is copied aside, then hold the copy so it can't be read.
    FAIL_AT.with(|f| f.set(Some("cleared")));
    let failed = Session::open_or_recover(net.clone(), &paths, &pairing);
    FAIL_AT.with(|f| f.set(None));
    assert!(failed.is_err());
    let copy = std::fs::read_dir(data.path())
        .expect("data")
        .map(|e| e.expect("entry").path())
        .find(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("cache.damaged-") && name.ends_with(".sqlite")
        })
        .expect("the quarantined copy");
    let held = lock(&copy);

    // The salvage can't read it: the recovery stops, keeping the record and the copy.
    let failed = Session::open_or_recover(net.clone(), &paths, &pairing);
    assert!(
        matches!(&failed, Err(OpenError::Unavailable(why)) if why.contains("couldn't be read back")),
        "{:?}",
        failed.as_ref().err()
    );
    assert!(
        data.path().join("recovery.json").exists(),
        "the record is kept"
    );
    assert!(copy.exists(), "the copy is kept");
    assert!(
        !data.cache().exists(),
        "no empty cache was put in its place"
    );

    // The next start reads it and keeps the edit.
    drop(held);
    let (session, recovered) =
        Session::open_or_recover(net.clone(), &paths, &pairing).expect("recover");
    assert_eq!(recovered.expect("recovered").kept, 1);
    assert_eq!(
        session.client().status().pending,
        1,
        "the queued edit survived"
    );
    session.start();
    eventually("the queued list reached the server", || {
        server.state().lists.contains_key(&queued)
    });
    net.block_on(session.shutdown());
    drop(session);
    drop(server);
    shut_down(net);
}

#[test]
fn signing_out_stops_downloads_before_removing_the_data() {
    // A server that accepts the connection and never answers: the download hangs until cancelled.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let mut pairing = server.pair(&net, &paths);
    pairing.config.server = format!("http://{}", silent.local_addr().expect("addr"));
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    let id = AttachmentId::from_uuid(session.client().new_uuid());
    let pending = session.download_to_open(id, "report.pdf");
    std::thread::sleep(Duration::from_millis(100));

    let data = session.data().clone();
    let started = Instant::now();
    let result = net.block_on(sign_out(
        session.shutdown(),
        &paths,
        &data,
        || {},
        || tokio::time::sleep(Duration::from_millis(20)),
    ));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it didn't wait for the download"
    );
    assert!(net.block_on(pending).is_err(), "the download was stopped");
    // No download can start any more.
    assert!(
        net.block_on(session.download_to_open(id, "report.pdf"))
            .is_err()
    );
    drop(session);
    // Released only now (the test kept the session): finished at the next start.
    if result.expect("signed out") != SignedOut::Clean {
        assert_eq!(config::sweep_signed_out(&paths, None), 1);
    }
    assert!(!data.path().exists(), "nothing was recreated");
    drop(silent);
    drop(server);
    shut_down(net);
}

#[test]
fn a_save_without_the_mark_still_saves_and_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, b"old").expect("old");
    let marked = save_replacing(&path, b"new", |_| {
        Err(std::io::Error::other("no alternate data streams here"))
    })
    .expect("saved");
    assert!(!marked);
    assert_eq!(std::fs::read(&path).expect("read"), b"new");
    // A failure before the replace leaves the old file and no temporary one.
    let missing_dir = dir.path().join("gone").join("notes.txt");
    assert!(save_replacing(&missing_dir, b"x", mark_of_the_web).is_err());
    assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 1);
}

#[test]
fn a_junction_at_the_attachments_directory_is_refused() {
    let home = tempfile::tempdir().expect("home");
    let outside = tempfile::tempdir().expect("outside");
    let paths = Paths::in_dir(home.path().to_owned());
    let data = paths.data(&config::PairingId::random());
    std::fs::create_dir_all(data.path()).expect("data");
    let ok = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(data.attachments())
        .arg(outside.path())
        .output()
        .expect("mklink")
        .status
        .success();
    assert!(ok);
    assert!(write_to_open(&data, "report.pdf", b"%PDF").is_err());
    assert_eq!(std::fs::read_dir(outside.path()).expect("dir").count(), 0);
}

/// Builds a paired home with a warm cache of about 600 tasks (mostly completed), for measuring cold start:
/// `TODO_BENCH_HOME=<dir> cargo test --release make_a_bench_home -- --ignored`, then
/// `TODO_DESKTOP_HOME=<dir> todo-desktop --bench startup --metrics <file>`. The server is gone by then,
/// so the app starts offline from the cache, which is the case being measured.
#[test]
#[ignore = "builds a benchmark fixture on demand"]
fn make_a_bench_home() {
    let Some(home) = std::env::var_os("TODO_BENCH_HOME") else {
        panic!("set TODO_BENCH_HOME");
    };
    let paths = Paths::in_dir(home.into());
    let _ = config::forget_pairing(&paths);
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let pairing = server.pair(&net, &paths);
    let s = Session::open(net.clone(), &paths, &pairing).expect("open");
    s.start();
    let mut lists = Vec::new();
    for name in [
        "Planning", "Finance", "Errands", "Reading", "Home", "Health",
    ] {
        let id = ListId::from_uuid(s.client().new_uuid());
        submit(&s, vec![commands::create_list(id, None, name)]);
        lists.push(id);
    }
    for i in 0..600 {
        let list = lists[i % lists.len()];
        let parsed = Parsed {
            title: format!("Sample task number {i}"),
            due: None,
            priority: None,
        };
        let room = RoomKey::List(list);
        let (id, cmds) = commands::add_task(&s.client().view(), room, &parsed, today(), || {
            s.client().new_uuid()
        });
        submit(&s, cmds);
        if i % 8 != 0 {
            let cmd = commands::toggle_complete(&s.client().view(), id.expect("id")).expect("cmd");
            submit(&s, vec![cmd]);
        }
    }
    eventually("everything sent", || s.client().status().pending == 0);
    drop(s);
    drop(server);
    if let Ok(rt) = std::sync::Arc::try_unwrap(net) {
        rt.shutdown_timeout(Duration::from_secs(5));
    }
}
