//! The local replica: confirmed state, event cursor, outbox and the optimistic view. No I/O besides the
//! [`Storage`] it owns, so every rule here is unit-testable without a network.
//!
//! # The view
//!
//! The view is the *confirmed* state (the fold of the server's events up to the cursor) with every
//! pending outbox command decided and applied on top, in order, with `todo-core`'s own
//! [`decide()`]/[`apply()`]. Each command is decided with its submission time as `now`, so the view is a
//! pure function of the confirmed state and the outbox and comes out the same every time it is rebuilt.
//! A pending command that no longer applies (the server state moved under it) is simply left out of
//! the view; the server decides its fate. Undos are not applied optimistically (that would need the
//! undone command's events); their effect shows once confirmed.
//!
//! # Leaving the outbox
//!
//! Server events do not carry the command ID. An entry leaves the outbox when the first of *its*
//! events reaches the confirmed state, because from then on the confirmed state includes its effect
//! and deciding it again on top would apply it twice. The engine learns an entry's events in two ways:
//!
//! - **Acknowledgement.** The 200 response lists the stored events. A command that changed nothing
//!   leaves at once.
//! - **Recognition.** The response can be lost, or arrive after the stream delivered the events. Event
//!   IDs are derived from the actor, the command and the server's `now`, and the server decides against
//!   exactly the state the confirmed state has just before the event. So for a command that was sent
//!   but not yet acknowledged, deciding it against the confirmed state as this client's actor at the
//!   event's time reproduces the event's ID exactly when the event is that command's. (The one
//!   ambiguity: two sessions sharing a device ID that send byte-identical commands at about the same
//!   time. One of them may then take the other's event for its own.)

use crate::outbox::{CommandId, FailedEntry, OutboxEntry, Rejection};
use crate::protocol::CommandResult;
use crate::storage::{Storage, StorageError, StorageOp};
use std::fmt;
use std::sync::Arc;
use todo_core::error::{ApplyError, DomainError};
use todo_core::event::Event;
use todo_core::time::Timestamp;
use todo_core::{Actor, State, apply, decide};

/// Default number of confirmed events between snapshots.
pub const SNAPSHOT_EVERY: usize = 500;

