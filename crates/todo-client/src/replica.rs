//! The local replica: confirmed state, event cursor, log epoch, outbox and the optimistic view. No I/O
//! besides the [`Storage`] it owns, so every rule here is unit-testable without a network.
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
//! Every server event carries the ID of the command that produced it. An entry leaves the outbox in
//! the same storage write that confirms the first event carrying its command ID: from then on the
//! confirmed state includes its effect, and deciding it again on top would apply it twice. An
//! acknowledgement (the 200 response) only marks the entry as not to be sent again, except for a
//! command that changed nothing, which leaves at once. Nothing else confirms an entry: an identical
//! command from another session has another command ID.
//!
//! # Storage failures
//!
//! Every change is written to storage before it is made in memory, so memory is never ahead of disk.
//! Confirming events applies them to a copy of the confirmed state, writes them (with the outbox
//! entries they confirm) in one batch, and swaps the copy in only once that write succeeded; a failed
//! write changes nothing.
//!
//! # Several tabs
//!
//! In the browser, every tab has a replica over the same database, and one of them (elected with the
//! Web Locks API) sends for all. It takes the other tabs' entries in with [`Replica::adopt`], which
//! slots them in by submission time behind anything already sent, and skips what it already has,
//! refused, or saw confirmed (a bounded memory of recently settled command IDs). Entries another tab
//! refused come in with [`Replica::adopt_failed`].
//!
//! # The log epoch
//!
//! The confirmed state belongs to one server log, named by its epoch (see
//! [`crate::protocol::EventPage::epoch`]). Events from another epoch are refused with
//! [`ConfirmError::EpochChanged`]; the client then resyncs from 0, keeping the outbox.
//!
//! Acknowledgements are bound to an epoch too (the server's command response names it). One from
//! another epoch than the replica's is not recorded ([`Ack::OtherEpoch`]), and recorded ones are
//! forgotten when the replica resyncs onto another log, so the entry is sent again; the command ID
//! makes that safe (a server that has the command replays its result, one that lost it applies it).

use crate::outbox::{CommandId, Epoch, FailedEntry, OutboxEntry, Rejection};
use crate::protocol::{CommandResult, LogEvent};
use crate::storage::{Loaded, Storage, StorageError, StorageOp};
use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use todo_core::error::{ApplyError, DomainError};
use todo_core::time::Timestamp;
use todo_core::{Actor, State, apply, decide};

/// How many recently settled command IDs a replica remembers, so it does not adopt an entry another
/// tab still listed after this one saw it confirmed.
const SETTLED_MEMORY: usize = 4096;

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
    #[error("confirmed state diverged from the server at event {seq}: {error}")]
    Diverged {
        /// The event's sequence number.
        seq: u64,
        /// Why it does not apply.
        error: ApplyError,
    },
    /// The events belong to another log than the confirmed state (it was restored from a backup):
    /// resync from 0. Nothing was applied.
    #[error("the server's event log was replaced")]
    EpochChanged,
    /// Storing failed. Nothing was applied.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// What [`Replica::acknowledge`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    /// Recorded (or the entry was already gone).
    Recorded,
    /// The server reports an event at or below the cursor for a command that is still pending, so
    /// the confirmed state has another event there: the server's log is not the one this replica
    /// followed. Resync, then send the entry again. Nothing was recorded.
    Diverged,
    /// The acknowledgement is from another log epoch than the replica's (a response that arrived
    /// after a resync, or one from a log the replica has yet to switch to). Nothing was recorded:
    /// send the entry again later.
    OtherEpoch,
}

