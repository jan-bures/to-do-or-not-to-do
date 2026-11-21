//! The replica's rules, without a network: optimistic view, acknowledgement, confirmation by command
//! ID, the log epoch, storage failures, rejection, persistence and resync.
#![allow(clippy::unwrap_used)]

mod fake;

use fake::*;
use proptest::prelude::*;
use todo_client::outbox::{Operation, OutboxEntry, Rejection};
use todo_client::replica::{Ack, ConfirmError, SubmitError};
use todo_client::storage::{Storage, StorageOp};
use todo_client::{MemoryStorage, Replica};
use todo_core::Command;
use uuid::Uuid;

fn entry(n: u128, command: Command, at_ms: i64) -> OutboxEntry {
    OutboxEntry {
        command_id: Uuid::from_u128(n),
        op: Operation::Command { command },
        submitted_at: ts(at_ms),
        attempts: 0,
        acked: None,
        acked_epoch: None,
    }
}

fn open(storage: &MemoryStorage) -> Replica {
    Replica::open(owner(), Box::new(storage.clone())).unwrap()
}

fn notes(r: &mut Replica, t: todo_core::ids::TaskId) -> String {
    r.view().tasks[&t].notes.clone()
}

/// A replica that has confirmed one list and one task.
fn seeded() -> (Replica, FakeServer, MemoryStorage) {
    let storage = MemoryStorage::new();
    let mut r = open(&storage);
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
            &create_task(task_id(1), list_id(1), "Task"),
        )
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    (r, server, storage)
}

#[test]
fn a_submitted_command_shows_in_the_view_at_once() {
    let (mut r, _server, _) = seeded();
    r.submit(entry(1, add_note(task_id(1), "hello"), 1))
        .unwrap();
    assert_eq!(notes(&mut r, task_id(1)), "hello");
    assert!(r.confirmed().tasks[&task_id(1)].notes.is_empty());
    assert_eq!(r.outbox().len(), 1);
}

#[test]
fn an_invalid_command_is_refused_locally_and_not_queued() {
    let (mut r, _server, _) = seeded();
    let err = r
        .submit(entry(1, add_note(task_id(99), "x"), 1))
        .unwrap_err();
    assert!(matches!(err, SubmitError::Invalid(_)));
    assert!(r.outbox().is_empty());
}

#[test]
fn a_storage_failure_refuses_the_submission() {
    let (mut r, _server, storage) = seeded();
    storage.set_fail_writes(true);
    let err = r
        .submit(entry(1, add_note(task_id(1), "x"), 1))
        .unwrap_err();
    assert!(matches!(err, SubmitError::Storage(_)));
    assert!(r.outbox().is_empty());
    assert!(notes(&mut r, task_id(1)).is_empty());
}

#[test]
fn an_acknowledged_command_leaves_when_its_events_arrive_and_is_never_applied_twice() {
    let (mut r, mut server, _) = seeded();
    let e = entry(1, add_note(task_id(1), "once"), 1);
    r.submit(e.clone()).unwrap();
    let sent = r.next_to_send().unwrap().unwrap();
    assert_eq!(sent.attempts, 1);
    let result = server
        .execute(&owner(), e.command_id, e.command().unwrap())
        .unwrap();
    r.acknowledge(e.command_id, &result).unwrap();
    // Acknowledged but not yet confirmed: still pending, still in the view.
    assert_eq!(r.outbox().len(), 1);
    assert!(r.next_to_send().unwrap().is_none());
    assert_eq!(notes(&mut r, task_id(1)), "once");
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    assert!(r.outbox().is_empty());
    assert_eq!(notes(&mut r, task_id(1)), "once");
}

#[test]
fn events_arriving_before_the_acknowledgement_confirm_the_entry_by_its_command_id() {
    let (mut r, mut server, _) = seeded();
    let e = entry(1, add_note(task_id(1), "once"), 1);
    r.submit(e.clone()).unwrap();
    r.next_to_send().unwrap().unwrap();
    let result = server
        .execute(&owner(), e.command_id, e.command().unwrap())
        .unwrap();
    // The stream is faster than the response.
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    assert!(r.outbox().is_empty(), "confirmed by the event's command ID");
    assert_eq!(notes(&mut r, task_id(1)), "once");
    // The late acknowledgement is harmless.
    assert_eq!(r.acknowledge(e.command_id, &result).unwrap(), Ack::Recorded);
    assert!(r.outbox().is_empty());
}