/// The stored data could not be turned into a replica.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// Reading failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// A submission was refused locally.
#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    /// The command is invalid against the current view (it would be refused by the server too).
    #[error(transparent)]
    Invalid(#[from] DomainError),
    /// The outbox could not be written, so the command was not accepted.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Confirmed events could not be applied.
#[derive(Debug, thiserror::Error)]
pub enum ConfirmError {
    /// The server sent an event without a sequence number.
    #[error("event without a sequence number")]
    MissingSeq,
    /// Events are missing between the cursor and this event.
    #[error("expected event {expected}, got {got}")]
    Gap {
        /// The next sequence number the replica needs.
        expected: u64,
        /// The one it got.
        got: u64,
    },
    /// The event does not apply to the confirmed state: the replica no longer matches the server and
    /// needs a full resync.
    #[error("confirmed state diverged from the server: {0}")]
    Diverged(#[from] ApplyError),
    /// Applied in memory, but storing failed. The next batch writes a full snapshot.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// What [`Replica::acknowledge`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    /// Recorded (or the entry was already gone).
    Recorded,
    /// The server's log differs from the confirmed state: resync. Nothing was recorded.
    Diverged,
}

/// Confirmed state, cursor, outbox and "couldn't sync" list, persisted in a [`Storage`].
pub struct Replica {
    actor: Actor,
    storage: Box<dyn Storage>,
    confirmed: State,
    cursor: u64,
    outbox: Vec<OutboxEntry>,
    failed: Vec<FailedEntry>,
    /// The cached view; `None` when it must be rebuilt.
    view: Option<Arc<State>>,
    /// Confirmed events stored since the last snapshot.
    unsnapshotted: usize,
    snapshot_every: usize,
    /// A write of confirmed events failed: the next write stores a full snapshot instead.
    needs_snapshot: bool,
}

impl fmt::Debug for Replica {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Replica")
            .field("actor", &self.actor)
            .field("cursor", &self.cursor)
            .field("outbox", &self.outbox.len())
            .field("failed", &self.failed.len())
            .finish_non_exhaustive()
    }
}

impl Replica {
    /// Loads the replica: the snapshot, then the stored events after it.
    ///
    /// Replay stops at the first event that does not follow on or does not apply; the rest is fetched
    /// from the server again.
    pub fn open(actor: Actor, mut storage: Box<dyn Storage>) -> Result<Self, OpenError> {
        let loaded = storage.load()?;
        let (mut confirmed, mut cursor) = loaded.snapshot.unwrap_or_default();
        let mut replayed = 0;
        let mut needs_snapshot = false;
        for e in &loaded.events {
            let Some(seq) = e.seq else { continue };
            if seq <= cursor {
                continue;
            }
            if seq != cursor + 1 {
                tracing::warn!(cursor, seq, "stored events have a gap; refetching the rest");
                needs_snapshot = true;
                break;
            }
            let mut next = confirmed.clone();
            if let Err(err) = apply(&mut next, e) {
                tracing::warn!(seq, error = %err, "a stored event does not apply; refetching from it");
                needs_snapshot = true;
                break;
            }
            confirmed = next;
            cursor = seq;
            replayed += 1;
        }
        tracing::debug!(
            cursor,
            replayed,
            outbox = loaded.outbox.len(),
            "replica loaded"
        );
        Ok(Self {
            actor,
            storage,
            confirmed,
            cursor,
            outbox: loaded.outbox,
            failed: loaded.failed,
            view: None,
            unsnapshotted: replayed,
            snapshot_every: SNAPSHOT_EVERY,
            needs_snapshot,
        })
    }

    /// Changes how many confirmed events are stored between snapshots.
    pub fn set_snapshot_every(&mut self, n: usize) {
        self.snapshot_every = n.max(1);
    }

    /// The actor the view decides pending commands as.
    #[must_use]
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// The last confirmed sequence number.
    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// The confirmed state.
    #[must_use]
    pub fn confirmed(&self) -> &State {
        &self.confirmed
    }

    /// Pending entries, oldest first.
    #[must_use]
    pub fn outbox(&self) -> &[OutboxEntry] {
        &self.outbox
    }

    /// Refused entries, oldest first.
    #[must_use]
    pub fn failed(&self) -> &[FailedEntry] {
        &self.failed
    }

    /// The confirmed state with the pending commands applied on top.
    pub fn view(&mut self) -> Arc<State> {
        if let Some(v) = &self.view {
            return v.clone();
        }
        let v = Arc::new(self.build_view());
        self.view = Some(v.clone());
        v
    }

    fn build_view(&self) -> State {
        let mut view = self.confirmed.clone();
        for entry in &self.outbox {
            if let Some(command) = entry.command() {
                overlay(&mut view, &self.actor, entry, command);
            }
        }
        view
    }

    /// Adds an entry to the outbox (durably, before it is accepted).
    ///
    /// A command is first decided against the view; one that is already invalid there is refused
    /// with the domain error and not queued.
    pub fn submit(&mut self, entry: OutboxEntry) -> Result<(), SubmitError> {
        let mut decided = None;
        if let Some(command) = entry.command() {
            let view = self.view();
            decided = Some(decide(&view, &self.actor, command, entry.submitted_at)?);
        }
        self.storage.write(&[StorageOp::PutOutbox(&entry)])?;
        // Extend the cached view instead of rebuilding it: the result is the same.
        if let (Some(events), Some(view)) = (decided, self.view.as_mut()) {
            let view = Arc::make_mut(view);
            for e in &events {
                if let Err(err) = apply(view, e) {
                    tracing::debug!(error = %err, "decided event does not apply to the view");
                    self.view = None;
                    break;
                }
            }
        }
        self.outbox.push(entry);
        Ok(())
    }

    /// The oldest entry the server has not acknowledged yet, with its attempt counted (and stored, so
    /// after a crash the replica knows the server may have applied it).
    pub fn next_to_send(&mut self) -> Result<Option<OutboxEntry>, StorageError> {
        let Some(entry) = self.outbox.iter_mut().find(|e| e.acked.is_none()) else {
            return Ok(None);
        };
        let mut next = entry.clone();
        next.attempts = next.attempts.saturating_add(1);
        self.storage.write(&[StorageOp::PutOutbox(&next)])?;
        *entry = next.clone();
        Ok(Some(next))
    }

    /// Records the server's acknowledgement. The entry leaves the outbox now if it changed nothing or
    /// its events are already confirmed; otherwise when they arrive.
    ///
    /// Returns [`Ack::Diverged`] if the server reports an event at or below the cursor that the
    /// confirmed state does not have: the server's log is not the one this replica followed (it was
    /// restored from a backup and has grown past the old cursor again), so a full resync is needed.
    pub fn acknowledge(
        &mut self,
        command_id: CommandId,
        result: &CommandResult,
    ) -> Result<Ack, StorageError> {
        if result
            .events
            .iter()
            .any(|r| r.seq <= self.cursor && !self.confirmed.journal.has_seen(r.id))
        {
            return Ok(Ack::Diverged);
        }
        let Some(pos) = self.position(command_id) else {
            return Ok(Ack::Recorded);
        };
        let confirmed = result.events.is_empty()
            || result
                .events
                .iter()
                .any(|r| self.confirmed.journal.has_seen(r.id));
        if confirmed {
            self.storage.write(&[StorageOp::RemoveOutbox(command_id)])?;
            self.outbox.remove(pos);
            self.view = None;
        } else {
            let mut next = self.outbox[pos].clone();
            next.acked = Some(result.events.iter().map(|r| r.id).collect());
            self.storage.write(&[StorageOp::PutOutbox(&next)])?;
            self.outbox[pos] = next;
        }
        Ok(Ack::Recorded)
    }

    /// Moves an entry to the "couldn't sync" list.
    pub fn reject(
        &mut self,
        command_id: CommandId,
        rejection: Rejection,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let Some(pos) = self.position(command_id) else {
            return Ok(());
        };
        let failed = FailedEntry {
            entry: self.outbox[pos].clone(),
            failed_at: now,
            rejection,
        };
        self.storage.write(&[
            StorageOp::PutFailed(&failed),
            StorageOp::RemoveOutbox(command_id),
        ])?;
        self.outbox.remove(pos);
        self.failed.push(failed);
        self.view = None;
        Ok(())
    }

    /// Removes an entry from the "couldn't sync" list. Returns false if there was none.
    pub fn dismiss(&mut self, command_id: CommandId) -> Result<bool, StorageError> {
        let Some(pos) = self
            .failed
            .iter()
            .position(|f| f.entry.command_id == command_id)
        else {
            return Ok(false);
        };
        self.storage.write(&[StorageOp::RemoveFailed(command_id)])?;
        self.failed.remove(pos);
        Ok(true)
    }

    fn position(&self, command_id: CommandId) -> Option<usize> {
        self.outbox.iter().position(|e| e.command_id == command_id)
    }

    /// True if `event` is the first event of `entry`'s command: deciding the command against the
    /// confirmed state with the event's actor and time reproduces its ID.
    fn recognises(&self, entry: &OutboxEntry, event: &Event) -> bool {
        if entry.acked.is_some()
            || entry.attempts == 0
            || event.group != event.id
            || event.actor != self.actor
        {
            return false;
        }
        let Some(command) = entry.command() else {
            return false;
        };
        decide(&self.confirmed, &self.actor, command, event.at)
            .ok()
            .and_then(|events| events.first().map(|e| e.id))
            == Some(event.id)
    }

    /// Applies confirmed events in sequence order. Events at or below the cursor are skipped.
    ///
    /// On an error the events before the failing one stay applied. Returns how many were applied.
    pub fn apply_confirmed(&mut self, events: &[Event]) -> Result<usize, ConfirmError> {
        let mut applied: Vec<Event> = Vec::new();
        let mut done: Vec<CommandId> = Vec::new();
        let mut failure = None;
        for e in events {
            let Some(seq) = e.seq else {
                failure = Some(ConfirmError::MissingSeq);
                break;
            };
            if seq <= self.cursor {
                continue;
            }
            if seq != self.cursor + 1 {
                failure = Some(ConfirmError::Gap {
                    expected: self.cursor + 1,
                    got: seq,
                });
                break;
            }
            let recognised: Vec<CommandId> = self
                .outbox
                .iter()
                .filter(|entry| !done.contains(&entry.command_id) && self.recognises(entry, e))
                .map(|entry| entry.command_id)
                .collect();
            if let Err(err) = apply(&mut self.confirmed, e) {
                failure = Some(ConfirmError::Diverged(err));
                break;
            }
            self.cursor = seq;
            done.extend(recognised);
            done.extend(
                self.outbox
                    .iter()
                    .filter(|entry| entry.acked.as_ref().is_some_and(|ids| ids.contains(&e.id)))
                    .map(|entry| entry.command_id),
            );
            applied.push(e.clone());
        }
        if applied.is_empty() {
            return failure.map_or(Ok(0), Err);
        }
        self.view = None;
        self.outbox
            .retain(|entry| !done.contains(&entry.command_id));
        self.unsnapshotted += applied.len();
        let snapshot = self.needs_snapshot || self.unsnapshotted >= self.snapshot_every;
        let mut ops: Vec<StorageOp<'_>> = Vec::with_capacity(done.len() + 1);
        if snapshot {
            ops.push(StorageOp::Snapshot {
                state: &self.confirmed,
                cursor: self.cursor,
            });
        } else {
            ops.push(StorageOp::AppendEvents(&applied));
        }
        ops.extend(done.iter().map(|id| StorageOp::RemoveOutbox(*id)));
        match self.storage.write(&ops) {
            Ok(()) => {
                if snapshot {
                    self.unsnapshotted = 0;
                    self.needs_snapshot = false;
                }
            }
            Err(err) => {
                self.needs_snapshot = true;
                return Err(err.into());
            }
        }
        match failure {
            Some(f) => Err(f),
            None => Ok(applied.len()),
        }
    }

    /// Forgets the confirmed state for a full resync from sequence 0, keeping the outbox and the
    /// "couldn't sync" list. Acknowledged entries are sent again: the server they were acknowledged by
    /// may no longer have them (a restore from backup), and if it does, the retry is idempotent.
    pub fn reset(&mut self) -> Result<(), StorageError> {
        let reacked: Vec<OutboxEntry> = self
            .outbox
            .iter()
            .filter(|e| e.acked.is_some())
            .map(|e| OutboxEntry {
                acked: None,
                ..e.clone()
            })
            .collect();
        let mut ops = vec![StorageOp::ClearConfirmed];
        ops.extend(reacked.iter().map(StorageOp::PutOutbox));
        self.storage.write(&ops)?;
        for e in &mut self.outbox {
            e.acked = None;
        }
        self.confirmed = State::new();
        self.cursor = 0;
        self.unsnapshotted = 0;
        self.needs_snapshot = false;
        self.view = None;
        Ok(())
    }
}

/// Decides and applies one pending command on top of the view, leaving the view untouched if it no
/// longer applies.
fn overlay(view: &mut State, actor: &Actor, entry: &OutboxEntry, command: &todo_core::Command) {
    match decide(view, actor, command, entry.submitted_at) {
        Ok(events) => {
            // `decide`'s events always apply to the state they were decided against.
            for e in &events {
                if let Err(err) = apply(view, e) {
                    tracing::warn!(command_id = %entry.command_id, error = %err, "decided event does not apply");
                    return;
                }
            }
        }
        Err(err) => {
            tracing::debug!(command_id = %entry.command_id, error = %err, "pending command no longer applies");
        }
    }
}
