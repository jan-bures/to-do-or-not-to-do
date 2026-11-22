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
use todo_core::command::{NewTask, Placement};
use todo_core::ids::{StepId, TaskId};
use todo_core::recurrence::{Recurrence, Rule};
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

fn undo_after(n: u128, original: &OutboxEntry) -> OutboxEntry {
    OutboxEntry {
        command_id: Uuid::from_u128(n),
        op: Operation::UndoAfter {
            original: original.command_id,
        },
        ..original.clone()
    }
}

#[test]
fn queued_undo_is_in_the_view_before_and_after_acknowledgement() {
    let (mut r, mut server, storage) = seeded();
    let complete = entry(10, Command::CompleteTask { id: task_id(1) }, 1);
    r.submit(complete.clone()).unwrap();
    assert!(r.view().tasks[&task_id(1)].is_done());
    r.next_to_send().unwrap();
    r.submit(undo_after(11, &complete)).unwrap();
    assert!(!r.view().tasks[&task_id(1)].is_done());
    let ack = server
        .execute(&owner(), complete.command_id, complete.command().unwrap())
        .unwrap();
    r.acknowledge(complete.command_id, &ack).unwrap();
    assert!(!r.view().tasks[&task_id(1)].is_done());
    let mut reopened = open(&storage);
    assert!(!reopened.view().tasks[&task_id(1)].is_done());
    reopened
        .submit(entry(12, complete.command().unwrap().clone(), 2))
        .unwrap();
    assert!(reopened.view().tasks[&task_id(1)].is_done());
}

/// A recurring task that is not done yet.
fn repeat(task: todo_core::ids::TaskId) -> Command {
    let mut repeat = NewTask::new(task, list_id(1), "Repeat");
    repeat.due = Some("2026-11-21".parse().unwrap());
    repeat.recurrence = Some(Recurrence::new(Rule::Daily { interval: 1 }));
    Command::CreateTask(Box::new(repeat))
}

/// A page can end inside the original's batch. The batch waits until it is whole, so the undo stays
/// in the view throughout (predicted from the pending original), before and after a restart.
#[test]
fn an_undo_stays_in_the_view_while_a_page_splits_the_originals_batch() {
    let (mut r, mut server, storage) = seeded();
    let task = task_id(2);
    server
        .execute(&owner(), Uuid::from_u128(9000), &repeat(task))
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    let original = entry(100, Command::CompleteTask { id: task }, 1);
    r.submit(original.clone()).unwrap();
    r.next_to_send().unwrap().unwrap();
    r.submit(undo_after(101, &original)).unwrap();
    let undone = |r: &mut Replica| {
        let view = r.view();
        !view.tasks[&task].is_done() && view.tasks.len() == 2
    };
    assert!(undone(&mut r));
    let first = server.log.len();
    let receipt = server
        .execute(&owner(), original.command_id, original.command().unwrap())
        .unwrap();
    assert_eq!(
        receipt.events.len(),
        2,
        "the completion and the next repeat"
    );

    let cursor = r.cursor();
    assert_eq!(
        r.apply_confirmed(EPOCH, &server.log[first..=first])
            .unwrap(),
        0
    );
    assert_eq!(r.cursor(), cursor, "half a batch is not confirmed");
    assert_eq!(
        r.received(),
        receipt.events[0].seq,
        "fetching goes on after it"
    );
    assert_eq!(r.outbox().len(), 2, "the original is still pending");
    assert!(
        undone(&mut r),
        "undone while the page ends inside the batch"
    );
    let mut reopened = open(&storage);
    assert_eq!(
        reopened.received(),
        cursor,
        "a restart fetches the batch again"
    );
    assert!(undone(&mut reopened), "and after a restart");

    r.apply_confirmed(EPOCH, &server.log[first + 1..]).unwrap();
    reopened
        .apply_confirmed(EPOCH, &server.log[first..])
        .unwrap();
    for r in [&mut r, &mut reopened] {
        assert_eq!(r.cursor(), server.head());
        assert!(matches!(r.outbox()[0].op, Operation::Undo { .. }));
        assert!(undone(r), "undone once the rest of the batch arrived");
        // A late acknowledgement changes nothing.
        r.acknowledge(original.command_id, &receipt).unwrap();
        assert!(undone(r));
    }
    assert!(undone(&mut open(&storage)));
}

/// A task with two steps, merged into a task with one step; then the second moved step is put before
/// the target's own step. Returns `(source, target, [moved steps], target's step)`.
fn merge_fixture(
    r: &mut Replica,
    server: &mut FakeServer,
) -> (
    todo_core::ids::TaskId,
    todo_core::ids::TaskId,
    [StepId; 2],
    StepId,
) {
    let (source, target) = (task_id(2), task_id(3));
    let steps = [step_id(1), step_id(2)];
    let own = step_id(3);
    let mut commands = vec![
        create_task(source, list_id(1), "Source"),
        create_task(target, list_id(1), "Target"),
    ];
    for (id, task) in [(steps[0], source), (steps[1], source), (own, target)] {
        commands.push(Command::CreateStep {
            id,
            task,
            title: "Step".into(),
            placement: Placement::Last,
        });
    }
    for (i, command) in commands.iter().enumerate() {
        server
            .execute(&owner(), Uuid::from_u128(9100 + i as u128), command)
            .unwrap();
    }
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    (source, target, steps, own)
}

