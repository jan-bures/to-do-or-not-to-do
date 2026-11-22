//! The sync client's loops against a mock transport, on a single-threaded executor and without a
//! network or a runtime.
#![allow(clippy::unwrap_used)]

mod fake;

use fake::*;
use futures_executor::block_on;
use futures_util::future::join;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use todo_client::{
    Backoff, ClientConfig, Connection, ErrorKind, MemoryStorage, Readiness, Replica, SyncClient,
};
use todo_core::Command;
use uuid::Uuid;

type Client = SyncClient<MockTransport, FastClock>;

fn client(t: &MockTransport, storage: &MemoryStorage, seed: i64) -> Client {
    let mut cfg = ClientConfig::new(owner());
    cfg.backoff = Backoff {
        initial: Duration::from_millis(1),
        max: Duration::from_millis(2),
    };
    SyncClient::new(
        t.clone(),
        FastClock::default(),
        CountingEntropy::seeded(seed),
        storage.clone(),
        cfg,
    )
    .unwrap()
}

/// Runs the client until `done` holds, then stops it. Panics if that takes too long.
fn run_until(c: &Client, done: impl Fn(&Client) -> bool) {
    block_on(join(c.run(), async {
        for _ in 0..200_000 {
            if done(c) {
                c.stop();
                return;
            }
            yield_now().await;
        }
        c.stop();
        panic!("condition not reached; status {:?}", c.status());
    }));
}

fn seeded_server() -> Arc<Mutex<FakeServer>> {
    let mut s = FakeServer::default();
    s.execute(
        &owner(),
        Uuid::from_u128(900),
        &create_list(list_id(1), "Inbox"),
    )
    .unwrap();
    s.execute(
        &owner(),
        Uuid::from_u128(901),
        &create_task(task_id(1), list_id(1), "Task"),
    )
    .unwrap();
    Arc::new(Mutex::new(s))
}

fn synced(c: &Client, t: &MockTransport) -> bool {
    let s = t.server.lock().unwrap();
    c.pending().is_empty() && c.cursor() == s.head() && c.view().data_eq(&s.state)
}

#[test]
fn a_new_client_catches_up_and_goes_online() {
    let server = seeded_server();
    let t = MockTransport::new(server.clone(), owner());
    let c = client(&t, &MemoryStorage::new(), 1);
    assert_eq!(c.status().connection, Connection::Offline);
    run_until(&c, |c| {
        synced(c, &t) && c.status().connection == Connection::Online
    });
    assert_eq!(c.status().last_error, None);
}

#[test]
fn readiness_says_caught_up_only_once_the_servers_head_is_in() {
    let server = seeded_server();
    let t = MockTransport::new(server.clone(), owner());
    let storage = MemoryStorage::new();
    let c = client(&t, &storage, 1);
    assert_eq!(c.readiness(), Readiness::Starting);
    run_until(&c, |c| synced(c, &t));
    assert_eq!(c.readiness(), Readiness::CaughtUp);
    // Elsewhere, meanwhile: the task is completed. A client on the warm cache starts again.
    server
        .lock()
        .unwrap()
        .execute(
            &owner(),
            Uuid::from_u128(950),
            &Command::CompleteTask { id: task_id(1) },
        )
        .unwrap();
    let head = server.lock().unwrap().head();
    let c = client(&t, &storage, 2);
    assert_eq!(c.readiness(), Readiness::Starting);
    let early = std::cell::Cell::new(false);
    run_until(&c, |c| {
        let (view, readiness) = c.view_and_readiness();
        if readiness == Readiness::CaughtUp {
            // Read together: caught up means the completion is in the view it came with.
            early.set(early.get() || view.tasks[&task_id(1)].completed_at.is_none());
            return true;
        }
        false
    });
    assert!(!early.get(), "caught up before the backlog was in");
    assert!(c.cursor() >= head);
}

#[test]
fn readiness_says_failed_when_the_first_attempt_fails_then_catches_up() {
    let server = seeded_server();
    let t = MockTransport::new(server.clone(), owner());
    t.script.lock().unwrap().offline = 3;
    let c = client(&t, &MemoryStorage::new(), 1);
    let failed = std::cell::Cell::new(false);
    run_until(&c, |c| {
        failed.set(failed.get() || c.readiness() == Readiness::Failed);
        c.readiness() == Readiness::CaughtUp
    });
    assert!(failed.get(), "the offline first attempt was reported");
}

