//! The desktop's session against an in-process `todo-server` over real HTTP and SSE: pairing two devices
//! the way the app does, then creating, completing and reordering through the view model's commands,
//! with each device seeing the other's changes, and a restart showing the cache before the network.

use std::time::{Duration, Instant};

use todo_client::native::HttpTransport;
use todo_client::pairing::{self, Polled};
use todo_client::platform::Request;
use todo_client::{Connection, Transport};
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
fn space_twice_after_a_held_completion_queues_undo_then_recompletion_offline() {
    use crate::store::{LiveStore, Store};
    use todo_client::TrackedUndo;
    use todo_client::outbox::Operation;
    use todo_core::Command;

    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    session.start();
    let (task, cmds) = commands::add_task(
        &session.client().view(),
        RoomKey::Smart(Smart::MyDay),
        &Parsed {
            title: "Milk".into(),
            due: None,
            priority: None,
        },
        today(),
        || session.client().new_uuid(),
    );
    let task = task.expect("new task");
    submit(&session, cmds);
    eventually("task confirmed", || session.client().pending().is_empty());
    net.block_on(session.shutdown());
    drop(session);

    // Don't start the new session: we can now reproduce the hold and two presses offline.
    let store = Store::Live(LiveStore::new(
        Session::open(net.clone(), &paths, &pairing).expect("offline cache"),
    ));
    let done = commands::toggle_complete(&store.view(), task).expect("complete command");
    let target = store.submit_undoable(done).expect("completion");
    // Another queued change means the completion is no longer the last unsent command.
    let (another, cmds) = commands::add_task(
        &store.view(),
        RoomKey::Smart(Smart::MyDay),
        &Parsed {
            title: "Bread".into(),
            due: None,
            priority: None,
        },
        today(),
        || store.new_uuid(),
    );
    assert!(another.is_some());
    for cmd in cmds {
        store.submit(cmd).expect("unrelated edit");
    }
    assert!(store.view().tasks[&task].is_done());
    // Space during the hold calls undo, not UncompleteTask. The second Space reads the store.
    let TrackedUndo::Queued(undo) = store.undo(&target).expect("queued undo") else {
        panic!("must not cancel the earlier completion");
    };
    assert!(!store.view().tasks[&task].is_done());
    let again = commands::toggle_complete(&store.view(), task).expect("recompletion");
    assert_eq!(again, Command::CompleteTask { id: task });
    store.submit_undoable(again).expect("queued recompletion");
    assert!(store.view().tasks[&task].is_done());
    let session = store.session().expect("session");
    assert!(
        session
            .client()
            .pending()
            .iter()
            .any(|e| e.command_id == undo && matches!(e.op, Operation::UndoAfter { .. }))
    );
    session.start();
    eventually("both presses settle", || {
        session.client().pending().is_empty()
    });
    assert!(server.state().tasks[&task].is_done());
    assert!(session.client().failed().is_empty());
    net.block_on(session.shutdown());
}

#[test]
fn a_reported_undo_failure_does_not_stay_in_the_failed_queue() {
    use crate::store::{LiveStore, Store};
    use todo_client::outbox::{FailedEntry, Operation, OutboxEntry, Rejection};

    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let store = Store::Live(LiveStore::new(
        Session::open(net.clone(), &paths, &pairing).expect("open"),
    ));
    let client = store.session().expect("session").client();
    for (code, notice) in [
        (
            "undo_conflict",
            "Couldn't undo: the item was changed again.",
        ),
        (
            "duplicate_name",
            "Couldn't undo: another tag already uses that name.",
        ),
        (
            "undo_no_event",
            "Couldn't undo: the original change couldn't be found.",
        ),
    ] {
        let id = store.new_uuid();
        let now = store.now().at;
        client
            .adopt_failed(vec![FailedEntry {
                entry: OutboxEntry {
                    command_id: id,
                    op: Operation::UndoAfter {
                        original: store.new_uuid(),
                    },
                    submitted_at: now,
                    attempts: 0,
                    acked: None,
                    acked_epoch: None,
                },
                failed_at: now,
                rejection: Rejection {
                    status: 409,
                    code: code.into(),
                    message: "Raw server message".into(),
                },
            }])
            .expect("record refused undo");
        assert_eq!(store.undo_result(id), Some(Err(notice.into())));
        assert!(store.dismiss_undo_failure(id));
        assert!(client.failed().is_empty());
    }
    drop(store);
    let reopened = Session::open(net, &paths, &pairing).expect("reopen");
    assert!(reopened.client().failed().is_empty());
}

/// The live lists of a collection, in order.
fn lists_in(state: &State, collection: todo_core::ids::CollectionId) -> Vec<ListId> {
    let mut lists: Vec<_> = state
        .lists
        .values()
        .filter(|l| l.collection == Some(collection) && state.is_live(l.id.into()))
        .collect();
    lists.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
    lists.iter().map(|l| l.id).collect()
}

