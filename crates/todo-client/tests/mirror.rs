//! `MirrorStorage`: it behaves exactly like `MemoryStorage`, and its journal, replayed by a backend,
//! rebuilds the same storage after a restart (the browser's IndexedDB backend relies on both).
#![allow(clippy::unwrap_used)]

mod fake;

use fake::*;
use proptest::prelude::*;
use todo_client::mirror::{Change, Key, MirrorStorage, Persisted};
use todo_client::outbox::{FailedEntry, Operation, OutboxEntry, Rejection};
use todo_client::protocol::LogEvent;
use todo_client::storage::{Storage, StorageOp};
use todo_client::{MemoryStorage, Replica};
use todo_core::State;
use uuid::Uuid;

fn log() -> Vec<LogEvent> {
    let mut s = FakeServer::default();
    s.execute(
        &owner(),
        Uuid::from_u128(1),
        &create_list(list_id(1), "Inbox"),
    )
    .unwrap();
    for n in 0..12u64 {
        s.execute(
            &owner(),
            Uuid::from_u128(100 + u128::from(n)),
            &create_task(task_id(n), list_id(1), &format!("Úkol {n}")),
        )
        .unwrap();
    }
    s.log
}

fn entry(n: u8, note: &str) -> OutboxEntry {
    OutboxEntry {
        command_id: Uuid::from_u128(u128::from(n)),
        op: Operation::Command {
            command: add_note(task_id(1), note),
        },
        submitted_at: ts(i64::from(n)),
        attempts: 0,
        acked: None,
        acked_epoch: None,
    }
}

#[derive(Clone, Debug)]
enum Op {
    Append(usize),
    Snapshot,
    Clear,
    Epoch(u8),
    PutOutbox(u8, bool),
    RemoveOutbox(u8),
    PutFailed(u8),
    RemoveFailed(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1usize..4).prop_map(Op::Append),
        Just(Op::Snapshot),
        Just(Op::Clear),
        (0u8..3).prop_map(Op::Epoch),
        (0u8..5, any::<bool>()).prop_map(|(n, b)| Op::PutOutbox(n, b)),
        (0u8..5).prop_map(Op::RemoveOutbox),
        (0u8..5).prop_map(Op::PutFailed),
        (0u8..5).prop_map(Op::RemoveFailed),
    ]
}

/// Applies `ops` (as one write) to both storages.
fn write(a: &mut MemoryStorage, b: &mut MirrorStorage, ops: &[StorageOp<'_>]) {
    a.write(ops).unwrap();
    b.write(ops).unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn mirror_matches_memory_and_its_journal_rebuilds_it(ops in prop::collection::vec(op(), 1..40)) {
        let log = log();
        let state = State::new();
        let mut mem = MemoryStorage::new();
        let mut mirror = MirrorStorage::new();
        let mut backend = Persisted::default();
        let mut next = 0usize;
        for o in &ops {
            match o {
                Op::Append(k) => {
                    let end = (next + k).min(log.len());
                    if end > next {
                        write(&mut mem, &mut mirror, &[StorageOp::AppendEvents(&log[next..end])]);
                        next = end;
                    }
                }
                Op::Snapshot => {
                    write(&mut mem, &mut mirror, &[StorageOp::Snapshot { state: &state, cursor: next as u64 }]);
                }
                Op::Clear => {
                    write(&mut mem, &mut mirror, &[StorageOp::ClearConfirmed]);
                    next = 0;
                }
                Op::Epoch(n) => {
                    write(&mut mem, &mut mirror, &[StorageOp::SetEpoch(Uuid::from_u128(u128::from(*n) + 7))]);
                }
                Op::PutOutbox(n, b) => {
                    let e = entry(*n, if *b { "jedna" } else { "dvě" });
                    write(&mut mem, &mut mirror, &[StorageOp::PutOutbox(&e)]);
                }
                Op::RemoveOutbox(n) => {
                    write(&mut mem, &mut mirror, &[StorageOp::RemoveOutbox(Uuid::from_u128(u128::from(*n)))]);
                }
                Op::PutFailed(n) => {
                    let f = FailedEntry {
                        entry: entry(*n, "x"),
                        failed_at: ts(5),
                        rejection: Rejection { status: 404, code: "deleted".into(), message: String::new() },
                    };
                    // With an outbox removal in the same batch, as the replica does.
                    write(&mut mem, &mut mirror, &[
                        StorageOp::RemoveOutbox(f.entry.command_id),
                        StorageOp::PutFailed(&f),
                    ]);
                }
                Op::RemoveFailed(n) => {
                    write(&mut mem, &mut mirror, &[StorageOp::RemoveFailed(Uuid::from_u128(u128::from(*n)))]);
                }
            }
            prop_assert_eq!(mem.load().unwrap(), mirror.load().unwrap());
            // The backend drains the journal at its own pace (here: every other step).
            if next.is_multiple_of(2) {
                for batch in mirror.take_batches() {
                    for c in &batch {
                        backend.apply(c);
                    }
                }
            }
        }
        for batch in mirror.take_batches() {
            for c in &batch {
                backend.apply(c);
            }
        }
        prop_assert_eq!(&backend, &mirror.persisted());
        let mut reopened = MirrorStorage::from_persisted(backend).unwrap();
        prop_assert_eq!(reopened.load().unwrap(), mem.load().unwrap());
    }
}

#[test]
fn a_failed_write_changes_nothing_and_journals_nothing() {
    let mut mirror = MirrorStorage::new();
    let log = log();
    let mut bad = log[0].clone();
    bad.event.seq = None;
    assert!(mirror.write(&[StorageOp::AppendEvents(&[bad])]).is_err());
    assert!(!mirror.has_pending());
    assert_eq!(mirror.load().unwrap(), Default::default());
}

#[test]
fn requeued_batches_come_back_first() {
    let mut mirror = MirrorStorage::new();
    mirror
        .write(&[StorageOp::SetEpoch(Uuid::from_u128(1))])
        .unwrap();
    let first = mirror.take_batches();
    mirror
        .write(&[StorageOp::SetEpoch(Uuid::from_u128(2))])
        .unwrap();
    mirror.requeue(first);
    let all = mirror.take_batches();
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[0],
        vec![Change::Put(
            Key::Epoch,
            format!("\"{}\"", Uuid::from_u128(1))
        )]
    );
}

/// The scenario the web app needs: an offline edit survives a restart (a reload of the page).
#[test]
fn a_pending_command_survives_a_restart_through_the_journal() {
    let mut server = FakeServer::default();
    server
        .execute(
            &owner(),
            Uuid::from_u128(900),
            &create_list(list_id(1), "Inbox"),
        )
        .unwrap();
    server
        .execute(
            &owner(),
            Uuid::from_u128(901),
            &create_task(task_id(1), list_id(1), "Koupit mléko"),
        )
        .unwrap();
    let mirror = MirrorStorage::new();
    let mut r = Replica::open(owner(), Box::new(mirror.clone())).unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    r.submit(entry(1, "offline")).unwrap();
    let mut backend = Persisted::default();
    for batch in mirror.take_batches() {
        for c in &batch {
            backend.apply(c);
        }
    }
    drop(r);
    let reopened = MirrorStorage::from_persisted(backend).unwrap();
    let mut r = Replica::open(owner(), Box::new(reopened)).unwrap();
    assert_eq!(r.outbox().len(), 1);
    assert_eq!(r.view().tasks[&task_id(1)].notes, "offline");
    assert_eq!(r.cursor(), 2);
}