#[test]
fn an_identical_command_from_another_session_does_not_confirm_a_pending_one() {
    // Another session of the same actor (same device ID) makes exactly the same change, and its
    // event reaches this replica first, before this replica's own command was even executed.
    let (mut r, mut server, _) = seeded();
    let mine = entry(1, add_note(task_id(1), "same"), 1);
    r.submit(mine.clone()).unwrap();
    r.next_to_send().unwrap();
    server
        .execute(&owner(), Uuid::from_u128(5), &add_note(task_id(1), "same"))
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    assert_eq!(r.outbox().len(), 1, "still pending");
    assert_eq!(notes(&mut r, task_id(1)).matches("same").count(), 2);
    // Its own events confirm it.
    let result = server
        .execute(&owner(), mine.command_id, mine.command().unwrap())
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    assert!(r.outbox().is_empty());
    assert_eq!(
        r.acknowledge(mine.command_id, &result).unwrap(),
        Ack::Recorded
    );
    assert_eq!(notes(&mut r, task_id(1)).matches("same").count(), 2);
    assert!(r.view().data_eq(&server.state));
}

#[test]
fn a_failed_confirmation_write_applies_nothing_and_the_entry_survives_it() {
    let (mut r, mut server, storage) = seeded();
    let e = entry(1, add_note(task_id(1), "once"), 1);
    r.submit(e.clone()).unwrap();
    r.next_to_send().unwrap();
    let result = server
        .execute(&owner(), e.command_id, e.command().unwrap())
        .unwrap();
    r.acknowledge(e.command_id, &result).unwrap();
    let cursor = r.cursor();

    storage.set_fail_writes(true);
    assert!(matches!(
        r.apply_confirmed(EPOCH, &server.log),
        Err(ConfirmError::Storage(_))
    ));
    // Memory is not ahead of disk: nothing applied, the entry still pending (and not resent).
    assert_eq!(r.cursor(), cursor);
    assert_eq!(r.outbox().len(), 1);
    assert!(
        r.next_to_send().unwrap().is_none(),
        "acknowledged: not resent"
    );
    assert_eq!(notes(&mut r, task_id(1)), "once");
    storage.set_fail_writes(false);

    // A crash here, then the events arrive again.
    drop(r);
    let mut again = open(&storage);
    assert_eq!(again.cursor(), cursor);
    assert_eq!(again.outbox().len(), 1);
    assert_eq!(notes(&mut again, task_id(1)), "once");
    again.apply_confirmed(EPOCH, &server.log).unwrap();
    assert!(again.outbox().is_empty());
    assert_eq!(notes(&mut again, task_id(1)), "once");
    assert!(open(&storage).outbox().is_empty());
}

#[test]
fn opening_removes_acknowledged_entries_whose_events_are_confirmed() {
    // Storage as a lossy backend might leave it: the snapshot has the command's event, the outbox
    // still has the acknowledged entry.
    let (mut r, mut server, _) = seeded();
    let e = entry(1, add_note(task_id(1), "once"), 1);
    let result = server
        .execute(&owner(), e.command_id, e.command().unwrap())
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    let mut storage = MemoryStorage::new();
    let acked = OutboxEntry {
        attempts: 1,
        acked: Some(result.events.iter().map(|r| r.id).collect()),
        acked_epoch: Some(EPOCH),
        ..e
    };
    storage
        .write(&[
            StorageOp::SetEpoch(EPOCH),
            StorageOp::Snapshot {
                state: r.confirmed(),
                cursor: r.cursor(),
            },
            StorageOp::PutOutbox(&acked),
        ])
        .unwrap();
    let mut again = open(&storage);
    assert!(again.outbox().is_empty());
    assert_eq!(notes(&mut again, task_id(1)), "once");
    assert!(
        open(&storage).outbox().is_empty(),
        "and the removal is stored"
    );
}

#[test]
fn events_from_another_epoch_are_refused() {
    let storage = MemoryStorage::new();
    let mut r = open(&storage);
    let mut server = FakeServer::default();
    server
        .execute(&owner(), Uuid::from_u128(1), &create_list(list_id(1), "A"))
        .unwrap();
    assert_eq!(r.epoch(), None);
    // An empty replica adopts the first epoch it sees, even from an empty page.
    r.apply_confirmed(EPOCH, &[]).unwrap();
    assert_eq!(r.epoch(), Some(EPOCH));
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    assert_eq!(open(&storage).epoch(), Some(EPOCH), "stored");
    let other = Uuid::from_u128(0xe1);
    server
        .execute(&owner(), Uuid::from_u128(2), &create_list(list_id(2), "B"))
        .unwrap();
    assert!(matches!(
        r.apply_confirmed(other, &server.log),
        Err(ConfirmError::EpochChanged)
    ));
    assert!(matches!(
        r.apply_confirmed(other, &[]),
        Err(ConfirmError::EpochChanged)
    ));
    assert_eq!(r.cursor(), 1, "nothing applied");
    r.reset().unwrap();
    assert_eq!(r.epoch(), None);
    assert_eq!(open(&storage).epoch(), None);
    r.apply_confirmed(other, &server.log).unwrap();
    assert_eq!(r.epoch(), Some(other));
}