/// A task's live steps, in order.
fn steps_of(state: &State, task: TaskId) -> Vec<todo_core::ids::StepId> {
    let mut steps: Vec<_> = state
        .steps
        .values()
        .filter(|s| s.task == task && state.is_live(s.id.into()))
        .collect();
    steps.sort_by(|a, b| (&a.position, a.id).cmp(&(&b.position, b.id)));
    steps.iter().map(|s| s.id).collect()
}

/// The tasks My Day shows (its rows), as the window derives them.
fn my_day_rows(store: &crate::store::Store) -> Vec<TaskId> {
    let shell = crate::model::derive(
        &store.view(),
        &crate::model::UiState::default(),
        &store.sync(),
        &store.now(),
    );
    shell.rows().map(|r| r.id).collect()
}

/// Two paired devices on one server. A has a collection of three lists, the middle one coloured, with a
/// task in My Day that has three steps, the middle one done; B has seen all of it.
struct Deletes {
    server: TestServer,
    net: Runtime,
    _homes: (tempfile::TempDir, tempfile::TempDir),
    paths: Paths,
    pairing: Pairing,
    b: Session,
    collection: todo_core::ids::CollectionId,
    lists: [ListId; 3],
    task: TaskId,
    steps: [todo_core::ids::StepId; 3],
}

impl Deletes {
    fn new() -> Self {
        use todo_core::Command;
        use todo_core::command::{NewTask, Placement};
        use todo_core::ids::{CollectionId, StepId};

        let server = TestServer::start();
        let net = runtime().expect("runtime");
        let homes = (
            tempfile::tempdir().expect("a"),
            tempfile::tempdir().expect("b"),
        );
        let paths = Paths::in_dir(homes.0.path().to_owned());
        let paths_b = Paths::in_dir(homes.1.path().to_owned());
        let pairing = server.pair(&net, &paths);
        let pairing_b = server.pair(&net, &paths_b);
        let a = Session::open(net.clone(), &paths, &pairing).expect("open a");
        a.start();
        let id = || a.client().new_uuid();
        let collection = CollectionId::from_uuid(id());
        let lists = [
            ListId::from_uuid(id()),
            ListId::from_uuid(id()),
            ListId::from_uuid(id()),
        ];
        let task = TaskId::from_uuid(id());
        let steps = [
            StepId::from_uuid(id()),
            StepId::from_uuid(id()),
            StepId::from_uuid(id()),
        ];
        let mut cmds = vec![Command::CreateCollection {
            id: collection,
            name: "Home".into(),
            color: None,
            placement: Placement::Last,
        }];
        for (list, (name, color)) in lists.iter().zip([
            ("Errands", None),
            ("Garden", Some("#3a7d44".to_string())),
            ("Repairs", None),
        ]) {
            cmds.push(Command::CreateList {
                id: *list,
                collection: Some(collection),
                name: name.into(),
                color,
                placement: Placement::Last,
            });
        }
        let mut new = NewTask::new(task, lists[1], "Plant the bulbs");
        new.my_day = Some(today());
        cmds.push(Command::CreateTask(Box::new(new)));
        for (step, title) in steps.iter().zip(["Buy bulbs", "Dig the bed", "Water"]) {
            cmds.push(Command::CreateStep {
                id: *step,
                task,
                title: title.into(),
                placement: Placement::Last,
            });
        }
        cmds.push(Command::CompleteStep { id: steps[1] });
        submit(&a, cmds);
        eventually("A's data is confirmed", || a.client().pending().is_empty());
        net.block_on(a.shutdown());
        drop(a);
        let b = Session::open(net.clone(), &paths_b, &pairing_b).expect("open b");
        b.start();
        eventually("B has A's data", || {
            b.client().view().steps.contains_key(&steps[2])
        });
        Self {
            server,
            net,
            _homes: homes,
            paths,
            pairing,
            b,
            collection,
            lists,
            task,
            steps,
        }
    }

    /// Device A's store, offline (nothing is sent until it starts) or syncing.
    fn store(&self, online: bool) -> crate::store::Store {
        use crate::store::{LiveStore, Store};
        let session = Session::open(self.net.clone(), &self.paths, &self.pairing).expect("open a");
        if online {
            session.start();
        }
        Store::Live(LiveStore::new(session))
    }

    /// What must come back exactly: the lists' order, names, colours and collection.
    fn list_settings(&self, state: &State) -> Vec<(String, Option<String>, bool)> {
        lists_in(state, self.collection)
            .iter()
            .map(|l| {
                let list = &state.lists[l];
                (
                    list.name.clone(),
                    list.color.clone(),
                    list.collection == Some(self.collection),
                )
            })
            .collect()
    }

    /// The task's steps in order, each with its title and whether it's done.
    fn step_settings(&self, state: &State) -> Vec<(String, bool)> {
        steps_of(state, self.task)
            .iter()
            .map(|s| {
                let step = &state.steps[s];
                (step.title.clone(), step.completed_at.is_some())
            })
            .collect()
    }

    fn finish(self, store: crate::store::Store) {
        if let Some(session) = store.session() {
            self.net.block_on(session.shutdown());
        }
        drop(store);
        self.net.block_on(self.b.shutdown());
        drop(self.b);
        drop(self.server);
        if let Ok(rt) = std::sync::Arc::try_unwrap(self.net) {
            rt.shutdown_timeout(Duration::from_secs(5));
        }
    }
}

