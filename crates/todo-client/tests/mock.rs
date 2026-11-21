//! The sync client's loops against a mock transport, on a single-threaded executor and without a
//! network or a runtime.
#![allow(clippy::unwrap_used)]

mod fake;

use fake::*;
use futures_executor::block_on;
use futures_util::future::join;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use todo_client::{Backoff, ClientConfig, Connection, MemoryStorage, Replica, SyncClient};
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