#[test]
fn an_acknowledgement_below_the_cursor_for_a_pending_entry_means_divergence() {
    // The server was restored and regrew: its event at seq 3 is this entry's, but the replica's
    // seq 3 is another (lost) event.
    let (mut r, mut server, _) = seeded();
    server
        .execute(&owner(), Uuid::from_u128(1), &add_note(task_id(1), "lost"))
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    server.restore(2, Uuid::from_u128(0xe1));
    let e = entry(7, add_note(task_id(1), "mine"), 1);
    r.submit(e.clone()).unwrap();
    r.next_to_send().unwrap();
    let mut result = server
        .execute(&owner(), e.command_id, e.command().unwrap())
        .unwrap();
    assert_eq!(result.events[0].seq, 3);
    // The response names the restored log's epoch: that alone says not to record it.
    assert_eq!(
        r.acknowledge(e.command_id, &result).unwrap(),
        Ack::OtherEpoch
    );
    // Without an epoch (defence in depth), the sequence number gives it away.
    result.epoch = None;
    assert_eq!(r.acknowledge(e.command_id, &result).unwrap(), Ack::Diverged);
    assert_eq!(r.outbox()[0].acked, None, "nothing recorded");
}

#[test]
fn a_command_that_changed_nothing_leaves_on_acknowledgement() {
    let (mut r, mut server, _) = seeded();
    let complete = Command::CompleteTask { id: task_id(1) };
    server
        .execute(&owner(), Uuid::from_u128(7), &complete)
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    let e = entry(1, complete.clone(), 1);
    r.submit(e.clone()).unwrap();
    r.next_to_send().unwrap();
    let result = server.execute(&owner(), e.command_id, &complete).unwrap();
    assert!(result.events.is_empty());
    r.acknowledge(e.command_id, &result).unwrap();
    assert!(r.outbox().is_empty());
}

#[test]
fn a_rejected_command_moves_to_couldnt_sync_until_dismissed() {
    let (mut r, _server, storage) = seeded();
    let e = entry(1, add_note(task_id(1), "x"), 1);
    r.submit(e.clone()).unwrap();
    let rejection = Rejection {
        status: 409,
        code: "deleted".into(),
        message: "task is deleted".into(),
    };
    r.reject(e.command_id, rejection.clone(), ts(2)).unwrap();
    assert!(r.outbox().is_empty());
    assert_eq!(r.failed().len(), 1);
    assert_eq!(r.failed()[0].rejection, rejection);
    assert!(
        notes(&mut r, task_id(1)).is_empty(),
        "no longer in the view"
    );
    // Survives a restart.
    let mut again = open(&storage);
    assert_eq!(again.failed().len(), 1);
    assert!(again.dismiss(e.command_id).unwrap());
    assert!(!again.dismiss(e.command_id).unwrap());
    assert!(open(&storage).failed().is_empty());
}

#[test]
fn a_restart_resumes_from_the_cursor_and_keeps_the_outbox() {
    for snapshot_every in [1, 2, 1000] {
        let storage = MemoryStorage::new();
        let mut server = FakeServer::default();
        let mut r = open(&storage);
        r.set_snapshot_every(snapshot_every);
        server
            .execute(&owner(), Uuid::from_u128(1), &create_list(list_id(1), "A"))
            .unwrap();
        server
            .execute(
                &owner(),
                Uuid::from_u128(2),
                &create_task(task_id(1), list_id(1), "T"),
            )
            .unwrap();
        server
            .execute(&owner(), Uuid::from_u128(3), &add_note(task_id(1), "n"))
            .unwrap();
        r.apply_confirmed(EPOCH, &server.log).unwrap();
        r.submit(entry(10, add_note(task_id(1), "pending"), 1))
            .unwrap();
        r.next_to_send().unwrap();
        let cursor = r.cursor();
        let view = r.view();
        drop(r);

        let mut again = open(&storage);
        assert_eq!(again.cursor(), cursor, "snapshot_every {snapshot_every}");
        assert!(again.confirmed().data_eq(&server.state));
        assert_eq!(again.outbox().len(), 1);
        assert_eq!(again.outbox()[0].attempts, 1, "attempts are durable");
        assert!(again.view().data_eq(&view));
    }
}