#[test]
fn undoing_an_unsent_list_delete_cancels_it_and_the_list_is_back_as_it_was() {
    use todo_client::TrackedUndo;
    use todo_core::Command;

    let d = Deletes::new();
    let store = d.store(false);
    let before = store.view();
    let garden = d.lists[1];
    let settings = d.list_settings(&before);
    assert_eq!(lists_in(&before, d.collection), d.lists);
    assert!(my_day_rows(&store).contains(&d.task));

    let target = store
        .submit_undoable(Command::Delete { id: garden.into() })
        .expect("delete");
    let deleted = store.view();
    assert_eq!(lists_in(&deleted, d.collection), [d.lists[0], d.lists[2]]);
    // The deleted list's room falls back to My Day, which no longer shows its task.
    assert_eq!(RoomKey::List(garden).resolved(&deleted), RoomKey::default());
    assert!(!my_day_rows(&store).contains(&d.task));

    assert_eq!(store.undo(&target).expect("undo"), TrackedUndo::Cancelled);
    let after = store.view();
    assert!(after.data_eq(&before), "nothing else changed");
    assert_eq!(lists_in(&after, d.collection), d.lists);
    assert_eq!(d.list_settings(&after), settings);
    assert_eq!(
        RoomKey::List(garden).resolved(&after),
        RoomKey::List(garden)
    );
    assert!(my_day_rows(&store).contains(&d.task));
    let session = store.session().expect("session");
    assert!(
        session.client().pending().is_empty(),
        "nothing left to send"
    );
    d.finish(store);
}

#[test]
fn undoing_an_acknowledged_list_delete_restores_it_and_keeps_an_edit_made_meanwhile() {
    use todo_client::TrackedUndo;
    use todo_core::Command;
    use todo_core::command::Placement;

    let d = Deletes::new();
    let store = d.store(true);
    let garden = d.lists[1];
    let settings = d.list_settings(&store.view());
    let target = store
        .submit_undoable(Command::Delete { id: garden.into() })
        .expect("delete");
    let session = store.session().expect("session");
    eventually("the delete is acknowledged", || {
        session.client().pending().is_empty()
    });
    assert!(!d.server.state().is_live(garden.into()));
    assert_eq!(
        RoomKey::List(garden).resolved(&store.view()),
        RoomKey::default()
    );

    // Meanwhile B renames the list beside it and adds another to the collection.
    let added = ListId::from_uuid(d.b.client().new_uuid());
    submit(
        &d.b,
        vec![
            Command::UpdateList {
                id: d.lists[0],
                name: Some("Errands and shopping".into()),
                color: None,
            },
            Command::CreateList {
                id: added,
                collection: Some(d.collection),
                name: "Winter".into(),
                color: None,
                placement: Placement::Last,
            },
        ],
    );
    eventually("A sees B's edits", || {
        store.view().lists.contains_key(&added)
    });

    let TrackedUndo::Queued(_) = store.undo(&target).expect("undo") else {
        panic!("an acknowledged delete is undone through the server");
    };
    // At once, before the server answers.
    assert!(store.view().is_live(garden.into()));
    eventually("the undo is confirmed", || {
        session.client().pending().is_empty()
    });
    assert!(session.client().failed().is_empty());
    let mut expected = settings;
    expected[0].0 = "Errands and shopping".into();
    expected.push(("Winter".into(), None, true));
    for (who, state) in [("server", d.server.state()), ("A", store.view())] {
        assert_eq!(
            lists_in(&state, d.collection),
            [d.lists[0], garden, d.lists[2], added],
            "{who}: order and collection"
        );
        assert_eq!(
            d.list_settings(&state),
            expected,
            "{who}: B's edits survive"
        );
        assert!(state.is_live(d.task.into()), "{who}: its task is back");
    }
    assert_eq!(
        RoomKey::List(garden).resolved(&store.view()),
        RoomKey::List(garden)
    );
    assert!(my_day_rows(&store).contains(&d.task), "back in My Day");
    eventually("B sees the list back", || {
        d.b.client().view().is_live(garden.into())
    });
    d.finish(store);
}

#[test]
fn undoing_an_unsent_step_delete_cancels_it_and_the_step_is_back_as_it_was() {
    use todo_client::TrackedUndo;
    use todo_core::Command;

    let d = Deletes::new();
    let store = d.store(false);
    let before = store.view();
    let settings = d.step_settings(&before);
    assert_eq!(steps_of(&before, d.task), d.steps);
    let target = store
        .submit_undoable(Command::Delete {
            id: d.steps[1].into(),
        })
        .expect("delete");
    assert_eq!(steps_of(&store.view(), d.task), [d.steps[0], d.steps[2]]);
    assert_eq!(store.undo(&target).expect("undo"), TrackedUndo::Cancelled);
    let after = store.view();
    assert!(after.data_eq(&before), "nothing else changed");
    assert_eq!(steps_of(&after, d.task), d.steps);
    assert_eq!(d.step_settings(&after), settings, "still done, same title");
    d.finish(store);
}

