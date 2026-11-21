//! `MirrorStorage`: it behaves exactly like `MemoryStorage`, and its journal, replayed by a backend,
//! rebuilds the same storage after a restart (the browser's IndexedDB backend relies on both).
#![allow(clippy::unwrap_used)]

mod fake;

use fake::*;
use proptest::prelude::*;
use todo_client::mirror::{Area, Change, Key, MirrorStorage, Persisted, Recovery, damaged_key};
use todo_client::outbox::{FailedEntry, Operation, OutboxEntry, Rejection};
use todo_client::protocol::LogEvent;
use todo_client::storage::{Loaded, Storage, StorageOp};
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

/// Outbox and failed entries in submission order, as a reopened storage reads them back (the backend
/// keeps them one record per command, so insertion order is not kept).
fn by_submission(mut l: Loaded) -> Loaded {
    l.outbox
        .sort_by_key(|e| (e.submitted_at.as_millisecond(), e.command_id));
    l.failed
        .sort_by_key(|f| (f.failed_at.as_millisecond(), f.entry.command_id));
    l
}

/// Drains the journal into the backend.
fn flush(mirror: &MirrorStorage, backend: &mut Persisted) {
    for batch in mirror.take_batches() {
        for c in &batch {
            backend.apply(c);
        }
    }
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
        let (mut reopened, rec) = MirrorStorage::recover(backend);
        prop_assert_eq!(rec, Recovery::default());
        prop_assert!(!reopened.has_pending());
        prop_assert_eq!(reopened.load().unwrap(), by_submission(mem.load().unwrap()));
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
fn a_batch_stays_queued_until_it_is_committed() {
    let mut mirror = MirrorStorage::new();
    mirror
        .write(&[StorageOp::SetEpoch(Uuid::from_u128(1))])
        .unwrap();
    mirror
        .write(&[StorageOp::SetEpoch(Uuid::from_u128(2))])
        .unwrap();
    let (seq, first) = mirror.front().unwrap();
    assert_eq!(
        first,
        vec![Change::Put(
            Key::Epoch,
            format!("\"{}\"", Uuid::from_u128(1))
        )]
    );
    // A failed write leaves it at the front, and says why.
    mirror.set_flush_error("QuotaExceededError");
    assert_eq!(mirror.front().unwrap().0, seq);
    assert_eq!(mirror.flush_error().as_deref(), Some("QuotaExceededError"));
    assert!(mirror.unsaved());
    mirror.commit_front(seq);
    assert_eq!(mirror.flush_error(), None);
    assert!(mirror.unsaved(), "the second batch is still queued");
    let (seq, _) = mirror.front().unwrap();
    mirror.commit_front(seq);
    assert!(!mirror.unsaved());
    assert_eq!(mirror.front(), None);
}

#[test]
fn an_outbox_entry_is_durable_once_the_batch_that_added_it_lands() {
    let mut mirror = MirrorStorage::new();
    let saved = mirror.saved().unwrap();
    let seen = saved.generation();
    let e = entry(1, "a");
    mirror.write(&[StorageOp::PutOutbox(&e)]).unwrap();
    assert!(!mirror.is_durable(e.command_id));
    // Rewriting it (an attempt counted) does not move the batch it waits for.
    let mut again = e.clone();
    again.attempts = 1;
    mirror.write(&[StorageOp::PutOutbox(&again)]).unwrap();
    let (seq, _) = mirror.front().unwrap();
    mirror.commit_front(seq);
    assert!(mirror.is_durable(e.command_id));
    assert!(saved.generation() > seen);
    // Entries another tab wrote, read back from the backend, are durable at once.
    let theirs = entry(2, "b");
    mirror.mark_durable([theirs.command_id]);
    mirror.write(&[StorageOp::PutOutbox(&theirs)]).unwrap();
    assert!(mirror.is_durable(theirs.command_id));
    // So are entries read back on start.
    let mut backend = Persisted::default();
    let fresh = entry(3, "c");
    mirror.write(&[StorageOp::PutOutbox(&fresh)]).unwrap();
    flush(&mirror, &mut backend);
    let (reopened, _) = MirrorStorage::recover(backend);
    assert!(reopened.is_durable(fresh.command_id));
}

#[test]
fn the_replica_does_not_send_an_entry_before_it_is_durable() {
    let server = server_with_task();
    let mirror = MirrorStorage::new();
    let mut r = Replica::open(owner(), Box::new(mirror.clone())).unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    let _ = mirror.take_batches();
    r.submit(entry(1, "first")).unwrap();
    r.submit(entry(2, "second")).unwrap();
    assert_eq!(r.next_to_send().unwrap(), None, "nothing has landed");
    // The first lands: it is sent, and the second still waits.
    let (seq, _) = mirror.front().unwrap();
    mirror.commit_front(seq);
    let first = r.next_to_send().unwrap().unwrap();
    assert_eq!(first.command_id, Uuid::from_u128(1));
}

fn server_with_task() -> FakeServer {
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
    server
}

/// Two tabs over one database, each with its own storage: neither overwrites the other's unsent
/// work, whatever order their batches land in, and a restart sends both.
#[test]
fn two_tabs_keep_each_others_outbox() {
    let server = server_with_task();
    let mut backend = Persisted::default();
    // Tab A syncs first and persists.
    let a = MirrorStorage::new();
    let mut ra = Replica::open(owner(), Box::new(a.clone())).unwrap();
    ra.apply_confirmed(EPOCH, &server.log).unwrap();
    flush(&a, &mut backend);
    // Tab B opens on what A stored.
    let (b, _) = MirrorStorage::recover(backend.clone());
    let mut rb = Replica::open(owner(), Box::new(b.clone())).unwrap();
    // Both queue a command offline; A's batch lands, then B's, then A writes again.
    ra.submit(entry(1, "z karty A")).unwrap();
    rb.submit(entry(2, "z karty B")).unwrap();
    flush(&a, &mut backend);
    flush(&b, &mut backend);
    ra.submit(entry(3, "zase A")).unwrap();
    flush(&a, &mut backend);
    drop((ra, rb));

    let (reopened, rec) = MirrorStorage::recover(backend);
    assert_eq!(rec, Recovery::default());
    let r = Replica::open(owner(), Box::new(reopened)).unwrap();
    let ids: Vec<_> = r.outbox().iter().map(|e| e.command_id).collect();
    assert_eq!(
        ids,
        [1u128, 2, 3].map(Uuid::from_u128),
        "every tab's command, once, in submission order"
    );
}

/// A tab confirming its own command removes only that record.
#[test]
fn removing_an_entry_touches_only_its_record() {
    let mut backend = Persisted::default();
    let mut a = MirrorStorage::new();
    a.write(&[StorageOp::PutOutbox(&entry(1, "a"))]).unwrap();
    flush(&a, &mut backend);
    let (mut b, _) = MirrorStorage::recover(Persisted::default());
    b.write(&[StorageOp::PutOutbox(&entry(2, "b"))]).unwrap();
    flush(&b, &mut backend);
    a.write(&[StorageOp::RemoveOutbox(Uuid::from_u128(1))])
        .unwrap();
    flush(&a, &mut backend);
    let (mut reopened, _) = MirrorStorage::recover(backend);
    let outbox = reopened.load().unwrap().outbox;
    assert_eq!(outbox, vec![entry(2, "b")]);
}

/// An unreadable snapshot is dropped (the server has it) and the outbox is kept.
#[test]
fn an_unreadable_snapshot_is_refetched_and_the_outbox_kept() {
    let mut backend = Persisted::default();
    let mut m = MirrorStorage::new();
    let log = log();
    m.write(&[
        StorageOp::SetEpoch(EPOCH),
        StorageOp::AppendEvents(&log[..3]),
        StorageOp::PutOutbox(&entry(1, "neodeslané")),
    ])
    .unwrap();
    flush(&m, &mut backend);
    backend.kv.insert(Key::Snapshot, "{not json".into());

    let (mut s, rec) = MirrorStorage::recover(backend.clone());
    assert!(rec.dropped_confirmed);
    assert!(!rec.has_damaged());
    let loaded = s.load().unwrap();
    assert_eq!(loaded.snapshot, None);
    assert!(loaded.events.is_empty());
    assert_eq!(loaded.epoch, None);
    assert_eq!(loaded.outbox, vec![entry(1, "neodeslané")]);
    // The repair lands first, and leaves the outbox record alone.
    flush(&s, &mut backend);
    assert!(!backend.kv.contains_key(&Key::Snapshot));
    assert!(backend.events.is_empty());
    assert_eq!(backend.outbox.len(), 1);
}

/// An unreadable event drops the stored events only; the snapshot stays.
#[test]
fn an_unreadable_event_drops_the_events_only() {
    let mut backend = Persisted::default();
    let mut m = MirrorStorage::new();
    let log = log();
    let state = State::new();
    m.write(&[StorageOp::Snapshot {
        state: &state,
        cursor: 0,
    }])
    .unwrap();
    m.write(&[StorageOp::AppendEvents(&log[..2])]).unwrap();
    flush(&m, &mut backend);
    backend.events.insert(3, "garbage".into());
    let (mut s, rec) = MirrorStorage::recover(backend);
    assert!(rec.dropped_events && !rec.dropped_confirmed);
    let loaded = s.load().unwrap();
    assert!(loaded.snapshot.is_some());
    assert!(loaded.events.is_empty());
}

/// An unreadable outbox or failed record is set aside, never deleted, and the rest still loads.
#[test]
fn an_unreadable_entry_is_set_aside() {
    let mut backend = Persisted::default();
    let mut m = MirrorStorage::new();
    m.write(&[StorageOp::PutOutbox(&entry(1, "dobrý"))])
        .unwrap();
    flush(&m, &mut backend);
    backend
        .outbox
        .insert("rozbitý".into(), "{\"command_id\":".into());
    backend.failed.insert("x".into(), "[]".into());

    let (mut s, rec) = MirrorStorage::recover(backend.clone());
    let a = damaged_key(Area::Outbox.name(), "rozbitý");
    let f = damaged_key(Area::Failed.name(), "x");
    assert_eq!(rec.damaged, vec![a.clone(), f.clone()]);
    assert_eq!(s.load().unwrap().outbox, vec![entry(1, "dobrý")]);
    flush(&s, &mut backend);
    assert_eq!(backend.damaged[&a], "{\"command_id\":");
    assert_eq!(backend.damaged[&f], "[]");
    assert!(!backend.outbox.contains_key("rozbitý"));
    // The next start reports what was set aside before, and repairs nothing.
    let (s, rec) = MirrorStorage::recover(backend);
    assert!(rec.damaged.is_empty() && rec.damaged_before == 2 && rec.has_damaged());
    assert!(!s.has_pending());
}

/// The first version stored the outbox as one array: it is split into records on start.
#[test]
fn the_legacy_outbox_array_is_split_into_records() {
    let mut backend = Persisted::default();
    let list = format!(
        "[{},{},{{\"nope\":1}}]",
        serde_json::to_string(&entry(2, "b")).unwrap(),
        serde_json::to_string(&entry(1, "a")).unwrap()
    );
    backend.kv.insert(Key::LegacyOutbox, list);
    let (mut s, rec) = MirrorStorage::recover(backend.clone());
    assert_eq!(rec.damaged, vec![damaged_key("outbox", "2")]);
    assert_eq!(s.load().unwrap().outbox, vec![entry(1, "a"), entry(2, "b")]);
    flush(&s, &mut backend);
    assert!(!backend.kv.contains_key(&Key::LegacyOutbox));
    assert_eq!(backend.outbox.len(), 2);
    assert_eq!(backend.damaged.len(), 1);
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
    let (reopened, _) = MirrorStorage::recover(backend);
    let mut r = Replica::open(owner(), Box::new(reopened)).unwrap();
    assert_eq!(r.outbox().len(), 1);
    assert_eq!(r.view().tasks[&task_id(1)].notes, "offline");
    assert_eq!(r.cursor(), 2);
}