#[test]
fn a_guarded_submit_queues_only_while_the_guard_holds() {
    let server = seeded_server();
    let t = MockTransport::new(server.clone(), owner());
    let c = client(&t, &MemoryStorage::new(), 1);
    run_until(&c, |c| synced(c, &t));
    let complete = Command::CompleteTask { id: task_id(1) };
    // The guard sees the view as it is when the command would be queued.
    let refused = c
        .submit_tracked_if(complete.clone(), |v| {
            v.tasks[&task_id(1)].completed_at.is_some()
        })
        .unwrap();
    assert!(refused.is_none());
    assert!(c.pending().is_empty(), "nothing was queued");
    let queued = c
        .submit_tracked_if(complete, |v| v.tasks[&task_id(1)].completed_at.is_none())
        .unwrap();
    assert!(queued.is_some());
    assert_eq!(c.pending().len(), 1);
    assert!(c.view().tasks[&task_id(1)].completed_at.is_some());
}

#[test]
fn outcome_unknown_is_retried_with_the_same_command_id_and_applied_once() {
    let server = seeded_server();
    let t = MockTransport::new(server.clone(), owner());
    t.script.lock().unwrap().outcome_unknown = 3;
    let storage = MemoryStorage::new();
    let c = client(&t, &storage, 1);
    run_until(&c, |c| synced(c, &t));
    let c = client(&t, &storage, 2);
    c.submit(add_note(task_id(1), "once")).unwrap();
    run_until(&c, |c| synced(c, &t));
    let s = server.lock().unwrap();
    assert_eq!(s.stored_commands(), 3);
    assert_eq!(s.state.tasks[&task_id(1)].notes.matches("once").count(), 1);
    // Either the retries got the replayed result or the stream delivered the events first.
    assert!(t.script.lock().unwrap().outcome_unknown < 3);
}

#[test]
fn network_errors_are_retried_until_the_command_is_through() {
    let server = seeded_server();
    let t = MockTransport::new(server.clone(), owner());
    let storage = MemoryStorage::new();
    let c = client(&t, &storage, 1);
    run_until(&c, |c| synced(c, &t));
    t.script.lock().unwrap().offline = 12;
    let c = client(&t, &storage, 2);
    for i in 0..5 {
        c.submit(add_note(task_id(1), &format!("<{i}>"))).unwrap();
    }
    run_until(&c, |c| synced(c, &t));
    let s = server.lock().unwrap();
    assert_eq!(s.stored_commands(), 2 + 5);
    let notes = &s.state.tasks[&task_id(1)].notes;
    let order: Vec<usize> = (0..5)
        .map(|i| notes.find(&format!("<{i}>")).unwrap())
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "sent in order");
}

#[test]
fn a_command_the_server_rejects_lands_in_couldnt_sync() {
    let server = seeded_server();
    let t = MockTransport::new(server.clone(), owner());
    let storage = MemoryStorage::new();
    let c = client(&t, &storage, 1);
    run_until(&c, |c| synced(c, &t));
    // Another device deletes the task while this client is not running; this client still sees it
    // and adds a note.
    let phone = todo_core::Actor::owner("phone").unwrap();
    server
        .lock()
        .unwrap()
        .execute(
            &phone,
            Uuid::from_u128(5),
            &Command::Delete {
                id: task_id(1).into(),
            },
        )
        .unwrap();
    let c = client(&t, &storage, 2);
    let id = c.submit(add_note(task_id(1), "too late")).unwrap();
    run_until(&c, |c| c.failed().len() == 1 && synced(c, &t));
    let failed = c.failed();
    assert_eq!(failed[0].entry.command_id, id);
    assert_eq!(failed[0].rejection.status, 409);
    assert_eq!(c.status().failed, 1);
    assert!(c.dismiss(id).unwrap());
    assert!(c.failed().is_empty());
}