#[test]
fn undoing_an_acknowledged_step_delete_restores_it_and_keeps_an_edit_made_meanwhile() {
    use todo_client::TrackedUndo;
    use todo_core::Command;
    use todo_core::command::TaskPatch;

    let d = Deletes::new();
    let store = d.store(true);
    let target = store
        .submit_undoable(Command::Delete {
            id: d.steps[1].into(),
        })
        .expect("delete");
    let session = store.session().expect("session");
    eventually("the delete is acknowledged", || {
        session.client().pending().is_empty()
    });
    // Meanwhile B renames the task and finishes its last step.
    submit(
        &d.b,
        vec![
            Command::UpdateTask {
                id: d.task,
                patch: TaskPatch {
                    title: Some("Plant the tulip bulbs".into()),
                    ..TaskPatch::default()
                },
            },
            Command::CompleteStep { id: d.steps[2] },
        ],
    );
    eventually("A sees B's edits", || {
        store.view().steps[&d.steps[2]].completed_at.is_some()
    });
    let TrackedUndo::Queued(_) = store.undo(&target).expect("undo") else {
        panic!("an acknowledged delete is undone through the server");
    };
    assert_eq!(steps_of(&store.view(), d.task), d.steps, "at once");
    eventually("the undo is confirmed", || {
        session.client().pending().is_empty()
    });
    assert!(session.client().failed().is_empty());
    for (who, state) in [("server", d.server.state()), ("A", store.view())] {
        assert_eq!(steps_of(&state, d.task), d.steps, "{who}: order");
        assert_eq!(
            d.step_settings(&state),
            [
                ("Buy bulbs".to_string(), false),
                ("Dig the bed".to_string(), true),
                ("Water".to_string(), true),
            ],
            "{who}: the step is back done, and B's completion survives"
        );
        assert_eq!(state.tasks[&d.task].title, "Plant the tulip bulbs", "{who}");
    }
    d.finish(store);
}

/// What must come back exactly when a task is restored or moved back: its list, title, My Day, and
/// its steps in order with whether each is done.
fn task_settings(d: &Deletes, state: &State) -> (ListId, String, bool, Vec<(String, bool)>) {
    let task = &state.tasks[&d.task];
    (
        task.list,
        task.title.clone(),
        task.my_day.is_some(),
        d.step_settings(state),
    )
}

#[test]
fn undoing_an_unsent_task_delete_or_move_cancels_it_and_the_task_is_back_as_it_was() {
    use todo_client::TrackedUndo;

    let d = Deletes::new();
    let store = d.store(false);
    let before = store.view();
    let settings = task_settings(&d, &before);
    let garden = d.lists[1];
    let changes = [
        commands::delete_task(&before, d.task).expect("delete"),
        commands::move_to_list(&before, d.task, d.lists[0]).expect("move"),
    ];
    for change in changes {
        let target = store.submit_undoable(change.clone()).expect("change");
        let changed = store.view();
        assert!(
            !changed.is_live(d.task.into()) || changed.tasks[&d.task].list != garden,
            "{change:?}"
        );
        assert!(order(&changed, garden).is_empty(), "gone from its list");
        assert_eq!(store.undo(&target).expect("undo"), TrackedUndo::Cancelled);
        let after = store.view();
        assert!(after.data_eq(&before), "nothing else changed: {change:?}");
        assert_eq!(task_settings(&d, &after), settings);
        assert_eq!(order(&after, garden), [d.task]);
        assert!(my_day_rows(&store).contains(&d.task));
    }
    let session = store.session().expect("session");
    assert!(session.client().pending().is_empty(), "nothing to send");
    d.finish(store);
}

#[test]
fn undoing_an_acknowledged_task_delete_restores_it_and_keeps_an_edit_made_meanwhile() {
    use todo_client::TrackedUndo;

    let d = Deletes::new();
    let store = d.store(true);
    let settings = task_settings(&d, &store.view());
    let delete = commands::delete_task(&store.view(), d.task).expect("delete");
    let target = store.submit_undoable(delete).expect("delete");
    let session = store.session().expect("session");
    eventually("the delete is acknowledged", || {
        session.client().pending().is_empty()
    });
    assert!(!d.server.state().is_live(d.task.into()));
    assert!(!my_day_rows(&store).contains(&d.task));
    // Meanwhile B renames the task's list.
    submit(
        &d.b,
        vec![todo_core::Command::UpdateList {
            id: d.lists[1],
            name: Some("Garden beds".into()),
            color: None,
        }],
    );
    eventually("A sees B's rename", || {
        store.view().lists[&d.lists[1]].name == "Garden beds"
    });
    let TrackedUndo::Queued(_) = store.undo(&target).expect("undo") else {
        panic!("an acknowledged delete is undone through the server");
    };
    assert!(store.view().is_live(d.task.into()), "at once");
    eventually("the undo is confirmed", || {
        session.client().pending().is_empty()
    });
    assert!(session.client().failed().is_empty());
    for (who, state) in [("server", d.server.state()), ("A", store.view())] {
        assert!(state.is_live(d.task.into()), "{who}");
        assert_eq!(task_settings(&d, &state), settings, "{who}: as it was");
        assert_eq!(state.lists[&d.lists[1]].name, "Garden beds", "{who}");
    }
    assert!(my_day_rows(&store).contains(&d.task), "back in My Day");
    eventually("B sees it back", || {
        d.b.client().view().is_live(d.task.into())
    });
    d.finish(store);
}

