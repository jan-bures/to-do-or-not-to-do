//! The desktop's session against an in-process `todo-server` over real HTTP and SSE: pairing two devices
//! the way the app does, then creating, completing and reordering through the view model's commands,
//! with each device seeing the other's changes, and a restart showing the cache before the network.

use std::time::{Duration, Instant};

use todo_client::Connection;
use todo_client::native::HttpTransport;
use todo_client::pairing::{self, Polled};
use todo_core::State;
use todo_core::ids::{ListId, TaskId};
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
            config: Config {
                server: self.url(),
                device: device_id,
            },
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

    let a = Session::open(net.clone(), &pa.cache(), &pair_a).expect("open a");
    let b = Session::open(net.clone(), &pb.cache(), &pair_b).expect("open b");
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
    let reopened = Session::open(net.clone(), &pa.cache(), &pair_a).expect("reopen");
    let v = reopened.client().view();
    assert_eq!(v.lists[&list].name, "Groceries");
    assert_eq!(order(&v, list), vec![milk, bread]);
    assert!(v.tasks[&milk].is_done());
    drop(reopened);
    drop(b);

    // Signing out removes everything local.
    config::forget(&pa).expect("forget");
    assert!(config::load(&pa).expect("load").is_none());
    assert!(!pa.cache().exists());
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
    let offline = Session::open(net.clone(), &paths.cache(), &pairing).expect("open");
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
    let online = Session::open(net.clone(), &paths.cache(), &pairing).expect("open");
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

#[test]
fn attachment_names_are_made_safe() {
    assert_eq!(safe_file_name("report.pdf"), "report.pdf");
    assert_eq!(safe_file_name("..\\..\\evil.exe"), "evil.exe");
    assert_eq!(safe_file_name("a/b/c:d?.txt"), "c_d_.txt");
    assert_eq!(safe_file_name("CON.txt"), "attachment-CON.txt");
    assert_eq!(safe_file_name("com1"), "attachment-com1");
    assert_eq!(safe_file_name("notes. "), "notes");
    assert_eq!(safe_file_name(""), "attachment");
    assert_eq!(safe_file_name(".."), "attachment");
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
    let _ = config::forget(&paths);
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let pairing = server.pair(&net, &paths);
    let s = Session::open(net.clone(), &paths.cache(), &pairing).expect("open");
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
