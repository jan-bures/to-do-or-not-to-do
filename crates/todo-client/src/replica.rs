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
//! the view; the server decides its fate.
//!
//! A pending undo is projected from its original's events: the batch the pending original decides
//! to, and once the original is confirmed, its whole confirmed batch. Never from part of a batch:
//! undoing a part can succeed where undoing the whole conflicts (a later command touched what the rest
//! of the batch changed), so the view would show an undo the server refuses. The replica collects
//! those events for every tracked command and for the original of every early undo. When it opens, it
//! takes them from the stored events for every undo in the outbox that names a stored first event. Any
//! other undo (its original's events went into a snapshot, or it came from the activity feed in this
//! session) shows once the server confirms it.
//!
//! # Whole batches
//!
//! The confirmed state only ever holds whole batches: all the events of a command or none of them.
//! A page or a stream can end inside a batch; every event says how many events its command produced
//! ([`LogEvent::command_len`]), and the events of a batch that isn't complete yet are held in memory,
//! not applied, stored or counted in the cursor, until the rest arrives. So the view never shows half
//! a command, a half-applied undo never settles its outbox entry, and a restart in between simply
//! fetches the batch again. [`Replica::received`] is where fetching continues. An event without a
//! length (an older server) counts as ending its batch.
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
//! An undo queued before its command's outcome is known waits in the outbox. The acknowledgement
//! or the event stream binds it to the authoritative event and epoch in the same storage write as
//! it records the original command's outcome. A refusal or no-op settles the dependent undo too.
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
//! slots them in by submission time behind anything *this replica* already sent (entries another tab
//! tried keep no place of their own: a tab that just became the sender merges everything, its own
//! queued commands included, by submission time), and skips what it already has, refused, or saw
//! confirmed recently. Entries another tab refused come in with [`Replica::adopt_failed`].
//!
//! The memory of recently settled command IDs is bounded, so an entry another tab still listed could
//! in principle be adopted after its ID aged out. That is harmless: it is sent again, the server
//! replays the command's result, and [`Replica::acknowledge`] finds its events already confirmed (the
//! confirmed state remembers every event ID) and lets the entry go, without a resync.
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

use crate::outbox::{CommandId, Epoch, FailedEntry, Operation, OutboxEntry, Rejection};
use crate::protocol::{CommandResult, EventRef, LogEvent};
use crate::storage::{Loaded, Storage, StorageError, StorageOp};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Weak};
use todo_core::error::{ApplyError, DomainError};
use todo_core::event::Event;
use todo_core::ids::EventId;
use todo_core::time::Timestamp;
use todo_core::undo::undo_batch;
use todo_core::{Actor, State, UndoOptions, apply, decide};

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

/// An undo target for a command submitted by this client. Keep it for as long as the action can be
/// undone; it retains the command's correlated events even if a snapshot drops them from storage.
#[derive(Clone, Debug)]
pub struct TrackedCommand(Arc<CommandId>);

/// The confirmed events of one command, from its first event on.
#[derive(Debug)]
struct CommandLog {
    handle: Weak<CommandId>,
    first: Option<(Epoch, EventRef)>,
    events: Vec<Event>,
    /// How many events the command produced, if the server said ([`LogEvent::command_len`]).
    len: Option<usize>,
}

impl CommandLog {
    /// The whole batch, once the confirmed state has all of it (`cursor`).
    fn batch(&self, cursor: u64) -> Option<&[Event]> {
        let last = self.events.last()?.seq?;
        let complete = match self.len {
            Some(len) => self.events.len() == len && last <= cursor,
            // Without a length, only a later confirmed event proves the batch ended (a command's
            // events are consecutive in the log).
            None => last < cursor,
        };
        complete.then_some(self.events.as_slice())
    }
}

/// The stored events of a command an outbox undo names, from its first event.
#[derive(Clone, Debug)]
struct StoredLog {
    first: EventRef,
    events: Vec<Event>,
    len: Option<usize>,
}

fn batch_len(event: &LogEvent) -> Option<usize> {
    event.command_len.map(|len| len as usize)
}

/// What happened when trying to undo a tracked command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackedUndo {
    /// The command was durably dropped before its first send attempt.
    Cancelled,
    /// The undo is durably queued under this new command ID.
    Queued(CommandId),
    /// The command had no effect, failed, or lost its history during a resync.
    Unavailable,
}