#[test]
fn a_gap_and_a_divergence_are_reported() {
    let (mut r, mut server, _) = seeded();
    server
        .execute(&owner(), Uuid::from_u128(1), &add_note(task_id(1), "a"))
        .unwrap();
    server
        .execute(&owner(), Uuid::from_u128(2), &add_note(task_id(1), "b"))
        .unwrap();
    let last = server.log.last().unwrap().clone();
    assert!(matches!(
        r.apply_confirmed(EPOCH, std::slice::from_ref(&last)),
        Err(ConfirmError::Gap {
            expected: 3,
            got: 4
        })
    ));
    // An event that does not apply (here: the same event under a new sequence number).
    let mut dup = server.log[0].clone();
    dup.event.seq = Some(3);
    assert!(matches!(
        r.apply_confirmed(EPOCH, &[dup]),
        Err(ConfirmError::Diverged { seq: 3, .. })
    ));
    assert_eq!(r.cursor(), 2);
}

#[test]
fn reset_forgets_the_confirmed_state_but_keeps_the_outbox() {
    let (mut r, mut server, storage) = seeded();
    let e = entry(1, add_note(task_id(1), "x"), 1);
    r.submit(e.clone()).unwrap();
    r.next_to_send().unwrap();
    let result = server
        .execute(&owner(), e.command_id, e.command().unwrap())
        .unwrap();
    r.acknowledge(e.command_id, &result).unwrap();
    r.reset().unwrap();
    assert_eq!(r.cursor(), 0);
    assert!(r.confirmed().tasks.is_empty());
    assert_eq!(r.outbox().len(), 1);
    assert_eq!(r.outbox()[0].acked, None, "sent again after a resync");
    let again = open(&storage);
    assert_eq!(again.cursor(), 0);
    assert_eq!(again.outbox().len(), 1);
}

#[test]
fn a_failed_confirmation_write_changes_nothing_even_if_storage_cannot_be_read_either() {
    for snapshot_every in [1, 1000] {
        let (mut r, mut server, storage) = seeded();
        r.set_snapshot_every(snapshot_every);
        let e = entry(1, add_note(task_id(1), "once"), 1);
        r.submit(e.clone()).unwrap();
        server
            .execute(&owner(), e.command_id, e.command().unwrap())
            .unwrap();
        let (cursor, confirmed) = (r.cursor(), r.confirmed().clone());
        storage.set_fail_writes(true);
        storage.set_fail_loads(true);
        assert!(matches!(
            r.apply_confirmed(EPOCH, &server.log),
            Err(ConfirmError::Storage(_))
        ));
        assert_eq!(r.cursor(), cursor, "snapshot_every {snapshot_every}");
        assert!(r.confirmed().data_eq(&confirmed));
        assert_eq!(r.confirmed().journal, confirmed.journal);
        assert_eq!(r.outbox().len(), 1);
        assert_eq!(notes(&mut r, task_id(1)), "once");
        // Once storage works again, the same events go through.
        storage.set_fail_writes(false);
        storage.set_fail_loads(false);
        r.apply_confirmed(EPOCH, &server.log).unwrap();
        assert!(r.outbox().is_empty());
        assert!(open(&storage).confirmed().data_eq(&server.state));
    }
}

#[test]
fn a_late_acknowledgement_from_a_replaced_log_is_not_recorded() {
    // The command commits in epoch A, but its response arrives only after the replica resynced onto
    // epoch B, whose log (restored from a backup) does not have the command.
    let (mut r, mut server, storage) = seeded();
    let e = entry(1, add_note(task_id(1), "mine"), 1);
    r.submit(e.clone()).unwrap();
    r.next_to_send().unwrap();
    let late = server
        .execute(&owner(), e.command_id, e.command().unwrap())
        .unwrap();
    assert_eq!(late.epoch, Some(EPOCH));
    let new_epoch = Uuid::from_u128(0xe1);
    server.restore(2, new_epoch);
    assert!(matches!(
        r.apply_confirmed(new_epoch, &server.log),
        Err(ConfirmError::EpochChanged)
    ));
    r.reset().unwrap();
    r.apply_confirmed(new_epoch, &server.log).unwrap();

    assert_eq!(r.acknowledge(e.command_id, &late).unwrap(), Ack::OtherEpoch);
    assert_eq!(r.outbox()[0].acked, None, "not recorded");
    assert_eq!(open(&storage).outbox()[0].acked, None, "not stored");
    // So it is sent again, and the restored server applies it.
    let sent = r.next_to_send().unwrap().expect("sent again");
    let result = server
        .execute(&owner(), sent.command_id, sent.command().unwrap())
        .unwrap();
    assert_eq!(result.epoch, Some(new_epoch));
    assert_eq!(r.acknowledge(e.command_id, &result).unwrap(), Ack::Recorded);
    r.apply_confirmed(new_epoch, &server.log).unwrap();
    assert!(r.outbox().is_empty());
    assert_eq!(notes(&mut r, task_id(1)).matches("mine").count(), 1);
    assert!(r.view().data_eq(&server.state));
}