#[test]
fn a_cursor_ahead_of_the_server_triggers_a_full_resync_that_keeps_the_outbox() {
    let first = seeded_server();
    let t = MockTransport::new(first.clone(), owner());
    let storage = MemoryStorage::new();
    let c = client(&t, &storage, 1);
    run_until(&c, |c| synced(c, &t));
    assert_eq!(c.cursor(), 2);
    // Queued while not running.
    let c = client(&t, &storage, 2);
    c.submit(create_list(list_id(5), "Queued")).unwrap();

    // The server is restored from an older backup: one event, different content.
    let mut restored = FakeServer::default();
    restored
        .execute(
            &owner(),
            Uuid::from_u128(1),
            &create_list(list_id(1), "Restored"),
        )
        .unwrap();
    let restored = Arc::new(Mutex::new(restored));
    let t2 = MockTransport::new(restored.clone(), owner());
    let c = client(&t2, &storage, 3);
    run_until(&c, |c| synced(c, &t2));
    let s = restored.lock().unwrap();
    assert_eq!(c.cursor(), 2, "resynced from 0, then the queued command");
    assert!(s.state.lists.contains_key(&list_id(5)));
    assert!(!c.view().tasks.contains_key(&task_id(1)));
    assert_eq!(c.view().lists[&list_id(1)].name, "Restored");
    // Storage was rewritten too.
    let again = Replica::open(owner(), Box::new(storage.clone())).unwrap();
    assert!(again.confirmed().data_eq(&s.state));
}

#[test]
fn a_restored_log_that_regrew_past_the_cursor_is_detected_by_its_epoch() {
    let server = seeded_server();
    server
        .lock()
        .unwrap()
        .execute(&owner(), Uuid::from_u128(3), &add_note(task_id(1), "lost"))
        .unwrap();
    let t = MockTransport::new(server.clone(), owner());
    let storage = MemoryStorage::new();
    let c = client(&t, &storage, 1);
    run_until(&c, |c| synced(c, &t));
    assert_eq!((c.cursor(), c.epoch()), (3, Some(EPOCH)));

    // Restored from a backup without the third event, then other events took its place: the cursor
    // is valid on the new log, but means another history.
    let new_epoch = Uuid::from_u128(0xe1);
    {
        let mut s = server.lock().unwrap();
        s.restore(2, new_epoch);
        let phone = todo_core::Actor::owner("phone").unwrap();
        s.execute(&phone, Uuid::from_u128(4), &add_note(task_id(1), "other a"))
            .unwrap();
        s.execute(&phone, Uuid::from_u128(5), &add_note(task_id(1), "other b"))
            .unwrap();
    }
    let c = client(&t, &storage, 2);
    c.submit(add_note(task_id(1), "queued")).unwrap();
    run_until(&c, |c| synced(c, &t) && c.epoch() == Some(new_epoch));
    let notes = c.view().tasks[&task_id(1)].notes.clone();
    assert!(!notes.contains("lost"), "{notes}");
    for n in ["other a", "other b", "queued"] {
        assert_eq!(notes.matches(n).count(), 1, "{notes}");
    }
    let again = Replica::open(owner(), Box::new(storage.clone())).unwrap();
    assert_eq!(again.epoch(), Some(new_epoch));
    assert!(again.confirmed().data_eq(&server.lock().unwrap().state));
}

#[test]
fn an_event_that_never_applies_is_a_backed_off_protocol_error_not_a_resync_loop() {
    let server = seeded_server();
    {
        // A broken log: event 3 repeats event 1.
        let mut s = server.lock().unwrap();
        let mut dup = s.log[0].clone();
        dup.event.seq = Some(3);
        s.log.push(dup);
    }
    let t = MockTransport::new(server.clone(), owner());
    let c = client(&t, &MemoryStorage::new(), 1);
    run_until(&c, |c| {
        c.status().last_error.as_ref().is_some_and(|e| {
            e.kind == ErrorKind::Protocol && e.message.contains("even after a full resync")
        })
    });
    assert_eq!(c.cursor(), 2, "everything before the broken event is kept");
}

/// Storage whose next `n` resets (writes that clear the confirmed state) fail.
#[derive(Clone)]
struct FailingResets {
    inner: MemoryStorage,
    left: Arc<std::sync::atomic::AtomicUsize>,
    failed: Arc<std::sync::atomic::AtomicUsize>,
}

impl todo_client::Storage for FailingResets {
    fn load(&mut self) -> Result<todo_client::storage::Loaded, todo_client::StorageError> {
        self.inner.load()
    }