#[test]
fn undoing_an_acknowledged_move_puts_the_task_back_and_keeps_an_edit_made_meanwhile() {
    use todo_client::TrackedUndo;
    use todo_core::command::TaskPatch;

    let d = Deletes::new();
    let store = d.store(true);
    let (garden, errands) = (d.lists[1], d.lists[0]);
    let moved = commands::move_to_list(&store.view(), d.task, errands).expect("move");
    let target = store.submit_undoable(moved).expect("move");
    let session = store.session().expect("session");
    eventually("the move is acknowledged", || {
        session.client().pending().is_empty()
    });
    assert_eq!(d.server.state().tasks[&d.task].list, errands);
    // Meanwhile B renames the task where it went.
    submit(
        &d.b,
        vec![todo_core::Command::UpdateTask {
            id: d.task,
            patch: TaskPatch {
                title: Some("Plant the tulip bulbs".into()),
                ..TaskPatch::default()
            },
        }],
    );
    eventually("A sees B's rename", || {
        store.view().tasks[&d.task].title == "Plant the tulip bulbs"
    });
    let TrackedUndo::Queued(_) = store.undo(&target).expect("undo") else {
        panic!("an acknowledged move is undone through the server");
    };
    assert_eq!(store.view().tasks[&d.task].list, garden, "at once");
    eventually("the undo is confirmed", || {
        session.client().pending().is_empty()
    });
    assert!(session.client().failed().is_empty());
    for (who, state) in [("server", d.server.state()), ("A", store.view())] {
        assert_eq!(order(&state, garden), [d.task], "{who}: back in its list");
        assert!(order(&state, errands).is_empty(), "{who}");
        let task = &state.tasks[&d.task];
        assert_eq!(
            task.title, "Plant the tulip bulbs",
            "{who}: B's edit survives"
        );
        assert_eq!(d.step_settings(&state).len(), 3, "{who}: with its steps");
    }
    d.finish(store);
}

/// The move keeps the task's key (it goes into an empty list), and B then reorders it there. Undoing
/// the acknowledged move must conflict rather than put the task back in the wrong place.
#[test]
fn undoing_an_acknowledged_move_after_a_reorder_elsewhere_conflicts_and_leaves_the_task_alone() {
    use todo_client::TrackedUndo;
    use todo_core::command::Placement;

    let d = Deletes::new();
    let store = d.store(true);
    let session = store.session().expect("session");
    let (garden, repairs) = (d.lists[1], d.lists[2]);
    // A second task in Garden, after the first.
    let second = TaskId::from_uuid(store.new_uuid());
    store
        .submit(todo_core::Command::CreateTask(Box::new(
            todo_core::command::NewTask {
                placement: Placement::Last,
                ..todo_core::command::NewTask::new(second, garden, "Rake the leaves")
            },
        )))
        .expect("second task");
    assert_eq!(order(&store.view(), garden), [d.task, second]);
    let key = store.view().tasks[&d.task].position.clone();
    let moved = commands::move_to_list(&store.view(), d.task, repairs).expect("move");
    let target = store.submit_undoable(moved).expect("move");
    eventually("the move is acknowledged", || {
        session.client().pending().is_empty()
    });
    assert_eq!(
        d.server.state().tasks[&d.task].position,
        key,
        "into an empty list, it keeps its key"
    );
    // B adds a task to Repairs and puts the moved one after it.
    let other = TaskId::from_uuid(d.b.client().new_uuid());
    eventually("B sees the move", || {
        d.b.client().view().tasks[&d.task].list == repairs
    });
    submit(
        &d.b,
        vec![
            todo_core::Command::CreateTask(Box::new(todo_core::command::NewTask::new(
                other,
                repairs,
                "Fix the gate",
            ))),
            todo_core::Command::Reorder {
                id: d.task.into(),
                placement: Placement::After(other.into()),
            },
        ],
    );
    eventually("A sees B's reorder", || {
        order(&store.view(), repairs) == [other, d.task]
    });
    let undo = match store.undo(&target).expect("undo") {
        TrackedUndo::Queued(undo) => undo,
        other => panic!("an acknowledged move is undone through the server: {other:?}"),
    };
    eventually("the server answers the undo", || {
        store.undo_result(undo).is_some()
    });
    assert_eq!(
        store.undo_result(undo),
        Some(Err("Couldn't undo: the item was changed again.".into()))
    );
    for (who, state) in [("server", d.server.state()), ("A", store.view())] {
        assert_eq!(order(&state, repairs), [other, d.task], "{who}: left alone");
        assert_eq!(order(&state, garden), [second], "{who}");
    }
    d.finish(store);
}

