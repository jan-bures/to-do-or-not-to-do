//! Durable client storage.
//!
//! A [`Storage`] keeps five things: confirmed events after the last snapshot (with the command each
//! came from), an optional snapshot of the confirmed state with its cursor, the log epoch they belong
//! to, the outbox in submission order, and the "couldn't sync" list. Every [`write`](Storage::write)
//! is one atomic batch.
//!
//! The trait is synchronous: the engine calls it while holding its state lock and never across an
//! `await`. A backend whose platform API is asynchronous (IndexedDB) keeps an in-memory copy and
//! persists it in the background, accepting that the newest writes may be lost on a crash; nothing is
//! lost for good, because unconfirmed events are fetched again and lost outbox removals only lead to an
//! idempotent resend. Lost outbox *additions* are the exception, so such a backend should flush
//! submissions promptly.

use crate::outbox::{CommandId, Epoch, FailedEntry, OutboxEntry};
use crate::protocol::LogEvent;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use todo_core::state::State;

/// A storage backend failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StorageError {
    /// The backend reported an error.
    #[error("storage backend error: {0}")]
    Backend(String),
    /// Stored data could not be decoded.
    #[error("stored data is corrupt: {0}")]
    Corrupt(String),
}

impl From<serde_json::Error> for StorageError {
    fn from(e: serde_json::Error) -> Self {
        Self::Corrupt(e.to_string())
    }
}

/// One change in a [`Storage::write`] batch.
#[derive(Debug)]
pub enum StorageOp<'a> {
    /// Append confirmed events (in sequence order, after every stored one).
    AppendEvents(&'a [LogEvent]),
    /// Replace the snapshot and drop every stored event (the snapshot is the whole confirmed state;
    /// stored events past its cursor could not be replayed and are fetched again).
    Snapshot {
        /// The confirmed state.
        state: &'a State,
        /// The sequence number it includes events up to.
        cursor: u64,
    },
    /// Forget the confirmed state: the snapshot, all events and the epoch (full resync).
    ClearConfirmed,
    /// Record the log epoch the confirmed state belongs to.
    SetEpoch(Epoch),
    /// Insert an outbox entry at the end, or replace the one with the same command ID in place.
    PutOutbox(&'a OutboxEntry),
    /// Remove an outbox entry (no-op if absent).
    RemoveOutbox(CommandId),
    /// Insert or replace a "couldn't sync" entry.
    PutFailed(&'a FailedEntry),
    /// Remove a "couldn't sync" entry (no-op if absent).
    RemoveFailed(CommandId),
}

/// Everything a [`Storage`] holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    /// The last snapshot and its cursor.
    pub snapshot: Option<(State, u64)>,
    /// Confirmed events after the snapshot, in sequence order.
    pub events: Vec<LogEvent>,
    /// The log epoch the snapshot and events belong to.
    pub epoch: Option<Epoch>,
    /// The outbox, oldest first.
    pub outbox: Vec<OutboxEntry>,
    /// The "couldn't sync" list, oldest first.
    pub failed: Vec<FailedEntry>,
}

/// Durable storage for the sync engine.
pub trait Storage: crate::platform::MaybeSend {
    /// Reads everything.
    fn load(&mut self) -> Result<Loaded, StorageError>;

    /// Applies the operations atomically, in order.
    fn write(&mut self, ops: &[StorageOp<'_>]) -> Result<(), StorageError>;
}

#[derive(Default)]
struct MemoryData {
    snapshot: Option<(String, u64)>,
    events: Vec<(u64, String)>,
    epoch: Option<Epoch>,
    outbox: Vec<(CommandId, String)>,
    failed: Vec<(CommandId, String)>,
    fail_writes: bool,
}

/// In-memory [`Storage`]. Clones share the same data, so a test can "restart" a client on it.
///
/// Values are kept as JSON, exactly as a durable backend would store them, so serialization round
/// trips are exercised too.
#[derive(Clone, Default)]
pub struct MemoryStorage {
    data: Arc<Mutex<MemoryData>>,
}

impl fmt::Debug for MemoryStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.lock();
        f.debug_struct("MemoryStorage")
            .field("events", &d.events.len())
            .field("outbox", &d.outbox.len())
            .field("failed", &d.failed.len())
            .finish_non_exhaustive()
    }
}

impl MemoryStorage {
    /// Empty storage.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryData> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes every following write fail (or succeed again), for tests.
    pub fn set_fail_writes(&self, fail: bool) {
        self.lock().fail_writes = fail;
    }
}

fn put(list: &mut Vec<(CommandId, String)>, id: CommandId, json: String) {
    match list.iter_mut().find(|(k, _)| *k == id) {
        Some(slot) => slot.1 = json,
        None => list.push((id, json)),
    }
}

impl Storage for MemoryStorage {
    fn load(&mut self) -> Result<Loaded, StorageError> {
        let d = self.lock();
        Ok(Loaded {
            snapshot: d
                .snapshot
                .as_ref()
                .map(|(s, c)| serde_json::from_str(s).map(|s| (s, *c)))
                .transpose()?,
            events: d
                .events
                .iter()
                .map(|(_, e)| serde_json::from_str(e))
                .collect::<Result<_, _>>()?,
            epoch: d.epoch,
            outbox: d
                .outbox
                .iter()
                .map(|(_, e)| serde_json::from_str(e))
                .collect::<Result<_, _>>()?,
            failed: d
                .failed
                .iter()
                .map(|(_, e)| serde_json::from_str(e))
                .collect::<Result<_, _>>()?,
        })
    }

    fn write(&mut self, ops: &[StorageOp<'_>]) -> Result<(), StorageError> {
        let mut d = self.lock();
        if d.fail_writes {
            return Err(StorageError::Backend("writes are disabled".into()));
        }
        // Encode everything first so a failure leaves the data untouched (atomicity).
        let mut next = MemoryData {
            snapshot: d.snapshot.clone(),
            events: d.events.clone(),
            epoch: d.epoch,
            outbox: d.outbox.clone(),
            failed: d.failed.clone(),
            fail_writes: false,
        };
        for op in ops {
            match op {
                StorageOp::AppendEvents(events) => {
                    for e in *events {
                        let seq = e
                            .event
                            .seq
                            .ok_or_else(|| StorageError::Backend("event without seq".into()))?;
                        next.events.push((seq, serde_json::to_string(e)?));
                    }
                }
                StorageOp::Snapshot { state, cursor } => {
                    next.snapshot = Some((serde_json::to_string(state)?, *cursor));
                    next.events.clear();
                }
                StorageOp::ClearConfirmed => {
                    next.snapshot = None;
                    next.events.clear();
                    next.epoch = None;
                }
                StorageOp::SetEpoch(e) => next.epoch = Some(*e),
                StorageOp::PutOutbox(e) => {
                    put(&mut next.outbox, e.command_id, serde_json::to_string(e)?);
                }
                StorageOp::RemoveOutbox(id) => next.outbox.retain(|(k, _)| k != id),
                StorageOp::PutFailed(f) => {
                    put(
                        &mut next.failed,
                        f.entry.command_id,
                        serde_json::to_string(f)?,
                    );
                }
                StorageOp::RemoveFailed(id) => next.failed.retain(|(k, _)| k != id),
            }
        }
        *d = next;
        Ok(())
    }
}