    fn write(
        &mut self,
        ops: &[todo_client::storage::StorageOp<'_>],
    ) -> Result<(), todo_client::StorageError> {
        use std::sync::atomic::Ordering::SeqCst;
        let reset = ops
            .iter()
            .any(|op| matches!(op, todo_client::storage::StorageOp::ClearConfirmed));
        if reset && self.left.load(SeqCst) > 0 {
            self.left.fetch_sub(1, SeqCst);
            self.failed.fetch_add(1, SeqCst);
            return Err(todo_client::StorageError::Backend(
                "disk full (test)".into(),
            ));
        }
        self.inner.write(ops)
    }
}

#[test]
fn a_reset_that_fails_is_retried_instead_of_tripping_the_divergence_guard() {
    // The server's log: a list, task 1, and a note on task 1 (event 3).
    let server = seeded_server();
    server
        .lock()
        .unwrap()
        .execute(&owner(), Uuid::from_u128(3), &add_note(task_id(1), "n"))
        .unwrap();
    // This client's stored state followed another history with the same epoch: its event 2 created
    // task 2, so the server's event 3 does not apply to it.
    let storage = MemoryStorage::new();
    {
        let mut other = FakeServer::default();
        other
            .execute(&owner(), Uuid::from_u128(1), &create_list(list_id(1), "L"))
            .unwrap();
        other
            .execute(
                &owner(),
                Uuid::from_u128(2),
                &create_task(task_id(2), list_id(1), "T"),
            )
            .unwrap();
        let mut r = Replica::open(owner(), Box::new(storage.clone())).unwrap();
        r.apply_confirmed(EPOCH, &other.log).unwrap();
    }
    let flaky = FailingResets {
        inner: storage.clone(),
        left: Arc::new(1.into()),
        failed: Arc::default(),
    };
    let t = MockTransport::new(server.clone(), owner());
    let mut cfg = ClientConfig::new(owner());
    cfg.backoff = Backoff {
        initial: Duration::from_millis(1),
        max: Duration::from_millis(2),
    };
    let c = SyncClient::new(
        t.clone(),
        FastClock::default(),
        CountingEntropy::seeded(1),
        flaky.clone(),
        cfg,
    )
    .unwrap();
    assert_eq!(c.cursor(), 2);
    run_until(&c, |c| synced(c, &t));
    assert_eq!(flaky.failed.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(c.view().tasks.contains_key(&task_id(1)));
    assert!(!c.view().tasks.contains_key(&task_id(2)));
}

/// A server from before `command_len` doesn't say where a batch ends, and the latest command has no
/// later event to prove it. The command's receipt lists all its events, so an undo of the latest
/// confirmed completion still shows offline.
#[test]
fn without_command_len_the_receipt_completes_the_latest_batch_for_an_undo() {
    use todo_client::TrackedUndo;
    use todo_core::command::NewTask;
    use todo_core::recurrence::{Recurrence, Rule};

    let server = seeded_server();
    {
        let mut s = server.lock().unwrap();
        s.legacy = true;
        for e in &mut s.log {
            e.command_len = None;
        }
    }
    let t = MockTransport::new(server.clone(), owner());
    let storage = MemoryStorage::new();
    run_until(&client(&t, &storage, 1), |c| synced(c, &t));
    let c = client(&t, &storage, 2);
    let task = task_id(2);
    let mut repeat = NewTask::new(task, list_id(1), "Repeat");
    repeat.due = Some("2026-11-21".parse().unwrap());
    repeat.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
    c.submit(Command::CreateTask(Box::new(repeat))).unwrap();
    let done = c
        .submit_tracked(Command::CompleteTask { id: task })
        .unwrap();
    run_until(&c, |c| synced(c, &t) && c.view().tasks.len() == 3);
    assert!(
        server
            .lock()
            .unwrap()
            .log
            .iter()
            .all(|e| e.command_len.is_none()),
        "an older server"
    );

    // Offline: the client isn't running.
    let TrackedUndo::Queued(_) = c.undo_tracked(&done).unwrap() else {
        panic!("a confirmed completion is undone through the outbox");
    };
    let view = c.view();
    assert!(!view.tasks[&task].is_done(), "the undo shows offline");
    assert_eq!(view.tasks.len(), 2, "with the next repeat gone");
}