fn step_id(n: u128) -> StepId {
    StepId::from_uuid(Uuid::from_u128(0x5000 + n))
}

/// The steps of `task`, in order.
fn steps_of(view: &todo_core::State, task: todo_core::ids::TaskId) -> Vec<StepId> {
    let mut steps: Vec<_> = view
        .steps
        .values()
        .filter(|s| s.task == task && s.deleted.is_none())
        .collect();
    steps.sort_by(|a, b| a.position.cmp(&b.position));
    steps.into_iter().map(|s| s.id).collect()
}

/// Undoing part of a batch is not undoing the batch. Merge a task with two steps, put the second
/// moved step before the target's own step, then undo the merge: the server refuses that undo (the
/// reorder changed what the merge did), and a page that ends after the first moved step must not make
/// it look as if it succeeded, before or after a restart.
#[test]
fn a_page_inside_a_batch_never_shows_an_undo_the_whole_batch_refuses() {
    for ack_first in [false, true] {
        let (mut r, mut server, storage) = seeded();
        let (source, target, [s1, s2], own) = merge_fixture(&mut r, &mut server);
        let merge = entry(100, Command::MergeTasks { source, target }, 1);
        let reorder = entry(
            101,
            Command::Reorder {
                id: s2.into(),
                placement: Placement::Before(own.into()),
            },
            2,
        );
        r.submit(merge.clone()).unwrap();
        r.next_to_send().unwrap().unwrap();
        r.submit(reorder.clone()).unwrap();
        r.submit(undo_after(102, &merge)).unwrap();
        // Merged, reordered, and the undo (which conflicts) left out.
        let shown = |r: &mut Replica| {
            let view = r.view();
            view.tasks[&source].merged_into == Some(target)
                && steps_of(&view, target) == vec![s2, own, s1]
        };
        assert!(shown(&mut r), "ack_first {ack_first}");

        let first = server.log.len();
        let receipt = server
            .execute(&owner(), merge.command_id, merge.command().unwrap())
            .unwrap();
        assert!(receipt.events.len() > 2, "both steps move, then the source");
        if ack_first {
            r.acknowledge(merge.command_id, &receipt).unwrap();
            assert!(shown(&mut r), "acknowledged");
        }
        // The stream stops after the first step moved.
        r.apply_confirmed(EPOCH, &server.log[first..=first])
            .unwrap();
        assert!(shown(&mut r), "mid-batch, ack_first {ack_first}");
        assert!(
            shown(&mut open(&storage)),
            "after a restart, ack_first {ack_first}"
        );

        // The rest arrives, and the reorder is confirmed too.
        server
            .execute(&owner(), reorder.command_id, reorder.command().unwrap())
            .unwrap();
        let mut reopened = open(&storage);
        r.apply_confirmed(EPOCH, &server.log[first + 1..]).unwrap();
        reopened
            .apply_confirmed(EPOCH, &server.log[first..])
            .unwrap();
        for r in [&mut r, &mut reopened] {
            assert_eq!(r.outbox().len(), 1, "only the undo waits");
            assert!(shown(r), "whole batch, ack_first {ack_first}");
        }
    }
}

/// An undo's own compensating batch is confirmed as a whole too. With the original's events gone into
/// a snapshot, the undo can't be predicted and waits for confirmation; a page that ends after its
/// first event (removing the next repeat) must not settle it while the task is still done, neither in
/// memory nor after a restart.
#[test]
fn an_undo_settles_only_once_its_whole_compensation_arrived() {
    let (mut r, mut server, storage) = seeded();
    r.set_snapshot_every(1);
    let task = task_id(2);
    server
        .execute(&owner(), Uuid::from_u128(9000), &repeat(task))
        .unwrap();
    let done = server
        .execute(
            &owner(),
            Uuid::from_u128(9001),
            &Command::CompleteTask { id: task },
        )
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    assert!(
        storage.clone().load().unwrap().events.is_empty(),
        "in the snapshot"
    );
    let first_event = done.events[0];
    let undo = OutboxEntry {
        command_id: Uuid::from_u128(200),
        op: Operation::Undo {
            seq: first_event.seq,
            expected_event_id: first_event.id,
            epoch: Some(EPOCH),
            force: false,
        },
        submitted_at: ts(5),
        attempts: 0,
        acked: None,
        acked_epoch: None,
    };
    let mut r = open(&storage);
    r.submit(undo.clone()).unwrap();
    let before_undo = |r: &mut Replica| {
        let view = r.view();
        let live = view.tasks.values().filter(|t| t.deleted.is_none()).count();
        view.tasks[&task].is_done() && live == 3
    };
    assert!(before_undo(&mut r), "waits for the server");
    r.next_to_send().unwrap().unwrap();
    let from = server.log.len();
    let receipt = server
        .undo(&owner(), undo.command_id, first_event.seq)
        .unwrap();
    assert_eq!(
        receipt.events.len(),
        2,
        "the next repeat goes, the task reopens"
    );
    r.acknowledge(undo.command_id, &receipt).unwrap();

    r.apply_confirmed(EPOCH, &server.log[from..=from]).unwrap();
    assert_eq!(r.outbox().len(), 1, "half a compensation settles nothing");
    assert!(before_undo(&mut r), "never half undone");
    let mut reopened = open(&storage);
    assert_eq!(reopened.outbox().len(), 1, "after a restart");
    assert!(
        before_undo(&mut reopened),
        "never half undone after a restart"
    );

    r.apply_confirmed(EPOCH, &server.log[from + 1..]).unwrap();
    reopened
        .apply_confirmed(EPOCH, &server.log[from..])
        .unwrap();
    for r in [&mut r, &mut reopened, &mut open(&storage)] {
        assert!(r.outbox().is_empty());
        let view = r.view();
        assert!(!view.tasks[&task].is_done());
        assert!(view.data_eq(&server.state));
    }
}