/// Confirmed state, cursor, epoch, outbox and "couldn't sync" list, persisted in a [`Storage`].
pub struct Replica {
    actor: Actor,
    storage: Box<dyn Storage>,
    confirmed: State,
    cursor: u64,
    epoch: Option<Epoch>,
    outbox: Vec<OutboxEntry>,
    failed: Vec<FailedEntry>,
    /// The cached view; `None` when it must be rebuilt.
    view: Option<Arc<State>>,
    /// Confirmed events stored since the last snapshot.
    unsnapshotted: usize,
    snapshot_every: usize,
    /// The stored events could not all be replayed: the next write stores a full snapshot (which
    /// drops them).
    needs_snapshot: bool,
    /// Command IDs recently confirmed, refused, or acknowledged as changing nothing, oldest first.
    settled: VecDeque<CommandId>,
    settled_set: BTreeSet<CommandId>,
}

/// What [`Replica::open`] rebuilt from storage.
struct Restored {
    confirmed: State,
    cursor: u64,
    epoch: Option<Epoch>,
    outbox: Vec<OutboxEntry>,
    failed: Vec<FailedEntry>,
    replayed: usize,
    needs_snapshot: bool,
    /// Outbox entries whose events the confirmed state already has.
    settled: Vec<CommandId>,
    /// Outbox entries whose acknowledgement belongs to another epoch, now unacknowledged.
    unbound: Vec<OutboxEntry>,
}

/// True if `entry`'s acknowledgement does not belong to the log with `epoch`.
fn acked_elsewhere(entry: &OutboxEntry, epoch: Epoch) -> bool {
    entry.acked.is_some() && entry.acked_epoch != Some(epoch)
}