fn checked_undo(epoch: Epoch, event: EventRef) -> Operation {
    Operation::Undo {
        seq: event.seq,
        expected_event_id: event.id,
        epoch: Some(epoch),
        force: false,
    }
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
    /// Entries this replica has sent (or is sending): [`Replica::adopt`] slots nothing before them.
    sent_here: BTreeSet<CommandId>,
    tracked: BTreeMap<CommandId, CommandLog>,
    /// A queued undo needs the original's log even if its caller drops the tracking handle (or never
    /// had one: the undo was restored from storage or adopted from another tab).
    undo_targets: BTreeMap<CommandId, TrackedCommand>,
    /// Received events after the cursor that start a batch whose rest hasn't arrived yet (see the
    /// module docs), consecutive from `cursor + 1`. Memory only.
    held: Vec<LogEvent>,
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
    resolved_undos: Vec<OutboxEntry>,
    failed_undos: Vec<FailedEntry>,
    /// The replayed events of the commands an outbox undo names, by command, from the first event.
    undo_logs: BTreeMap<CommandId, StoredLog>,
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
    let mut first_events = BTreeMap::new();
    // The events an outbox undo needs to show in the view: those of the command whose first event it
    // names, and those of an early undo's original.
    let mut undone_events = BTreeSet::new();
    let mut undone_commands = BTreeSet::new();
    for entry in &loaded.outbox {
        match entry.op {
            Operation::Undo {
                seq,
                expected_event_id,
                epoch: Some(epoch),
                force: false,
            } if loaded.epoch == Some(epoch) => {
                undone_events.insert((seq, expected_event_id));
            }
            Operation::UndoAfter { original } => {
                undone_commands.insert(original);
            }
            _ => {}
        }
    }
    let mut undo_logs: BTreeMap<CommandId, StoredLog> = BTreeMap::new();
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
        if e.command_idx == 0 {
            let first = EventRef {
                seq,
                id: e.event.id,
            };
            first_events.insert(e.command_id, first);
            if undone_events.contains(&(seq, e.event.id)) || undone_commands.contains(&e.command_id)
            {
                undo_logs.insert(
                    e.command_id,
                    StoredLog {
                        first,
                        events: vec![e.event.clone()],
                        len: batch_len(e),
                    },
                );
            }
        } else if let Some(log) = undo_logs.get_mut(&e.command_id)
            && log.events.len() == e.command_idx as usize
        {
            log.events.push(e.event.clone());
        }
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
    let pending_ids: BTreeSet<_> = outbox.iter().map(|entry| entry.command_id).collect();
    let mut resolved_undos = Vec::new();
    let mut failed_undos = Vec::new();
    outbox.retain_mut(|entry| {
        let Operation::UndoAfter { original } = entry.op else {
            return true;
        };
        if pending_ids.contains(&original) {
            return true;
        }
        if let Some((epoch, event)) = loaded.epoch.zip(first_events.get(&original)) {
            *entry = resolved_undo(entry, epoch, *event);
            resolved_undos.push(entry.clone());
            true
        } else {
            // A snapshot can prove an acknowledged command settled without retaining its seq.
            failed_undos.push(undo_without_event(entry));
            false
        }
    });
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
        resolved_undos,
        failed_undos,
        undo_logs,
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
            sent_here: BTreeSet::new(),
            tracked: BTreeMap::new(),
            undo_targets: BTreeMap::new(),
            held: Vec::new(),
        };
        r.load()?;
        Ok(r)
    }

    /// Builds everything from storage.
    fn load(&mut self) -> Result<(), StorageError> {
        let r = restore(self.storage.load()?);
        let failed_ids: Vec<_> = r.failed_undos.iter().map(|f| f.entry.command_id).collect();
        if !r.settled.is_empty()
            || !r.unbound.is_empty()
            || !r.resolved_undos.is_empty()
            || !failed_ids.is_empty()
        {
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
            ops.extend(
                r.unbound
                    .iter()
                    .chain(&r.resolved_undos)
                    .map(StorageOp::PutOutbox),
            );
            ops.extend(r.failed_undos.iter().map(StorageOp::PutFailed));
            ops.extend(failed_ids.iter().copied().map(StorageOp::RemoveOutbox));
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
        self.failed.extend(r.failed_undos);
        self.view = None;
        self.unsnapshotted = r.replayed;
        self.needs_snapshot = r.needs_snapshot;
        self.remember_settled(r.settled.into_iter().chain(failed_ids));
        self.hold_restored_logs(&r.undo_logs);
        Ok(())
    }

    /// Keeps the original's events for every undo in the restored outbox, so it shows in the view as
    /// it did before the restart.
    fn hold_restored_logs(&mut self, logs: &BTreeMap<CommandId, StoredLog>) {
        let mut holds = Vec::new();
        for entry in &self.outbox {
            match entry.op {
                Operation::UndoAfter { original } => holds.push((entry.command_id, original, None)),
                Operation::Undo {
                    seq,
                    expected_event_id,
                    epoch: Some(epoch),
                    force: false,
                } if self.epoch == Some(epoch) => {
                    let event = EventRef {
                        seq,
                        id: expected_event_id,
                    };
                    if seq <= self.cursor {
                        if let Some((original, log)) =
                            logs.iter().find(|(_, log)| log.first == event)
                        {
                            holds.push((entry.command_id, *original, Some((epoch, log.clone()))));
                        }
                    } else if let Some(original) = self.acked_original(epoch, expected_event_id) {
                        // Acknowledged, not confirmed yet: its events are still to come.
                        holds.push((entry.command_id, original.command_id, None));
                    }
                }
                _ => {}
            }
        }
        for (undo, original, log) in holds {
            self.hold_log(undo, original, log);
        }
    }

    /// Keeps `original`'s log, collecting its confirmed events, for as long as `undo` is in the outbox.
    fn hold_log(&mut self, undo: CommandId, original: CommandId, log: Option<(Epoch, StoredLog)>) {
        let handle = match self.tracked.get(&original).and_then(|l| l.handle.upgrade()) {
            Some(handle) => handle,
            None => {
                let handle = Arc::new(original);
                let (first, events, len) = log.map_or((None, Vec::new(), None), |(epoch, log)| {
                    (Some((epoch, log.first)), log.events, log.len)
                });
                self.tracked.insert(
                    original,
                    CommandLog {
                        handle: Arc::downgrade(&handle),
                        first,
                        events,
                        len,
                    },
                );
                handle
            }
        };
        self.undo_targets.insert(undo, TrackedCommand(handle));
    }

    /// The pending entry the server acknowledged with `event` as its first event.
    fn acked_original(&self, epoch: Epoch, event: EventId) -> Option<&OutboxEntry> {
        self.outbox.iter().find(|entry| {
            entry.acked_epoch == Some(epoch)
                && entry.acked.as_ref().and_then(|ids| ids.first()) == Some(&event)
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

    /// The last sequence number received: the cursor, or the last event held from a batch whose rest
    /// hasn't arrived yet (see the module docs). Fetching continues after it.
    #[must_use]
    pub fn received(&self) -> u64 {
        self.held
            .last()
            .and_then(|e| e.event.seq)
            .unwrap_or(self.cursor)
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
        let keep_batches = self.outbox.iter().any(|e| e.command().is_none());
        let mut view = self.confirmed.clone();
        let mut batches = BTreeMap::new();
        for entry in &self.outbox {
            if let Some(command) = entry.command() {
                if let Some(events) = overlay(&mut view, &self.actor, entry, command)
                    && keep_batches
                {
                    batches.insert(entry.command_id, events);
                }
            } else if let Some(events) = self.undo_events(&entry.op, &batches) {
                self.project_undo(&mut view, events, entry.submitted_at);
            }
        }
        view
    }

    fn undo_events<'a>(
        &'a self,
        op: &Operation,
        batches: &'a BTreeMap<CommandId, Vec<Event>>,
    ) -> Option<&'a [Event]> {
        match *op {
            Operation::UndoAfter { original } => batches.get(&original).map(Vec::as_slice),
            Operation::Undo {
                seq,
                expected_event_id,
                epoch: Some(epoch),
                force: false,
            } if self.epoch == Some(epoch) => {
                if seq > self.cursor {
                    // A response can resolve the undo before its original appears in the stream.
                    let original = self.acked_original(epoch, expected_event_id)?;
                    return batches.get(&original.command_id).map(Vec::as_slice);
                }
                let event = EventRef {
                    seq,
                    id: expected_event_id,
                };
                // Only the whole batch, and only while the confirmed state has it (not after a
                // resync, for example).
                self.tracked
                    .values()
                    .find(|log| log.first == Some((epoch, event)))?
                    .batch(self.cursor)
            }
            _ => None,
        }
    }

    fn project_undo(&self, view: &mut State, events: &[Event], at: Timestamp) -> bool {
        if events.is_empty() {
            return false;
        }
        let Ok(inverse) = undo_batch(view, events, &self.actor, at, UndoOptions::default()) else {
            return false;
        };
        // undo_batch validates the full compensation against a copy before returning it.
        inverse.iter().all(|event| apply(view, event).is_ok())
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
        if let Operation::UndoAfter { original } = entry.op {
            self.hold_log(entry.command_id, original, None);
        }
        // Commands can extend the cached view; undos need the original's batch.
        if entry.command().is_none() {
            self.view = None;
        } else if let (Some(events), Some(view)) = (decided, self.view.as_mut()) {
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

    pub(crate) fn track(&mut self, id: CommandId) -> TrackedCommand {
        self.prune_tracking();
        let id = Arc::new(id);
        self.tracked.insert(
            *id,
            CommandLog {
                handle: Arc::downgrade(&id),
                first: None,
                events: Vec::new(),
                len: None,
            },
        );
        TrackedCommand(id)
    }

    fn prune_tracking(&mut self) {
        self.undo_targets
            .retain(|id, _| self.outbox.iter().any(|e| e.command_id == *id));
        self.tracked.retain(|_, log| log.handle.strong_count() > 0);
    }

    fn tracked(&self, target: &TrackedCommand) -> Option<&CommandLog> {
        self.tracked
            .get(&target.0)
            .filter(|log| log.handle.ptr_eq(&Arc::downgrade(&target.0)))
    }

    /// Runs under the same lock as the send loop, which persists its first attempt before sending.
    pub(crate) fn undo_tracked(
        &mut self,
        target: &TrackedCommand,
        make_entry: impl FnOnce(Operation) -> OutboxEntry,
    ) -> Result<TrackedUndo, SubmitError> {
        let Some(log) = self.tracked(target) else {
            return Ok(TrackedUndo::Unavailable);
        };
        let op = if let Some((epoch, event)) = log.first {
            if self.epoch.is_some_and(|current| current != epoch) {
                return Ok(TrackedUndo::Unavailable);
            }
            checked_undo(epoch, event)
        } else {
            let original = *target.0;
            let Some(pos) = self.position(original) else {
                return Ok(TrackedUndo::Unavailable);
            };
            // Don't drop a pending action a later local command might depend on. In that case take
            // the normal core/server conflict path, ordered after the already queued commands.
            if pos + 1 == self.outbox.len()
                && self.outbox[pos].attempts == 0
                && self.outbox[pos].acked.is_none()
            {
                self.storage.write(&[StorageOp::RemoveOutbox(original)])?;
                self.outbox.remove(pos);
                self.remember_settled([original]);
                self.view = None;
                return Ok(TrackedUndo::Cancelled);
            }
            Operation::UndoAfter { original }
        };
        let entry = make_entry(op);
        let id = entry.command_id;
        self.submit(entry)?;
        self.undo_targets.insert(id, target.clone());
        Ok(TrackedUndo::Queued(id))
    }

    /// The oldest entry the server has not acknowledged yet, with its attempt counted (and stored, so
    /// after a crash the replica knows the server may have applied it).
    ///
    /// `None` also while that entry is not durable yet ([`Storage::is_durable`]) or is an early
    /// undo that still needs the original's receipt. Nothing after it is sent before it.
    pub fn next_to_send(&mut self) -> Result<Option<OutboxEntry>, StorageError> {
        loop {
            let Some(pos) = self.outbox.iter().position(|e| e.acked.is_none()) else {
                return Ok(None);
            };
            let entry = &self.outbox[pos];
            if !self.storage.is_durable(entry.command_id) {
                return Ok(None);
            }
            if let Operation::UndoAfter { original } = entry.op {
                if self.position(original).is_some() {
                    return Ok(None);
                }
                self.settle(entry.command_id, vec![undo_without_event(entry)])?;
                continue;
            }
            let mut next = entry.clone();
            next.attempts = next.attempts.saturating_add(1);
            self.storage.write(&[StorageOp::PutOutbox(&next)])?;
            self.outbox[pos] = next.clone();
            self.sent_here.insert(next.command_id);
            return Ok(Some(next));
        }
    }

    /// Records the server's acknowledgement: the entry is not sent again, and leaves the outbox when
    /// its events are confirmed (at once if it changed nothing).
    ///
    /// Returns [`Ack::OtherEpoch`] without recording anything if the acknowledgement names another log
    /// epoch than the replica's. While the replica has no epoch yet, it is recorded with its epoch and
    /// forgotten if the replica then adopts another one.
    ///
    /// If the server reports events at or below the cursor that the confirmed state already has, the
    /// command was confirmed before this replica held the entry (another tab queued it, and it was
    /// adopted after its confirmation): those events need nothing more, and if they are all of them,
    /// the entry leaves the outbox at once.
    ///
    /// Returns [`Ack::Diverged`] if the server reports an event at or below the cursor that the
    /// confirmed state doesn't have: the server's log is not the one this replica followed (it was
    /// restored from a backup and grew past the old cursor again).
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
        let below = result.events.iter().filter(|r| r.seq <= self.cursor);
        if below
            .clone()
            .any(|r| !self.confirmed.journal.has_seen(r.id))
        {
            return Ok(Ack::Diverged);
        }
        let confirmed = below.count();
        if result.events.is_empty() {
            let failed = self
                .dependent_undos(command_id)
                .map(undo_without_event)
                .collect();
            self.settle(command_id, failed)?;
            return Ok(Ack::Recorded);
        }
        let seq = result.events.first().copied();
        // If the epoch is established, the response is enough to resolve the dependency; the
        // stream can also resolve it, including if the response is lost. With no epoch wait for
        // the stream to establish which log we're following.
        let ready: Vec<_> = self
            .epoch
            .filter(|epoch| result.epoch == Some(*epoch) || confirmed == result.events.len())
            .zip(seq)
            .map_or_else(Vec::new, |(epoch, event)| {
                self.dependent_undos(command_id)
                    .map(|entry| resolved_undo(entry, epoch, event))
                    .collect()
            });
        let mut ops: Vec<_> = ready.iter().map(StorageOp::PutOutbox).collect();
        if confirmed == result.events.len() {
            tracing::debug!(%command_id, "the command was confirmed already");
            ops.push(StorageOp::RemoveOutbox(command_id));
            self.storage.write(&ops)?;
            self.outbox.remove(pos);
            self.view = None;
            self.remember_settled([command_id]);
        } else {
            let mut next = self.outbox[pos].clone();
            next.acked = Some(result.events.iter().map(|r| r.id).collect());
            next.acked_epoch = result.epoch;
            ops.push(StorageOp::PutOutbox(&next));
            self.storage.write(&ops)?;
            self.outbox[pos] = next;
        }
        if !ready.is_empty() {
            self.replace_outbox(ready);
            self.view = None;
        }
        Ok(Ack::Recorded)
    }

    fn dependent_undos(&self, id: CommandId) -> impl Iterator<Item = &OutboxEntry> {
        self.outbox.iter().filter(
            move |entry| matches!(entry.op, Operation::UndoAfter { original } if original == id),
        )
    }

    fn replace_outbox(&mut self, entries: Vec<OutboxEntry>) {
        for entry in entries {
            if let Some(pos) = self.position(entry.command_id) {
                self.outbox[pos] = entry;
            }
        }
    }

    fn settle(&mut self, id: CommandId, failed: Vec<FailedEntry>) -> Result<(), StorageError> {
        let mut remove = BTreeSet::from([id]);
        remove.extend(failed.iter().map(|f| f.entry.command_id));
        let ops: Vec<_> = failed
            .iter()
            .map(StorageOp::PutFailed)
            .chain(remove.iter().copied().map(StorageOp::RemoveOutbox))
            .collect();
        self.storage.write(&ops)?;
        self.outbox.retain(|e| !remove.contains(&e.command_id));
        self.failed.extend(failed);
        self.view = None;
        self.remember_settled(remove);
        self.prune_tracking();
        Ok(())
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
        let failures = std::iter::once(failed)
            .chain(self.dependent_undos(command_id).map(undo_without_event))
            .collect();
        self.settle(command_id, failures)
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
        for entry in &new {
            if let Operation::UndoAfter { original } = entry.op {
                self.hold_log(entry.command_id, original, None);
            }
        }
        // Everything up to the last entry this replica sent stays where it is. Entries another tab
        // tried (their attempts count says so) are ordered by submission time like the rest.
        self.sent_here
            .retain(|id| self.outbox.iter().any(|e| e.command_id == *id));
        let started = self
            .outbox
            .iter()
            .rposition(|e| self.sent_here.contains(&e.command_id))
            .map_or(0, |i| i + 1);
        // The rest, this replica's own queued entries and the adopted ones, in submission order.
        self.outbox.extend(new);
        self.outbox[started..].sort_by_key(|e| (e.submitted_at.as_millisecond(), e.command_id));
        self.view = None;
        Ok(added)
    }

    /// Takes in entries another tab moved to the "couldn't sync" list: they leave this replica's
    /// outbox and join its list. Returns how many were new.
    pub fn adopt_failed(&mut self, failed: Vec<FailedEntry>) -> Result<usize, StorageError> {
        let mut ids: BTreeSet<_> = self.failed.iter().map(|f| f.entry.command_id).collect();
        let mut new: Vec<_> = failed
            .into_iter()
            .filter(|f| ids.insert(f.entry.command_id))
            .collect();
        let adopted = new.len();
        if adopted == 0 {
            return Ok(0);
        }
        let originals: BTreeSet<_> = new.iter().map(|f| f.entry.command_id).collect();
        new.extend(self.outbox.iter().filter_map(|entry| match entry.op {
            Operation::UndoAfter { original }
                if originals.contains(&original) && ids.insert(entry.command_id) =>
            {
                Some(undo_without_event(entry))
            }
            _ => None,
        }));
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
        self.remember_settled(ids);
        self.prune_tracking();
        Ok(adopted)
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
    /// entries they confirm, in one storage write. Events at or below [`Replica::received`] are
    /// skipped. An empty slice only checks (or records) the epoch.
    ///
    /// Only whole batches are applied: if the events end inside a batch, its events are held until
    /// the rest arrives (see the module docs). Returns how many events were applied.
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
        // The held start of a batch, then what arrived: consecutive from the cursor.
        let mut received: Vec<&LogEvent> = Vec::new();
        let mut failure = None;
        let mut last = self.cursor;
        for e in self.held.iter().chain(events) {
            let Some(seq) = e.event.seq else {
                failure = Some(ConfirmError::MissingSeq);
                break;
            };
            if seq <= last {
                continue;
            }
            if seq != last + 1 {
                failure = Some(ConfirmError::Gap {
                    expected: last + 1,
                    got: seq,
                });
                break;
            }
            last = seq;
            received.push(e);
        }
        // A batch that goes on past what arrived waits for the rest (a command's events are
        // consecutive, so only the last one can be incomplete).
        let whole = match received.last() {
            Some(e) if e.batch_continues() => received
                .iter()
                .rposition(|other| other.command_id != e.command_id)
                .map_or(0, |i| i + 1),
            _ => received.len(),
        };
        let mut applied: Vec<LogEvent> = Vec::with_capacity(whole);
        let mut cursor = self.cursor;
        let mut next: Option<State> = None;
        for e in &received[..whole] {
            let Some(seq) = e.event.seq else { break };
            // `apply` leaves the state untouched when it fails.
            let state = next.get_or_insert_with(|| self.confirmed.clone());
            if let Err(error) = apply(state, &e.event) {
                failure = Some(ConfirmError::Diverged { seq, error });
                break;
            }
            cursor = seq;
            applied.push((*e).clone());
        }
        // Held events must follow on from the cursor; after a divergence there is nothing to hold.
        let held: Vec<LogEvent> = if applied.len() == whole {
            received[whole..].iter().map(|e| (*e).clone()).collect()
        } else {
            Vec::new()
        };
        if applied.is_empty() && !adopt {
            self.held = held;
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
        // Resolve durable early Undo requests before removing their original from the outbox.
        let mut ready = Vec::new();
        let mut failed = Vec::new();
        if self
            .outbox
            .iter()
            .any(|e| matches!(e.op, Operation::UndoAfter { .. }))
        {
            let first: BTreeMap<_, _> = applied
                .iter()
                .filter(|log| log.command_idx == 0)
                .filter_map(|log| {
                    Some((
                        log.command_id,
                        EventRef {
                            seq: log.event.seq?,
                            id: log.event.id,
                        },
                    ))
                })
                .collect();
            for entry in &self.outbox {
                if let Operation::UndoAfter { original } = entry.op
                    && done.contains(&original)
                {
                    if let Some(event) = first.get(&original) {
                        ready.push(resolved_undo(entry, epoch, *event));
                    } else {
                        // We only saw the tail of the batch, never bind to a later event.
                        failed.push(undo_without_event(entry));
                    }
                }
            }
        }
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
        let failed_ids: BTreeSet<_> = failed.iter().map(|f| f.entry.command_id).collect();
        ops.extend(ready.iter().map(StorageOp::PutOutbox));
        ops.extend(failed.iter().map(StorageOp::PutFailed));
        ops.extend(
            settled
                .iter()
                .chain(&failed_ids)
                .map(|id| StorageOp::RemoveOutbox(*id)),
        );
        if let Err(err) = self.storage.write(&ops) {
            tracing::warn!(error = %err, cursor = self.cursor, "cannot store confirmed events");
            return Err(err.into());
        }
        self.replace_outbox(ready);
        self.held = held;
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
            for event in &applied {
                if let Some(log) = self.tracked.get_mut(&event.command_id) {
                    // Bind to the first log in which this command was seen. On a same-epoch replay,
                    // ignore its duplicate events; on a new epoch, the old target stays unavailable.
                    if let (None, Some(seq), 0) = (log.first, event.event.seq, event.command_idx) {
                        log.first = Some((
                            epoch,
                            EventRef {
                                seq,
                                id: event.event.id,
                            },
                        ));
                    }
                    if log.first.is_some_and(|(origin, _)| origin == epoch)
                        && (event.command_idx as usize) == log.events.len()
                    {
                        log.events.push(event.event.clone());
                        log.len = batch_len(event).or(log.len);
                    }
                }
            }
            self.outbox
                .retain(|e| !done.contains(&e.command_id) && !failed_ids.contains(&e.command_id));
            self.failed.extend(failed);
            self.remember_settled(done.into_iter().chain(failed_ids));
            self.prune_tracking();
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
        self.held.clear();
        self.epoch = None;
        self.unsnapshotted = 0;
        self.needs_snapshot = false;
        self.view = None;
        Ok(())
    }
}

/// Replaces a dependency with a checked undo of the original command's first event.
fn resolved_undo(entry: &OutboxEntry, epoch: Epoch, event: EventRef) -> OutboxEntry {
    OutboxEntry {
        op: checked_undo(epoch, event),
        ..entry.clone()
    }
}

fn undo_without_event(entry: &OutboxEntry) -> FailedEntry {
    FailedEntry {
        entry: entry.clone(),
        failed_at: entry.submitted_at,
        rejection: Rejection {
            status: 0,
            code: "undo_no_event".into(),
            message: "No matching event is available for the original change.".into(),
        },
    }
}

/// Decides and applies one pending command and returns its batch for any dependent undo.
fn overlay(
    view: &mut State,
    actor: &Actor,
    entry: &OutboxEntry,
    command: &todo_core::Command,
) -> Option<Vec<Event>> {
    match decide(view, actor, command, entry.submitted_at) {
        Ok(events) => {
            // `decide`'s events always apply to the state they were decided against.
            for e in &events {
                if let Err(err) = apply(view, e) {
                    tracing::warn!(command_id = %entry.command_id, error = %err, "decided event does not apply");
                    return None;
                }
            }
            Some(events)
        }
        Err(err) => {
            tracing::debug!(command_id = %entry.command_id, error = %err, "pending command no longer applies");
            None
        }
    }
}