#[test]
fn open_resolves_a_settled_commands_undo_from_the_first_replayed_event() {
    let (r, mut server, mut storage) = seeded();
    let original = entry(10, Command::CompleteTask { id: task_id(1) }, 1);
    let undo = undo_after(11, &original);
    let ack = server
        .execute(&owner(), original.command_id, original.command().unwrap())
        .unwrap();
    // Repair a cache that contains both the original and its already-confirmed receipt.
    storage
        .write(&[
            StorageOp::AppendEvents(&server.log[r.cursor() as usize..]),
            StorageOp::PutOutbox(&original),
            StorageOp::PutOutbox(&undo),
        ])
        .unwrap();
    let mut reopened = open(&storage);
    assert_eq!(reopened.outbox().len(), 1);
    let next = reopened.next_to_send().unwrap().unwrap();
    assert_eq!(next.command_id, undo.command_id);
    assert_eq!(
        next.op,
        Operation::Undo {
            seq: ack.events[0].seq,
            expected_event_id: ack.events[0].id,
            epoch: Some(EPOCH),
            force: false
        }
    );
    assert_eq!(open(&storage).outbox(), &[next]);
}

#[test]
fn open_fails_an_undo_if_only_a_snapshot_proves_the_original_settled() {
    let (_r, mut server, mut storage) = seeded();
    let mut original = entry(10, Command::CompleteTask { id: task_id(1) }, 1);
    let undo = undo_after(11, &original);
    let ack = server
        .execute(&owner(), original.command_id, original.command().unwrap())
        .unwrap();
    original.acked = Some(ack.events.iter().map(|e| e.id).collect());
    original.acked_epoch = Some(EPOCH);
    storage
        .write(&[
            StorageOp::Snapshot {
                state: &server.state,
                cursor: server.head(),
            },
            StorageOp::PutOutbox(&original),
            StorageOp::PutOutbox(&undo),
        ])
        .unwrap();
    storage.set_fail_writes(true);
    assert!(Replica::open(owner(), Box::new(storage.clone())).is_err());
    assert_eq!(storage.load().unwrap().outbox.len(), 2);
    storage.set_fail_writes(false);
    let mut reopened = open(&storage);
    assert!(reopened.outbox().is_empty());
    assert!(reopened.next_to_send().unwrap().is_none());
    assert_eq!(reopened.failed()[0].entry, undo);
    assert_eq!(reopened.failed()[0].rejection.code, "undo_no_event");
    assert_eq!(open(&storage).failed(), reopened.failed());
}

#[test]
fn adopting_a_refusal_fails_the_dependent_undo_atomically() {
    let (mut r, _, storage) = seeded();
    let original = entry(10, Command::CompleteTask { id: task_id(1) }, 1);
    let undo = undo_after(11, &original);
    r.submit(original.clone()).unwrap();
    r.submit(undo.clone()).unwrap();
    let refusal = todo_client::outbox::FailedEntry {
        entry: original,
        failed_at: ts(2),
        rejection: Rejection {
            status: 409,
            code: "conflict".into(),
            message: "conflict".into(),
        },
    };
    storage.set_fail_writes(true);
    assert!(r.adopt_failed(vec![refusal.clone()]).is_err());
    assert_eq!(r.outbox().len(), 2);
    storage.set_fail_writes(false);
    assert_eq!(r.adopt_failed(vec![refusal]).unwrap(), 1);
    assert!(r.outbox().is_empty());
    let reopened = open(&storage);
    assert!(reopened.outbox().is_empty());
    assert!(
        reopened
            .failed()
            .iter()
            .any(|f| f.entry == undo && f.rejection.code == "undo_no_event")
    );
}