/// Replays the stored events onto the snapshot and finds the outbox entries they settle.
fn restore(loaded: Loaded) -> Restored {
    let (mut confirmed, mut cursor) = loaded.snapshot.unwrap_or_default();
    let mut replayed = 0;
    let mut needs_snapshot = false;
    let mut seen_commands = BTreeSet::new();
    for e in &loaded.events {
        let Some(seq) = e.event.seq else { continue };
        if seq <= cursor {
            continue;
        }
        if seq != cursor + 1 {
            tracing::warn!(cursor, seq, "stored events have a gap; refetching the rest");
            needs_snapshot = true;
            break;
        }
        if let Err(err) = apply(&mut confirmed, &e.event) {
            tracing::warn!(seq, error = %err, "a stored event does not apply; refetching from it");
            needs_snapshot = true;
            break;
        }
        seen_commands.insert(e.command_id);
        cursor = seq;
        replayed += 1;
    }
    // Writes are atomic, so an entry leaves storage with the events that confirm it. Check anyway:
    // an entry whose events are confirmed must not be applied (or sent) again.
    let (settled, mut outbox): (Vec<_>, Vec<_>) = loaded.outbox.into_iter().partition(|e| {
        seen_commands.contains(&e.command_id)
            || e.acked
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| confirmed.journal.has_seen(*id)))
    });
    // An acknowledgement from another log than the stored one is void: send the entry again.
    let mut unbound = Vec::new();
    if let Some(epoch) = loaded.epoch {
        for e in &mut outbox {
            if acked_elsewhere(e, epoch) {
                *e = e.unacknowledged();
                unbound.push(e.clone());
            }
        }
    }
    Restored {
        confirmed,
        cursor,
        epoch: loaded.epoch,
        outbox,
        failed: loaded.failed,
        replayed,
        needs_snapshot,
        settled: settled.into_iter().map(|e| e.command_id).collect(),
        unbound,
    }
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
    ///
    /// Outbox entries whose events are already confirmed (by command ID, or by an acknowledged event
    /// ID) are removed.
    pub fn open(actor: Actor, storage: Box<dyn Storage>) -> Result<Self, OpenError> {
        let mut r = Self {
            actor,
            storage,
            confirmed: State::new(),
            cursor: 0,
            epoch: None,
            outbox: Vec::new(),
            failed: Vec::new(),
            view: None,
            unsnapshotted: 0,
            snapshot_every: SNAPSHOT_EVERY,
            needs_snapshot: false,
            settled: VecDeque::new(),
            settled_set: BTreeSet::new(),
        };
        r.load()?;
        Ok(r)
    }

    /// Builds everything from storage.
    fn load(&mut self) -> Result<(), StorageError> {
        let r = restore(self.storage.load()?);
        if !r.settled.is_empty() || !r.unbound.is_empty() {
            tracing::info!(
                settled = r.settled.len(),
                unbound = r.unbound.len(),
                "reconciling the outbox with the confirmed state"
            );
            let mut ops: Vec<StorageOp<'_>> = r
                .settled
                .iter()
                .map(|id| StorageOp::RemoveOutbox(*id))
                .collect();
            ops.extend(r.unbound.iter().map(StorageOp::PutOutbox));
            self.storage.write(&ops)?;
        }
        tracing::debug!(
            cursor = r.cursor,
            replayed = r.replayed,
            outbox = r.outbox.len(),
            "replica loaded"
        );
        self.confirmed = r.confirmed;
        self.cursor = r.cursor;
        self.epoch = r.epoch;
        self.outbox = r.outbox;
        self.failed = r.failed;
        self.view = None;
        self.unsnapshotted = r.replayed;
        self.needs_snapshot = r.needs_snapshot;
        self.remember_settled(r.settled);
        Ok(())
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

    /// The log epoch the confirmed state belongs to (`None` before the first page or stream).
    #[must_use]
    pub fn epoch(&self) -> Option<Epoch> {
        self.epoch
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
    ///
    /// `None` also while that entry is not durable yet ([`Storage::is_durable`]): it is sent once
    /// storage has it, and nothing after it is sent before it.
    pub fn next_to_send(&mut self) -> Result<Option<OutboxEntry>, StorageError> {
        let Some(entry) = self.outbox.iter_mut().find(|e| e.acked.is_none()) else {
            return Ok(None);
        };
        if !self.storage.is_durable(entry.command_id) {
            return Ok(None);
        }
        let mut next = entry.clone();
        next.attempts = next.attempts.saturating_add(1);
        self.storage.write(&[StorageOp::PutOutbox(&next)])?;
        *entry = next.clone();
        Ok(Some(next))
    }

    /// Records the server's acknowledgement: the entry is not sent again, and leaves the outbox when
    /// its events are confirmed (at once if it changed nothing).
    ///
    /// Returns [`Ack::OtherEpoch`] without recording anything if the acknowledgement names another log
    /// epoch than the replica's. While the replica has no epoch yet, it is recorded with its epoch and
    /// forgotten if the replica then adopts another one.
    ///
    /// Returns [`Ack::Diverged`] if the entry is still pending but the server reports one of its
    /// events at or below the cursor: had the confirmed state that event, it would have confirmed the
    /// entry, so the server's log is not the one this replica followed (it was restored from a backup
    /// and grew past the old cursor again).
    pub fn acknowledge(
        &mut self,
        command_id: CommandId,
        result: &CommandResult,
    ) -> Result<Ack, StorageError> {
        let Some(pos) = self.position(command_id) else {
            return Ok(Ack::Recorded);
        };
        if let (Some(theirs), Some(ours)) = (result.epoch, self.epoch)
            && theirs != ours
        {
            return Ok(Ack::OtherEpoch);
        }
        if result.events.iter().any(|r| r.seq <= self.cursor) {
            return Ok(Ack::Diverged);
        }
        if result.events.is_empty() {
            self.storage.write(&[StorageOp::RemoveOutbox(command_id)])?;
            self.outbox.remove(pos);
            self.view = None;
            self.remember_settled([command_id]);
        } else {
            let mut next = self.outbox[pos].clone();
            next.acked = Some(result.events.iter().map(|r| r.id).collect());
            next.acked_epoch = result.epoch;
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
        self.remember_settled([command_id]);
        Ok(())
    }

    fn remember_settled(&mut self, ids: impl IntoIterator<Item = CommandId>) {
        for id in ids {
            if self.settled_set.insert(id) {
                self.settled.push_back(id);
            }
        }
        while self.settled.len() > SETTLED_MEMORY {
            if let Some(old) = self.settled.pop_front() {
                self.settled_set.remove(&old);
            }
        }
    }

    /// True if this replica knows `entry` is done: it is in the "couldn't sync" list, was settled
    /// recently, or its acknowledged events are confirmed.
    fn knows_done(&self, entry: &OutboxEntry) -> bool {
        self.settled_set.contains(&entry.command_id)
            || self
                .failed
                .iter()
                .any(|f| f.entry.command_id == entry.command_id)
            || entry
                .acked
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| self.confirmed.journal.has_seen(*id)))
    }

    /// Takes in outbox entries another tab queued (see the module docs), in submission order, never
    /// ahead of an entry this replica already sent or is sending. Entries it already has or knows to
    /// be done are skipped. Returns how many were added.
    ///
    /// Adopted commands are not decided again: the tab that submitted them did.
    pub fn adopt(&mut self, entries: Vec<OutboxEntry>) -> Result<usize, StorageError> {
        let mut new: Vec<OutboxEntry> = Vec::new();
        for e in entries {
            if self.position(e.command_id).is_none()
                && !self.knows_done(&e)
                && !new.iter().any(|n| n.command_id == e.command_id)
            {
                new.push(e);
            }
        }
        if new.is_empty() {
            return Ok(0);
        }
        let ops: Vec<StorageOp<'_>> = new.iter().map(StorageOp::PutOutbox).collect();
        self.storage.write(&ops)?;
        let added = new.len();
        // Everything up to the last entry already sent (or acknowledged) stays where it is.
        let started = self
            .outbox
            .iter()
            .rposition(|e| e.attempts > 0 || e.acked.is_some())
            .map_or(0, |i| i + 1);
        let key = |e: &OutboxEntry| (e.submitted_at.as_millisecond(), e.command_id);
        for e in new {
            let at = started
                + self.outbox[started..]
                    .iter()
                    .position(|o| key(o) > key(&e))
                    .unwrap_or(self.outbox.len() - started);
            self.outbox.insert(at, e);
        }
        self.view = None;
        Ok(added)
    }

    /// Takes in entries another tab moved to the "couldn't sync" list: they leave this replica's
    /// outbox and join its list. Returns how many were new.
    pub fn adopt_failed(&mut self, failed: Vec<FailedEntry>) -> Result<usize, StorageError> {
        let new: Vec<FailedEntry> = failed
            .into_iter()
            .filter(|f| {
                !self
                    .failed
                    .iter()
                    .any(|o| o.entry.command_id == f.entry.command_id)
            })
            .collect();
        if new.is_empty() {
            return Ok(0);
        }
        let mut ops: Vec<StorageOp<'_>> = Vec::with_capacity(new.len() * 2);
        for f in &new {
            ops.push(StorageOp::PutFailed(f));
            if self.position(f.entry.command_id).is_some() {
                ops.push(StorageOp::RemoveOutbox(f.entry.command_id));
            }
        }
        self.storage.write(&ops)?;
        let ids: Vec<CommandId> = new.iter().map(|f| f.entry.command_id).collect();
        self.outbox.retain(|e| !ids.contains(&e.command_id));
        self.failed.extend(new);
        self.failed
            .sort_by_key(|f| (f.failed_at.as_millisecond(), f.entry.command_id));
        self.view = None;
        self.remember_settled(ids.iter().copied());
        Ok(ids.len())
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

    /// Applies confirmed events from the log with `epoch`, in sequence order, and removes the outbox
    /// entries they confirm, in one storage write. Events at or below the cursor are skipped. An empty
    /// slice only checks (or records) the epoch.
    ///
    /// The first epoch is adopted while the replica is empty; another one is refused with
    /// [`ConfirmError::EpochChanged`]. Adopting an epoch forgets acknowledgements from other ones. On
    /// a gap or a divergence the events before it stay applied (and stored). The events are applied to
    /// a copy of the confirmed state that replaces it only after the write succeeded, so if the write
    /// fails, nothing changes.
    pub fn apply_confirmed(
        &mut self,
        epoch: Epoch,
        events: &[LogEvent],
    ) -> Result<usize, ConfirmError> {
        let adopt = match self.epoch {
            Some(e) if e == epoch => false,
            None if self.cursor == 0 => true,
            _ => return Err(ConfirmError::EpochChanged),
        };
        let mut applied: Vec<LogEvent> = Vec::new();
        let mut failure = None;
        let mut cursor = self.cursor;
        let mut next: Option<State> = None;
        for e in events {
            let Some(seq) = e.event.seq else {
                failure = Some(ConfirmError::MissingSeq);
                break;
            };
            if seq <= cursor {
                continue;
            }
            if seq != cursor + 1 {
                failure = Some(ConfirmError::Gap {
                    expected: cursor + 1,
                    got: seq,
                });
                break;
            }
            // `apply` leaves the state untouched when it fails.
            let state = next.get_or_insert_with(|| self.confirmed.clone());
            if let Err(error) = apply(state, &e.event) {
                failure = Some(ConfirmError::Diverged { seq, error });
                break;
            }
            cursor = seq;
            applied.push(e.clone());
        }
        if applied.is_empty() && !adopt {
            return failure.map_or(Ok(0), Err);
        }
        let unbound: Vec<OutboxEntry> = if adopt {
            self.outbox
                .iter()
                .filter(|e| acked_elsewhere(e, epoch))
                .map(OutboxEntry::unacknowledged)
                .collect()
        } else {
            Vec::new()
        };
        let done: BTreeSet<CommandId> = applied.iter().map(|e| e.command_id).collect();
        let settled: Vec<CommandId> = self
            .outbox
            .iter()
            .map(|e| e.command_id)
            .filter(|id| done.contains(id))
            .collect();
        let snapshot = !applied.is_empty()
            && (self.needs_snapshot || self.unsnapshotted + applied.len() >= self.snapshot_every);
        let mut ops: Vec<StorageOp<'_>> = Vec::with_capacity(settled.len() + unbound.len() + 2);
        if adopt {
            ops.push(StorageOp::SetEpoch(epoch));
            ops.extend(unbound.iter().map(StorageOp::PutOutbox));
        }
        if snapshot {
            ops.push(StorageOp::Snapshot {
                state: next.as_ref().unwrap_or(&self.confirmed),
                cursor,
            });
        } else if !applied.is_empty() {
            ops.push(StorageOp::AppendEvents(&applied));
        }
        ops.extend(settled.iter().map(|id| StorageOp::RemoveOutbox(*id)));
        if let Err(err) = self.storage.write(&ops) {
            tracing::warn!(error = %err, cursor = self.cursor, "cannot store confirmed events");
            return Err(err.into());
        }
        if adopt {
            self.epoch = Some(epoch);
            for u in unbound {
                if let Some(e) = self
                    .outbox
                    .iter_mut()
                    .find(|e| e.command_id == u.command_id)
                {
                    *e = u;
                }
            }
        }
        if let Some(state) = next {
            self.confirmed = state;
            self.cursor = cursor;
        }
        if snapshot {
            self.unsnapshotted = 0;
            self.needs_snapshot = false;
        } else {
            self.unsnapshotted += applied.len();
        }
        if !applied.is_empty() {
            self.view = None;
            self.outbox.retain(|e| !done.contains(&e.command_id));
            self.remember_settled(done.iter().copied());
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
            .map(OutboxEntry::unacknowledged)
            .collect();
        let mut ops = vec![StorageOp::ClearConfirmed];
        ops.extend(reacked.iter().map(StorageOp::PutOutbox));
        self.storage.write(&ops)?;
        for e in &mut self.outbox {
            *e = e.unacknowledged();
        }
        self.confirmed = State::new();
        self.cursor = 0;
        self.epoch = None;
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