#[test]
fn refused_changes_are_listed_in_plain_words_and_dismissed_for_good() {
    use crate::model::menus::{self, MenuAction, MenuEntry};
    use crate::store::{LiveStore, Store};
    use todo_client::outbox::{FailedEntry, Operation, OutboxEntry, Rejection};

    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let store = Store::Live(LiveStore::new(
        Session::open(net.clone(), &paths, &pairing).expect("open"),
    ));
    let client = store.session().expect("session").client();
    let now = store.now().at;
    let refused = |command, code: &str| FailedEntry {
        entry: OutboxEntry {
            command_id: store.new_uuid(),
            op: Operation::Command { command },
            submitted_at: now,
            attempts: 1,
            acked: None,
            acked_epoch: None,
        },
        failed_at: now,
        rejection: Rejection {
            status: 409,
            code: code.into(),
            message: "Raw server message".into(),
        },
    };
    let task = TaskId::from_uuid(store.new_uuid());
    let list = ListId::from_uuid(store.new_uuid());
    let failed = vec![
        refused(todo_core::Command::Delete { id: task.into() }, "not_found"),
        refused(
            commands::create_list(list, None, "Garden"),
            "duplicate_name",
        ),
        refused(todo_core::Command::CompleteTask { id: task }, "teapot"),
    ];
    let ids: Vec<_> = failed.iter().map(|f| f.entry.command_id).collect();
    client.adopt_failed(failed).expect("record refusals");
    assert_eq!(store.sync().failed, 3);
    let menu = menus::failures_menu(&store.view(), &store.failed());
    let mut rows: Vec<String> = menu
        .iter()
        .filter_map(|e| match e {
            MenuEntry::Failure { what, why, .. } => Some(format!("{what}: {why}")),
            _ => None,
        })
        .collect();
    // Listed by when they were refused; these were refused at the same moment.
    rows.sort();
    assert_eq!(
        rows,
        [
            "Add a list \u{201c}Garden\u{201d}: That name is already taken.",
            "Complete a task: The server refused it.",
            "Delete a task: It no longer exists.",
        ]
    );
    assert!(menus::items(&menu).any(|i| i.action == MenuAction::DismissAllFailures));
    assert_eq!(store.dismiss_failed(ids[1]), Ok(true));
    assert_eq!(store.sync().failed, 2);
    // What was shown, and one refused since, which stays.
    let shown: Vec<_> = store.failed().iter().map(|f| f.entry.command_id).collect();
    client
        .adopt_failed(vec![refused(
            todo_core::Command::UncompleteTask { id: task },
            "deleted",
        )])
        .expect("refused since");
    assert_eq!(store.dismiss_failed_many(&shown), Ok(2));
    assert_eq!(store.sync().failed, 1);
    let later: Vec<_> = store.failed().iter().map(|f| f.entry.command_id).collect();
    assert_eq!(store.dismiss_failed_many(&later), Ok(1));
    assert_eq!(store.sync().failed, 0);
    drop(store);
    let reopened = Session::open(net, &paths, &pairing).expect("reopen");
    assert!(reopened.client().failed().is_empty(), "dismissed for good");
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
    let now = crate::model::Now::fixed("2026-11-21T10:35:00Z").eval();
    let room = RoomKey::Smart(Smart::MyDay);
    let before = a.client().view();
    let ids = [a.client().new_uuid(), a.client().new_uuid()];
    let text = "Pay the invoice by fri !!";
    let mut actual_ids = ids.into_iter();
    let (milk, cmds) =
        commands::quick_add_task(&before, room, text, &now, || actual_ids.next().expect("ID"));
    let mut expected_ids = ids.into_iter();
    let (expected_id, expected_cmds) =
        commands::quick_add_task(&before, commands::TaskRoom::MyDay, text, &now, || {
            expected_ids.next().expect("ID")
        });
    assert_eq!((milk, &cmds), (expected_id, &expected_cmds));
    submit(&a, cmds);
    let (bread, cmds) = commands::quick_add_task(&a.client().view(), room, "Bread", &now, || {
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
    let state = b.client().view();
    assert_eq!(state.lists[&list].name, commands::DEFAULT_LIST);
    assert!(state.lists[&list].color.is_some());
    let task = &state.tasks[&milk];
    assert_eq!(task.title, "Pay the invoice");
    assert_eq!(task.my_day, Some(now.today));
    assert_eq!(task.due, Some("2026-11-27".parse().expect("date")));
    assert_eq!(task.priority, todo_core::model::Priority::Normal);
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
fn a_copy_that_can_never_be_read_can_be_set_aside_to_start_fresh() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let (pairing, queued) = queued_offline(&server, &net, &paths);
    let data = paths.data(&pairing.config.pairing);

    // The cache is garbage: it can be recognised as damaged, but nothing can ever be read back.
    for log in ["cache.sqlite-wal", "cache.sqlite-shm"] {
        let _ = std::fs::remove_file(data.path().join(log));
    }
    std::fs::write(data.cache(), vec![0x5a; 8192]).expect("garbage");
    assert_eq!(Session::pending(&paths, &pairing), None);
    for attempt in ["the start", "Try again"] {
        let failed = Session::open_or_recover(net.clone(), &paths, &pairing);
        assert!(
            matches!(&failed, Err(OpenError::Unavailable(_))),
            "{attempt} fails: {:?}",
            failed.as_ref().err()
        );
        assert_eq!(
            Session::pending(&paths, &pairing),
            Some(Pending::Recovery),
            "{attempt}: start fresh is offered"
        );
    }

    let (session, fresh) =
        Session::start_fresh(net.clone(), &paths, &pairing).expect("start fresh");
    assert_eq!(
        Session::pending(&paths, &pairing),
        None,
        "the records are gone"
    );
    assert!(!data.path().join("recovery.json").exists());
    // The damaged data is kept, with its record, under the pairing's data.
    assert!(
        fresh.kept.starts_with(data.path().join("damaged")),
        "{}",
        fresh.kept.display()
    );
    let kept: Vec<String> = std::fs::read_dir(&fresh.kept)
        .expect("kept")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(kept.iter().any(|n| n == "recovery.json"), "{kept:?}");
    assert!(
        kept.iter()
            .any(|n| n.starts_with("cache.damaged-") && n.ends_with(".sqlite")),
        "{kept:?}"
    );
    assert!(
        !std::fs::read_dir(data.path()).expect("data").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with("cache.damaged-")),
        "nothing damaged is left beside the new cache"
    );

    // A working, synced app on an empty cache; the unsent edit was given up.
    assert_eq!(session.client().status().pending, 0);
    session.start();
    eventually("loaded from the server", || {
        let view = session.client().view();
        !view.lists.is_empty() && view.lists.len() == server.state().lists.len()
    });
    assert!(!server.state().lists.contains_key(&queued));
    net.block_on(session.shutdown());
    drop(session);

    // The next start opens the new cache as usual.
    let (session, recovered) =
        Session::open_or_recover(net.clone(), &paths, &pairing).expect("reopen");
    assert_eq!(recovered, None);

    // Signing out removes the kept copy with the rest of the pairing's data.
    let result = net.block_on(sign_out(
        session.shutdown(),
        &paths,
        &data,
        move || drop(session),
        || tokio::time::sleep(Duration::from_millis(100)),
    ));
    assert_eq!(result.expect("signed out"), SignedOut::Clean);
    assert!(!fresh.kept.exists() && !data.path().exists());
    drop(server);
    shut_down(net);
}

/// A pairing whose cache is garbage, opened once: a recovery is pending that can never finish.
fn unreadable_recovery(
    server: &TestServer,
    net: &Runtime,
    paths: &Paths,
) -> (Pairing, config::DataDir) {
    let (pairing, _) = queued_offline(server, net, paths);
    let data = paths.data(&pairing.config.pairing);
    for log in ["cache.sqlite-wal", "cache.sqlite-shm"] {
        let _ = std::fs::remove_file(data.path().join(log));
    }
    std::fs::write(data.cache(), vec![0x5a; 8192]).expect("garbage");
    assert!(Session::open_or_recover(net.clone(), paths, &pairing).is_err());
    assert_eq!(Session::pending(paths, &pairing), Some(Pending::Recovery));
    (pairing, data)
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn quarantined_name(data: &config::DataDir) -> String {
    names(data.path())
        .into_iter()
        .find(|n| n.starts_with("cache.damaged-") && n.ends_with(".sqlite"))
        .expect("the quarantined copy")
}

fn junction(link: &std::path::Path, target: &std::path::Path) {
    let ok = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .expect("mklink")
        .status
        .success();
    assert!(ok, "mklink /J {}", link.display());
}

#[test]
fn start_fresh_refuses_a_pairing_directory_that_is_a_link() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let paths = Paths::in_dir(home.path().to_owned());
    let (pairing, data) = unreadable_recovery(&server, &net, &paths);

    // The pairing's directory becomes a junction to a directory outside the app's data.
    let outside = elsewhere.path().join("outside");
    std::fs::rename(data.path(), &outside).expect("move away");
    junction(data.path(), &outside);
    let before = names(&outside);

    let refused = Session::start_fresh(net.clone(), &paths, &pairing);
    assert!(refused.is_err(), "refused");
    assert_eq!(
        names(&outside),
        before,
        "nothing outside was created or moved"
    );
    drop(server);
    shut_down(net);
}

#[test]
fn an_interrupted_start_fresh_finishes_into_the_same_folder() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let (pairing, data) = unreadable_recovery(&server, &net, &paths);
    let copy = quarantined_name(&data);
    let wal = format!("{copy}-wal");
    std::fs::write(data.path().join(&wal), b"log").expect("wal");

    // Interrupted after the copy's log moved, before the copy itself.
    FAIL_AT.with(|f| f.set(Some("moved-wal")));
    let failed = Session::start_fresh(net.clone(), &paths, &pairing);
    FAIL_AT.with(|f| f.set(None));
    assert!(failed.is_err());
    assert_eq!(
        Session::pending(&paths, &pairing),
        Some(Pending::StartFresh)
    );
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(data.path().join("start-fresh.json")).expect("record"),
    )
    .expect("json");
    let dest = data
        .path()
        .join("damaged")
        .join(record["dest"].as_str().expect("dest"));
    assert_eq!(
        names(&dest),
        vec![wal.clone()],
        "the log moved, not the copy"
    );

    // Meanwhile nothing opens or salvages the half-moved set.
    let blocked = Session::open_or_recover(net.clone(), &paths, &pairing);
    assert!(
        matches!(&blocked, Err(OpenError::Unavailable(why)) if why.contains("starting fresh")),
        "{:?}",
        blocked.as_ref().err()
    );
    assert!(data.path().join(&copy).exists() && data.path().join("recovery.json").exists());
    assert!(!data.cache().exists(), "no cache was opened");

    // Finishing moves the rest into the same folder, the records last.
    let (session, fresh) = Session::start_fresh(net.clone(), &paths, &pairing).expect("finish");
    assert_eq!(fresh.kept, dest);
    let mut want = vec![
        copy,
        wal,
        "recovery.json".to_string(),
        "start-fresh.json".to_string(),
    ];
    want.sort();
    assert_eq!(names(&dest), want);
    assert_eq!(Session::pending(&paths, &pairing), None);
    assert_eq!(session.client().status().pending, 0);
    drop(session);
    drop(server);
    shut_down(net);
}