#[test]
fn an_acknowledgement_from_before_the_epoch_was_known_counts_only_for_its_own_epoch() {
    for same in [true, false] {
        let storage = MemoryStorage::new();
        let mut r = open(&storage);
        let mut server = FakeServer::default();
        let e = entry(1, create_list(list_id(1), "L"), 1);
        r.submit(e.clone()).unwrap();
        r.next_to_send().unwrap();
        let result = server
            .execute(&owner(), e.command_id, e.command().unwrap())
            .unwrap();
        // The replica has no epoch yet: recorded, with the response's epoch.
        assert_eq!(r.acknowledge(e.command_id, &result).unwrap(), Ack::Recorded);
        assert_eq!(r.outbox()[0].acked_epoch, Some(EPOCH));
        let epoch = if same {
            EPOCH
        } else {
            // Restored to an empty backup before the replica saw any log.
            server.restore(0, Uuid::from_u128(0xe1));
            server.epoch
        };
        r.apply_confirmed(epoch, &[]).unwrap();
        if same {
            assert!(r.next_to_send().unwrap().is_none(), "still acknowledged");
        } else {
            assert_eq!(r.outbox()[0].acked, None, "forgotten");
            assert_eq!(open(&storage).outbox()[0].acked, None, "and stored");
            assert!(r.next_to_send().unwrap().is_some(), "sent again");
        }
    }
}

#[test]
fn opening_forgets_acknowledgements_from_another_epoch() {
    let mut storage = MemoryStorage::new();
    let mut e = entry(1, create_list(list_id(1), "L"), 1);
    e.attempts = 1;
    e.acked = Some(vec![todo_core::ids::EventId::from_uuid(Uuid::from_u128(
        77,
    ))]);
    e.acked_epoch = Some(EPOCH);
    storage
        .write(&[
            StorageOp::SetEpoch(Uuid::from_u128(0xe1)),
            StorageOp::PutOutbox(&e),
        ])
        .unwrap();
    let mut r = open(&storage);
    assert_eq!(r.outbox()[0].acked, None);
    assert!(r.next_to_send().unwrap().is_some());
    assert_eq!(open(&storage).outbox()[0].acked, None, "stored");
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// However responses get lost and acknowledgements and confirmed events interleave, every command
    /// ends up applied once: the outbox drains, the replica's confirmed state equals the server's, and
    /// the view holds every note exactly once at every step.
    #[test]
    fn acknowledgements_and_events_in_any_order_apply_each_command_once(
        plan in prop::collection::vec((any::<bool>(), 0usize..4), 1..12)
    ) {
        let (mut r, mut server, _) = seeded();
        let mut delivered = server.log.len();
        for (i, (ack, deliver)) in plan.iter().enumerate() {
            let e = entry(100 + i as u128, add_note(task_id(1), &format!("[{i}]")), i as i64 + 10);
            r.submit(e).unwrap();
            // The sender retries the oldest unacknowledged entry; `ack == false` loses the response.
            if let Some(sent) = r.next_to_send().unwrap() {
                let result = server.execute(&owner(), sent.command_id, sent.command().unwrap()).unwrap();
                if *ack {
                    r.acknowledge(sent.command_id, &result).unwrap();
                }
            }
            let upto = (delivered + deliver).min(server.log.len());
            r.apply_confirmed(EPOCH, &server.log[delivered..upto]).unwrap();
            delivered = upto;
            let view = r.view();
            for j in 0..=i {
                prop_assert_eq!(view.tasks[&task_id(1)].notes.matches(&format!("[{j}]")).count(), 1);
            }
        }
        // The network recovers.
        while let Some(sent) = r.next_to_send().unwrap() {
            let result = server.execute(&owner(), sent.command_id, sent.command().unwrap()).unwrap();
            r.acknowledge(sent.command_id, &result).unwrap();
        }
        r.apply_confirmed(EPOCH, &server.log[delivered..]).unwrap();
        prop_assert!(r.outbox().is_empty());
        prop_assert_eq!(server.stored_commands(), 2 + plan.len());
        prop_assert!(r.confirmed().data_eq(&server.state));
        prop_assert!(r.view().data_eq(&server.state));
    }
}