#[test]
fn an_orphaned_undo_does_not_block_the_next_command() {
    let (mut r, _, storage) = seeded();
    let original = entry(10, Command::CompleteTask { id: task_id(1) }, 1);
    let undo = undo_after(11, &original);
    let next = entry(12, add_note(task_id(1), "still sends"), 2);
    r.adopt(vec![next.clone(), undo.clone()]).unwrap();
    assert_eq!(
        r.next_to_send().unwrap().unwrap().command_id,
        next.command_id
    );
    assert_eq!(r.failed()[0].entry, undo);
    assert_eq!(open(&storage).failed()[0].rejection.code, "undo_no_event");
}

#[test]
fn undo_after_survives_restart_and_resolves_only_against_its_command() {
    let (mut r, mut server, storage) = seeded();
    let mine = entry(22, Command::CompleteTask { id: task_id(1) }, 1);
    r.submit(mine.clone()).unwrap();
    assert_eq!(
        r.next_to_send().unwrap().unwrap().command_id,
        mine.command_id
    );
    let undo = OutboxEntry {
        command_id: Uuid::from_u128(23),
        op: Operation::UndoAfter {
            original: mine.command_id,
        },
        ..mine.clone()
    };
    r.submit(undo.clone()).unwrap();
    drop(r);
    let mut r = open(&storage);
    assert_eq!(
        r.outbox(),
        [
            OutboxEntry {
                attempts: 1,
                ..mine.clone()
            },
            undo.clone()
        ]
    );
    // An unrelated completion is not an undo target, even if it has identical command content.
    let another = Uuid::from_u128(24);
    server
        .execute(&owner(), another, mine.command().unwrap())
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    assert_eq!(r.outbox()[1].op, undo.op);
    server
        .execute(
            &owner(),
            Uuid::from_u128(25),
            &Command::UncompleteTask { id: task_id(1) },
        )
        .unwrap();
    let result = server
        .execute(&owner(), mine.command_id, mine.command().unwrap())
        .unwrap();

    // The stream beats the POST response. The receipt and undo must become durable together.
    storage.set_fail_writes(true);
    assert!(matches!(
        r.apply_confirmed(EPOCH, &server.log),
        Err(ConfirmError::Storage(_))
    ));
    assert_eq!(r.outbox()[1].op, undo.op);
    storage.set_fail_writes(false);
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    drop(r);
    let mut reopened = open(&storage);
    let ready = reopened.next_to_send().unwrap().unwrap();
    assert_eq!(ready.command_id, undo.command_id);
    assert_eq!(
        ready.op,
        Operation::Undo {
            seq: result.events[0].seq,
            expected_event_id: result.events[0].id,
            epoch: Some(EPOCH),
            force: false
        }
    );
}