#[test]
fn start_fresh_refuses_a_damaged_entry_that_is_not_a_plain_file() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let (pairing, data) = unreadable_recovery(&server, &net, &paths);
    let before = names(data.path());

    // A directory where a staged cache would be.
    let odd = data.path().join("cache.recovering.sqlite");
    std::fs::create_dir(&odd).expect("dir");
    let refused = Session::start_fresh(net.clone(), &paths, &pairing);
    assert!(
        matches!(&refused, Err(OpenError::Unavailable(why)) if why.contains("nothing was set aside")),
        "{:?}",
        refused.as_ref().err()
    );
    let mut after = names(data.path());
    after.retain(|n| n != "cache.recovering.sqlite");
    assert_eq!(after, before, "nothing was moved or created");
    assert_eq!(Session::pending(&paths, &pairing), Some(Pending::Recovery));

    std::fs::remove_dir(&odd).expect("remove");
    let (session, _) = Session::start_fresh(net.clone(), &paths, &pairing).expect("start fresh");
    drop(session);
    drop(server);
    shut_down(net);
}

#[test]
fn a_leftover_sign_out_temp_file_is_replaced_only_if_it_is_a_plain_file() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let pairing = server.pair(&net, &paths);
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    let data = session.data().clone();
    let tmp = paths.cache_dir.join("pairings").join(format!(
        "{}.signed-out.tmp",
        pairing.config.pairing.as_str()
    ));

    // Not a plain file: signing out stops, and it is left alone.
    std::fs::create_dir(&tmp).expect("dir");
    let result = net.block_on(sign_out(
        session.shutdown(),
        &paths,
        &data,
        || {},
        || async {},
    ));
    assert!(result.is_err(), "still signed in");
    assert!(tmp.is_dir(), "not ours to remove");
    assert!(paths.token().exists());

    // A plain file left over is removed (unlinked, never written through: here it is a hard link to a
    // file outside), a new one is created, and signing out goes ahead.
    std::fs::remove_dir(&tmp).expect("remove");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let outside = elsewhere.path().join("keep.txt");
    std::fs::write(&outside, b"keep").expect("outside");
    std::fs::hard_link(&outside, &tmp).expect("hard link");
    let result = net.block_on(sign_out(
        session.shutdown(),
        &paths,
        &data,
        move || drop(session),
        || tokio::time::sleep(Duration::from_millis(100)),
    ));
    assert_eq!(result.expect("signed out"), SignedOut::Clean);
    assert!(!tmp.exists() && !data.path().exists());
    assert_eq!(std::fs::read(&outside).expect("outside"), b"keep");
    drop(server);
    shut_down(net);
}

#[test]
fn connecting_to_an_unavailable_server_uses_the_shared_offline_wording() {
    let server = TestServer::start();
    let net = runtime().expect("runtime");
    let home = tempfile::tempdir().expect("home");
    let paths = Paths::in_dir(home.path().to_owned());
    let mut pairing = server.pair(&net, &paths);

    // Reserve a port without listening, so the connection fails before a request can be sent.
    let socket = tokio::net::TcpSocket::new_v4().expect("socket");
    socket
        .bind("127.0.0.1:0".parse().expect("loopback"))
        .expect("bind");
    pairing.config.server = format!("http://{}", socket.local_addr().expect("address"));
    let session = Session::open(net.clone(), &paths, &pairing).expect("open");
    let id = AttachmentId::from_uuid(session.client().new_uuid());
    let error = net
        .block_on(session.download_to_open(id, "report.pdf"))
        .expect_err("the download fails");
    assert_eq!(error.to_string(), todo_client::failure::ATTACHMENT_OFFLINE);
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