#[test]
fn undo_after_a_noop_does_not_send_an_undo_for_a_different_change() {
    let (mut r, mut server, _) = seeded();
    let mine = entry(31, Command::UncompleteTask { id: task_id(1) }, 1);
    r.submit(mine.clone()).unwrap();
    let undo_id = Uuid::from_u128(32);
    r.submit(OutboxEntry {
        command_id: undo_id,
        op: Operation::UndoAfter {
            original: mine.command_id,
        },
        ..mine.clone()
    })
    .unwrap();
    let result = server
        .execute(&owner(), mine.command_id, mine.command().unwrap())
        .unwrap();
    assert!(result.events.is_empty());
    r.acknowledge(mine.command_id, &result).unwrap();
    assert!(r.next_to_send().unwrap().is_none());
    assert_eq!(r.failed()[0].entry.command_id, undo_id);
    assert_eq!(r.failed()[0].rejection.code, "undo_no_event");
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

/// A cache damaged so that it has a completed repeat's next occurrence but not the task itself, and
/// an undo of that completion in the outbox. Returns the storage, the undo, the server, the cursor of
/// the damaged state, the next occurrence, and where the undo's own events start in the log.
fn damaged_cache_with_an_undo() -> (MemoryStorage, OutboxEntry, FakeServer, u64, TaskId, usize) {
    let (mut r, mut server, _) = seeded();
    let task = task_id(2);
    server
        .execute(&owner(), Uuid::from_u128(9000), &repeat(task))
        .unwrap();
    let done = server
        .execute(
            &owner(),
            Uuid::from_u128(9001),
            &Command::CompleteTask { id: task },
        )
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    let mut damaged = r.confirmed().clone();
    damaged.tasks.remove(&task);
    let next = *damaged.tasks.keys().find(|t| **t != task_id(1)).unwrap();
    let first_event = done.events[0];
    let undo = OutboxEntry {
        command_id: Uuid::from_u128(200),
        op: Operation::Undo {
            seq: first_event.seq,
            expected_event_id: first_event.id,
            epoch: Some(EPOCH),
            force: false,
        },
        submitted_at: ts(5),
        attempts: 1,
        acked: None,
        acked_epoch: None,
    };
    let mut storage = MemoryStorage::new();
    storage
        .write(&[
            StorageOp::SetEpoch(EPOCH),
            StorageOp::Snapshot {
                state: &damaged,
                cursor: server.head(),
            },
            StorageOp::PutOutbox(&undo),
        ])
        .unwrap();
    let cursor = server.head();
    let from = server.log.len();
    let receipt = server
        .undo(&owner(), undo.command_id, first_event.seq)
        .unwrap();
    assert_eq!(
        receipt.events.len(),
        2,
        "the next repeat goes, then the task reopens"
    );
    (storage, undo, server, cursor, next, from)
}

fn live(state: &todo_core::State, task: TaskId) -> bool {
    state.tasks.get(&task).is_some_and(|t| t.deleted.is_none())
}

/// A batch whose second event doesn't apply is not applied at all: its first event is neither
/// applied nor stored, and the command's outbox entry is not settled.
#[test]
fn a_divergence_inside_a_batch_discards_the_whole_batch() {
    let (storage, undo, server, cursor, next, from) = damaged_cache_with_an_undo();
    let mut r = open(&storage);
    assert!(live(r.confirmed(), next));
    assert!(matches!(
        r.apply_confirmed(EPOCH, &server.log[from..]),
        Err(ConfirmError::Diverged { .. })
    ));
    for r in [&mut r, &mut open(&storage)] {
        assert_eq!(r.cursor(), cursor, "nothing of the batch is confirmed");
        assert_eq!(r.received(), cursor, "nor held");
        assert!(live(r.confirmed(), next), "the next repeat is still there");
        assert_eq!(r.outbox(), std::slice::from_ref(&undo), "not settled");
    }
}

/// Earlier whole batches stay applied when a later one diverges.
#[test]
fn a_divergence_keeps_the_whole_batches_before_it() {
    let (storage, undo, mut server, cursor, next, from) = damaged_cache_with_an_undo();
    // A command between the undo's events would break the log; put one before them instead.
    let mut log = server.log[..from].to_vec();
    let note = server
        .execute(
            &owner(),
            Uuid::from_u128(300),
            &add_note(task_id(1), "kept"),
        )
        .unwrap();
    let mut note_event = server.log.last().unwrap().clone();
    note_event.event.seq = Some(cursor + 1);
    assert_eq!(note.events.len(), 1);
    log.push(note_event);
    for e in &server.log[from..from + 2] {
        let mut e = e.clone();
        e.event.seq = e.event.seq.map(|s| s + 1);
        log.push(e);
    }
    let mut r = open(&storage);
    assert!(matches!(
        r.apply_confirmed(EPOCH, &log[from..]),
        Err(ConfirmError::Diverged { .. })
    ));
    for r in [&mut r, &mut open(&storage)] {
        assert_eq!(r.cursor(), cursor + 1, "the note's batch is confirmed");
        assert_eq!(r.confirmed().tasks[&task_id(1)].notes, "kept");
        assert!(live(r.confirmed(), next));
        assert_eq!(r.outbox(), std::slice::from_ref(&undo));
    }
}

/// The same holds for stored events replayed on opening: a stored batch that doesn't apply as a whole
/// is left out, with the outbox entry it would settle kept.
#[test]
fn a_stored_batch_that_diverges_is_replayed_not_at_all() {
    let (mut storage, undo, server, cursor, next, from) = damaged_cache_with_an_undo();
    storage
        .write(&[StorageOp::AppendEvents(&server.log[from..])])
        .unwrap();
    let r = open(&storage);
    assert_eq!(r.cursor(), cursor, "refetched from the batch's start");
    assert!(live(r.confirmed(), next));
    assert_eq!(r.outbox(), std::slice::from_ref(&undo), "not settled");
}

/// Should stored events end inside a batch, the batch is fetched again rather than half replayed.
#[test]
fn stored_events_that_end_inside_a_batch_are_fetched_again() {
    let (r, mut server, mut storage) = seeded();
    let cursor = r.cursor();
    let task = task_id(2);
    server
        .execute(&owner(), Uuid::from_u128(9000), &repeat(task))
        .unwrap();
    let from = server.log.len();
    server
        .execute(
            &owner(),
            Uuid::from_u128(9001),
            &Command::CompleteTask { id: task },
        )
        .unwrap();
    storage
        .write(&[StorageOp::AppendEvents(&server.log[cursor as usize..=from])])
        .unwrap();
    let r = open(&storage);
    assert_eq!(
        r.cursor(),
        cursor + 1,
        "the creation, not half the completion"
    );
    assert!(!r.confirmed().tasks[&task].is_done());
}

/// A batch that arrives one event at a time is held until its last event, and a failed write then
/// leaves the held start as it was, so the same events go through once storage works again.
#[test]
fn a_batch_streamed_event_by_event_survives_a_failed_write_at_its_end() {
    let (mut r, mut server, storage) = seeded();
    let (source, target, _, _) = merge_fixture(&mut r, &mut server);
    let cursor = r.cursor();
    let from = server.log.len();
    server
        .execute(
            &owner(),
            Uuid::from_u128(100),
            &Command::MergeTasks { source, target },
        )
        .unwrap();
    let last = server.log.len() - 1;
    assert!(last - from >= 2, "a batch of three or more");
    for e in &server.log[from..last] {
        assert_eq!(
            r.apply_confirmed(EPOCH, std::slice::from_ref(e)).unwrap(),
            0
        );
        assert_eq!(r.received(), e.event.seq.unwrap());
    }
    assert_eq!(r.cursor(), cursor);
    storage.set_fail_writes(true);
    assert!(matches!(
        r.apply_confirmed(EPOCH, &server.log[last..]),
        Err(ConfirmError::Storage(_))
    ));
    assert_eq!(r.cursor(), cursor);
    assert_eq!(r.received(), server.log[last - 1].event.seq.unwrap());
    storage.set_fail_writes(false);
    assert_eq!(
        r.apply_confirmed(EPOCH, &server.log[last..]).unwrap(),
        last + 1 - from
    );
    assert_eq!(r.received(), server.head());
    for r in [&mut r, &mut open(&storage)] {
        assert_eq!(r.cursor(), server.head());
        assert!(r.confirmed().data_eq(&server.state));
    }
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

    /// An undo queued before its original's outcome is known binds to the first event of the
    /// original's own batch, however the acknowledgement, the stream pages (split anywhere in the
    /// batch), a restart and a failed write interleave. Throughout, the view is the one a fresh
    /// replica builds from storage, and it shows the undo as the whole batch's undo: the recurring
    /// completion undone as a whole, never half of it, and the merge (whose undo conflicts with a
    /// later reorder) never undone at all.
    #[test]
    fn early_undo_binds_only_to_the_own_batch_first_event(
        ack_at in 0..4u8,
        split in any::<prop::sample::Index>(),
        restart_at in 0..5u8,
        fail_once in any::<bool>(),
        outcome in 0..3u8,
        merge in any::<bool>(),
    ) {
        let (mut r, mut server, storage) = seeded();
        let task = task_id(2);
        let (original, reorder, outcome) = if merge {
            let (source, target, [_, s2], own) = merge_fixture(&mut r, &mut server);
            let reorder = entry(104, Command::Reorder {
                id: s2.into(),
                placement: Placement::Before(own.into()),
            }, 2);
            (entry(100, Command::MergeTasks { source, target }, 1), Some(reorder), 0)
        } else {
            server.execute(&owner(), Uuid::from_u128(9000), &repeat(task)).unwrap();
            r.apply_confirmed(EPOCH, &server.log).unwrap();
            (entry(100, Command::CompleteTask { id: task }, 1), None, outcome)
        };
        let undo = undo_after(101, &original);
        r.submit(original.clone()).unwrap();
        r.next_to_send().unwrap().unwrap();
        if let Some(reorder) = &reorder {
            r.submit(reorder.clone()).unwrap();
        }
        r.submit(undo.clone()).unwrap();

        if !merge {
            // Another session of the same actor performs the exact command first.
            server.execute(&owner(), Uuid::from_u128(102), original.command().unwrap()).unwrap();
            r.apply_confirmed(EPOCH, &server.log).unwrap();
            prop_assert_eq!(&r.outbox()[1], &undo);
        }
        let before_original = match outcome {
            _ if merge => None,
            0 => Some(Command::UncompleteTask { id: task }), // our command still changes the task
            1 => None, // our command is a no-op
            _ => Some(Command::Delete { id: task.into() }), // our command is refused
        };
        if let Some(command) = before_original {
            server.execute(&owner(), Uuid::from_u128(103), &command).unwrap();
            r.apply_confirmed(EPOCH, &server.log).unwrap();
        }
        // The view must show the original undone: the state before it, whatever it does. Or, for
        // the merge, the merge and the reorder, since the server refuses that undo.
        let before = server.state.clone();
        let check = |r: &mut Replica| -> Result<(), TestCaseError> {
            let view = r.view();
            prop_assert!(view.data_eq(&open(&storage).view()), "the view is what storage gives");
            if merge {
                let (source, target) = (task_id(2), task_id(3));
                prop_assert_eq!(view.tasks[&source].merged_into, Some(target), "never undone");
                prop_assert_eq!(
                    steps_of(&view, target),
                    vec![step_id(2), step_id(3), step_id(1)],
                    "merged and reordered"
                );
                return Ok(());
            }
            prop_assert!(view.tasks.keys().eq(before.tasks.keys()), "the whole batch is undone");
            prop_assert_eq!(
                view.tasks.get(&task).map(|t| (t.is_done(), t.deleted.is_some())),
                before.tasks.get(&task).map(|t| (t.is_done(), t.deleted.is_some())),
            );
            Ok(())
        };
        check(&mut r)?;
        let first = server.log.len();
        let result = server.execute(&owner(), original.command_id, original.command().unwrap());
        if let Some(reorder) = &reorder {
            // The reorder reaches the server after the merge; its event follows the batch.
            server.execute(&owner(), reorder.command_id, reorder.command().unwrap()).unwrap();
        }
        let mut acked = false;
        let mut fail = fail_once;
        // Four checkpoints admit an ack before the pages, between them, after them, or lost,
        // and a restart before, between, or after applying the original's receipts.
        for stage in 0..=4 {
            if stage == restart_at {
                r = open(&storage);
                check(&mut r)?;
            }
            if outcome != 0 {
                if stage != 2 { continue; }
                let mut settle = || match &result {
                    Ok(ack) => r.acknowledge(original.command_id, ack).map(drop),
                    Err((status, code)) => r.reject(original.command_id, Rejection {
                        status: *status, code: (*code).into(), message: String::new(),
                    }, ts(3)),
                };
                if fail {
                    storage.set_fail_writes(true);
                    prop_assert!(settle().is_err());
                    storage.set_fail_writes(false);
                }
                settle().unwrap();
                check(&mut r)?;
                continue;
            }
            let receipt = result.as_ref().unwrap();
            prop_assert!(receipt.events.len() > 1);
            let last_seq = receipt.events.last().unwrap().seq;
            // The first page ends anywhere in the batch, or after all of it. Each page starts where
            // the replica's fetching goes on (after a restart, that is the cursor again).
            let cut = first + 1 + split.index(receipt.events.len());
            let ends = [cut, server.log.len()];
            let mut steps = Vec::new();
            if stage == ack_at && ack_at < 3 { steps.push(None); }
            if matches!(stage, 0 | 1) && (r.received() as usize) < ends[stage as usize] {
                steps.push(Some(ends[stage as usize]));
            }
            for step in steps {
                let page = step.map(|end| &server.log[r.received() as usize..end]);
                let outbox = r.outbox().to_vec();
                let cursor = r.cursor();
                let apply_step = |r: &mut Replica| match page {
                    Some(page) => r.apply_confirmed(EPOCH, page).map(drop).map_err(|e| e.to_string()),
                    None => r.acknowledge(original.command_id, receipt).map(drop).map_err(|e| e.to_string()),
                };
                if fail {
                    storage.set_fail_writes(true);
                    let failed = apply_step(&mut r).is_err();
                    storage.set_fail_writes(false);
                    if failed {
                        prop_assert_eq!(r.outbox(), &outbox);
                        prop_assert_eq!(r.cursor(), cursor);
                        prop_assert_eq!(storage.clone().load().unwrap().outbox, outbox);
                        fail = false;
                    } else {
                        // Nothing to write: the page only started the batch, which waits.
                        prop_assert!(page.is_some());
                        prop_assert_eq!(r.cursor(), cursor);
                    }
                    check(&mut r)?;
                }
                apply_step(&mut r).unwrap();
                acked |= page.is_none();
                check(&mut r)?;
                // Bound by the acknowledgement or once the whole batch is confirmed, never earlier.
                let dependent = r.outbox().iter().find(|e| e.command_id == undo.command_id).unwrap();
                if acked || r.cursor() >= last_seq {
                    prop_assert_eq!(&dependent.op, &Operation::Undo {
                        seq: receipt.events[0].seq,
                        expected_event_id: receipt.events[0].id,
                        epoch: Some(EPOCH),
                        force: false,
                    });
                } else {
                    prop_assert_eq!(&dependent.op, &undo.op);
                }
            }
        }
        let reopened = open(&storage);
        prop_assert!(r.outbox().iter().chain(reopened.outbox()).all(|e|
            e.command_id != original.command_id && !matches!(e.op, Operation::UndoAfter { .. })
        ), "no unresolved dependency survives the original" );
        prop_assert_eq!(r.outbox(), reopened.outbox());
        prop_assert!(reopened.confirmed().data_eq(&server.state));
        if outcome != 0 {
            prop_assert!(r.outbox().is_empty());
            prop_assert!(reopened.failed().iter().any(|f|
                f.entry.command_id == undo.command_id && f.rejection.code == "undo_no_event"
            ));
        }
    }

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

#[test]
fn adopted_entries_slot_in_by_submission_time_behind_what_was_sent() {
    let (mut r, _server, storage) = seeded();
    r.submit(entry(1, add_note(task_id(1), "a"), 10)).unwrap();
    r.submit(entry(3, add_note(task_id(1), "c"), 30)).unwrap();
    // Entry 1 is in flight.
    assert_eq!(
        r.next_to_send().unwrap().unwrap().command_id,
        Uuid::from_u128(1)
    );
    // Another tab queued 0 (before 1, but 1 is already sent), 2 and 4.
    let theirs = vec![
        entry(4, add_note(task_id(1), "d"), 40),
        entry(0, add_note(task_id(1), "z"), 5),
        entry(2, add_note(task_id(1), "b"), 20),
    ];
    assert_eq!(r.adopt(theirs.clone()).unwrap(), 3);
    let order: Vec<u128> = r.outbox().iter().map(|e| e.command_id.as_u128()).collect();
    assert_eq!(order, [1, 0, 2, 3, 4]);
    // The view applies them in that order, and storage has them.
    let n = notes(&mut r, task_id(1));
    let at = |t: &str| n.find(t).unwrap();
    assert!(at("a") < at("z") && at("z") < at("b") && at("b") < at("c") && at("c") < at("d"));
    assert_eq!(storage.clone().load().unwrap().outbox.len(), 5);
    // Adopting again changes nothing.
    assert_eq!(r.adopt(theirs).unwrap(), 0);
    assert_eq!(r.outbox().len(), 5);
}

#[test]
fn entries_already_settled_or_refused_are_not_adopted() {
    let (mut r, mut server, _) = seeded();
    let mine = entry(1, add_note(task_id(1), "a"), 10);
    r.submit(mine.clone()).unwrap();
    // Confirmed: it leaves the outbox, and a tab still listing it does not bring it back.
    server
        .execute(&owner(), mine.command_id, mine.command().unwrap())
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    assert!(r.outbox().is_empty());
    assert_eq!(r.adopt(vec![mine]).unwrap(), 0);
    // Refused: likewise.
    let bad = entry(2, add_note(task_id(1), "b"), 20);
    r.submit(bad.clone()).unwrap();
    r.reject(
        bad.command_id,
        Rejection {
            status: 409,
            code: "deleted".into(),
            message: String::new(),
        },
        ts(30),
    )
    .unwrap();
    assert_eq!(r.adopt(vec![bad]).unwrap(), 0);
    assert!(r.outbox().is_empty());
}

#[test]
fn an_entry_another_tab_saw_refused_leaves_the_outbox() {
    let (mut r, _server, storage) = seeded();
    let e = entry(1, add_note(task_id(1), "a"), 10);
    r.submit(e.clone()).unwrap();
    let failed = todo_client::FailedEntry {
        entry: e.clone(),
        failed_at: ts(20),
        rejection: Rejection {
            status: 409,
            code: "deleted".into(),
            message: String::new(),
        },
    };
    assert_eq!(r.adopt_failed(vec![failed.clone()]).unwrap(), 1);
    assert!(r.outbox().is_empty());
    assert_eq!(r.failed(), std::slice::from_ref(&failed));
    let stored = storage.clone().load().unwrap();
    assert!(stored.outbox.is_empty());
    assert_eq!(stored.failed.len(), 1);
    assert_eq!(r.adopt_failed(vec![failed]).unwrap(), 0);
    assert!(notes(&mut r, task_id(1)).is_empty());
}

#[test]
fn a_new_sender_merges_its_own_queue_with_what_another_tab_tried_by_submission_time() {
    // This tab queued a rename at 20 while another tab was the sender; that tab had already tried
    // its own rename from 10 (attempts 3) when it closed. This tab never sent anything, so the older
    // rename goes first.
    let (mut r, _server, _) = seeded();
    r.submit(entry(2, add_note(task_id(1), "newer"), 20))
        .unwrap();
    let mut tried = entry(1, add_note(task_id(1), "older"), 10);
    tried.attempts = 3;
    assert_eq!(r.adopt(vec![tried]).unwrap(), 1);
    let order: Vec<u128> = r.outbox().iter().map(|e| e.command_id.as_u128()).collect();
    assert_eq!(order, [1, 2]);
    assert_eq!(
        r.next_to_send().unwrap().unwrap().command_id,
        Uuid::from_u128(1)
    );
}

#[test]
fn a_replayed_command_that_is_already_confirmed_just_leaves() {
    // The command was confirmed; a replica that no longer remembers it (reopened) adopts it from
    // another tab's record, sends it, and the server replays the old result.
    let (mut r, mut server, storage) = seeded();
    let e = entry(1, add_note(task_id(1), "once"), 10);
    let result = server
        .execute(&owner(), e.command_id, e.command().unwrap())
        .unwrap();
    r.apply_confirmed(EPOCH, &server.log).unwrap();
    drop(r);
    let mut r = open(&storage);
    assert_eq!(r.adopt(vec![e.clone()]).unwrap(), 1);
    assert_eq!(r.next_to_send().unwrap().unwrap().command_id, e.command_id);
    assert_eq!(r.acknowledge(e.command_id, &result).unwrap(), Ack::Recorded);
    assert!(r.outbox().is_empty(), "no resync, and nothing left to send");
    assert_eq!(notes(&mut r, task_id(1)), "once");
    // An event at or below the cursor that the confirmed state lacks is still a divergence.
    let other = entry(2, add_note(task_id(1), "x"), 20);
    r.submit(other.clone()).unwrap();
    let mut forged = result.clone();
    forged.command_id = other.command_id;
    forged.events[0].id = todo_core::ids::EventId::from_uuid(Uuid::from_u128(77));
    assert_eq!(
        r.acknowledge(other.command_id, &forged).unwrap(),
        Ack::Diverged
    );
}
