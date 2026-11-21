//! A [`Storage`] for platforms whose durable storage is asynchronous (IndexedDB in the browser).
//!
//! The engine calls [`Storage`] synchronously while it holds its state lock, so such a platform keeps
//! the data in memory and persists it in the background. [`MirrorStorage`] is that memory: it applies
//! every [`write`](Storage::write) at once, and records it as one [`Batch`] of [`Change`]s in a
//! journal. The platform drains the journal ([`MirrorStorage::take_batches`]) and writes each batch
//! atomically, in order (one IndexedDB transaction per batch). On start it reads the persisted
//! [`Persisted`] image back and opens [`MirrorStorage::from_persisted`] on it.
//!
//! The persisted form is a small key-value image plus the events by sequence number:
//!
//! | Key | Value |
//! |---|---|
//! | [`Key::Snapshot`] | `{"cursor": n, "state": <State>}` |
//! | [`Key::Epoch`] | the log epoch |
//! | [`Key::Outbox`] | the outbox entries, oldest first (a JSON array) |
//! | [`Key::Failed`] | the "couldn't sync" entries, oldest first |
//! | events | each [`LogEvent`](crate::protocol::LogEvent) under its `seq` |
//!
//! The outbox and the failed list are small, so they are rewritten whole; events are appended one by
//! one and cleared together. What is lost if the page closes before a batch lands is described on
//! [`crate::storage`]: confirmed events are fetched again, and a lost outbox removal only leads to an
//! idempotent resend. Lost outbox *additions* are the exception, so a platform should flush promptly
//! (the browser flushes on every write, and again when the page is hidden).

use crate::outbox::{CommandId, Epoch, FailedEntry, OutboxEntry};

use crate::notify::Notify;
use crate::storage::{Loaded, Storage, StorageError, StorageOp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use todo_core::state::State;

/// A key of the persisted key-value image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// The snapshot of the confirmed state and its cursor.
    Snapshot,
    /// The log epoch.
    Epoch,
    /// The outbox.
    Outbox,
    /// The "couldn't sync" list.
    Failed,
}

impl Key {
    /// Every key.
    pub const ALL: [Key; 4] = [Key::Snapshot, Key::Epoch, Key::Outbox, Key::Failed];

    /// The key's name in the backend.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Key::Snapshot => "snapshot",
            Key::Epoch => "epoch",
            Key::Outbox => "outbox",
            Key::Failed => "failed",
        }
    }

    /// The key with this name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Key> {
        Key::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// One change to the persisted image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Set a key to a JSON value.
    Put(Key, String),
    /// Remove a key.
    Delete(Key),
    /// Store an event (JSON) under its sequence number.
    PutEvent(u64, String),
    /// Remove every event.
    ClearEvents,
}

/// The changes of one [`Storage::write`], to be applied atomically.
pub type Batch = Vec<Change>;

/// The persisted image: what a backend holds, and what it reads back on start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Persisted {
    /// The key-value part.
    pub kv: BTreeMap<Key, String>,
    /// Events by sequence number.
    pub events: BTreeMap<u64, String>,
}

impl Persisted {
    /// Applies one change (what a backend does, in memory; tests use it to check the journal).
    pub fn apply(&mut self, change: &Change) {
        match change {
            Change::Put(k, v) => {
                self.kv.insert(*k, v.clone());
            }
            Change::Delete(k) => {
                self.kv.remove(k);
            }
            Change::PutEvent(seq, v) => {
                self.events.insert(*seq, v.clone());
            }
            Change::ClearEvents => self.events.clear(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SnapshotRecord<'a> {
    cursor: u64,
    #[serde(borrow)]
    state: &'a serde_json::value::RawValue,
}

#[derive(Default)]
struct Image {
    /// `(state JSON, cursor)`.
    snapshot: Option<(String, u64)>,
    events: BTreeMap<u64, String>,
    epoch: Option<Epoch>,
    outbox: Vec<(CommandId, String)>,
    failed: Vec<(CommandId, String)>,
}

struct Inner {
    image: Image,
    journal: VecDeque<Batch>,
}

/// In-memory [`Storage`] with a journal of changes for a background backend. Clones share everything.
#[derive(Clone)]
pub struct MirrorStorage {
    inner: Arc<Mutex<Inner>>,
    written: Notify,
}

impl fmt::Debug for MirrorStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = self.lock();
        f.debug_struct("MirrorStorage")
            .field("events", &g.image.events.len())
            .field("outbox", &g.image.outbox.len())
            .field("journal", &g.journal.len())
            .finish_non_exhaustive()
    }
}

impl Default for MirrorStorage {
    fn default() -> Self {
        Self::new()
    }
}

fn ids_of(list: &str) -> Result<Vec<(CommandId, String)>, StorageError> {
    #[derive(Deserialize)]
    struct WithId {
        command_id: CommandId,
    }
    #[derive(Deserialize)]
    struct FailedWithEntry {
        entry: WithId,
    }
    let raw: Vec<Box<serde_json::value::RawValue>> = serde_json::from_str(list)?;
    raw.into_iter()
        .map(|r| {
            let id = match serde_json::from_str::<WithId>(r.get()) {
                Ok(w) => w.command_id,
                Err(_) => {
                    serde_json::from_str::<FailedWithEntry>(r.get())?
                        .entry
                        .command_id
                }
            };
            Ok((id, r.get().to_owned()))
        })
        .collect()
}

fn join(list: &[(CommandId, String)]) -> String {
    let mut out = String::from("[");
    for (i, (_, json)) in list.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(json);
    }
    out.push(']');
    out
}

fn put(list: &mut Vec<(CommandId, String)>, id: CommandId, json: String) {
    match list.iter_mut().find(|(k, _)| *k == id) {
        Some(slot) => slot.1 = json,
        None => list.push((id, json)),
    }
}

impl MirrorStorage {
    /// Empty storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                image: Image::default(),
                journal: VecDeque::new(),
            })),
            written: Notify::default(),
        }
    }

    /// Storage holding what a backend read back. Fails if the image cannot be decoded (the caller
    /// should then clear the backend and start empty: the server has everything but the outbox).
    pub fn from_persisted(p: Persisted) -> Result<Self, StorageError> {
        let mut image = Image::default();
        if let Some(s) = p.kv.get(&Key::Snapshot) {
            let rec: SnapshotRecord<'_> = serde_json::from_str(s)?;
            image.snapshot = Some((rec.state.get().to_owned(), rec.cursor));
        }
        if let Some(e) = p.kv.get(&Key::Epoch) {
            image.epoch = Some(serde_json::from_str(e)?);
        }
        if let Some(o) = p.kv.get(&Key::Outbox) {
            image.outbox = ids_of(o)?;
        }
        if let Some(f) = p.kv.get(&Key::Failed) {
            image.failed = ids_of(f)?;
        }
        image.events = p.events;
        let s = Self::new();
        s.lock().image = image;
        Ok(s)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes every batch written since the last call, oldest first.
    #[must_use]
    pub fn take_batches(&self) -> Vec<Batch> {
        self.lock().journal.drain(..).collect()
    }

    /// Puts batches back at the front of the journal (the backend failed to write them; they are
    /// retried before anything newer).
    pub fn requeue(&self, batches: Vec<Batch>) {
        let mut g = self.lock();
        for b in batches.into_iter().rev() {
            g.journal.push_front(b);
        }
    }

    /// True if batches are waiting to be persisted.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.lock().journal.is_empty()
    }

    /// Changes after every write; a backend's flusher waits on it.
    #[must_use]
    pub fn written(&self) -> Notify {
        self.written.clone()
    }

    /// The whole image as a backend would hold it once every batch has landed.
    #[must_use]
    pub fn persisted(&self) -> Persisted {
        let g = self.lock();
        let mut p = Persisted {
            events: g.image.events.clone(),
            ..Persisted::default()
        };
        if let Some((state, cursor)) = &g.image.snapshot {
            p.kv.insert(Key::Snapshot, snapshot_json(state, *cursor));
        }
        if let Some(e) = g.image.epoch
            && let Ok(json) = serde_json::to_string(&e)
        {
            p.kv.insert(Key::Epoch, json);
        }
        if !g.image.outbox.is_empty() {
            p.kv.insert(Key::Outbox, join(&g.image.outbox));
        }
        if !g.image.failed.is_empty() {
            p.kv.insert(Key::Failed, join(&g.image.failed));
        }
        p
    }
}

fn snapshot_json(state: &str, cursor: u64) -> String {
    format!("{{\"cursor\":{cursor},\"state\":{state}}}")
}

impl Storage for MirrorStorage {
    fn load(&mut self) -> Result<Loaded, StorageError> {
        let g = self.lock();
        let d = &g.image;
        Ok(Loaded {
            snapshot: d
                .snapshot
                .as_ref()
                .map(|(s, c)| serde_json::from_str::<State>(s).map(|s| (s, *c)))
                .transpose()?,
            events: d
                .events
                .values()
                .map(|e| serde_json::from_str(e))
                .collect::<Result<_, _>>()?,
            epoch: d.epoch,
            outbox: d
                .outbox
                .iter()
                .map(|(_, e)| serde_json::from_str::<OutboxEntry>(e))
                .collect::<Result<_, _>>()?,
            failed: d
                .failed
                .iter()
                .map(|(_, e)| serde_json::from_str::<FailedEntry>(e))
                .collect::<Result<_, _>>()?,
        })
    }

    fn write(&mut self, ops: &[StorageOp<'_>]) -> Result<(), StorageError> {
        // Encode everything first, so a failure leaves the image and the journal untouched.
        let mut g = self.lock();
        let mut next = Image {
            snapshot: g.image.snapshot.clone(),
            events: g.image.events.clone(),
            epoch: g.image.epoch,
            outbox: g.image.outbox.clone(),
            failed: g.image.failed.clone(),
        };
        let mut batch = Batch::new();
        let (mut outbox_changed, mut failed_changed) = (false, false);
        for op in ops {
            match op {
                StorageOp::AppendEvents(events) => {
                    for e in *events {
                        let seq = e
                            .event
                            .seq
                            .ok_or_else(|| StorageError::Backend("event without seq".into()))?;
                        let json = serde_json::to_string(e)?;
                        next.events.insert(seq, json.clone());
                        batch.push(Change::PutEvent(seq, json));
                    }
                }
                StorageOp::Snapshot { state, cursor } => {
                    let json = serde_json::to_string(state)?;
                    batch.push(Change::ClearEvents);
                    batch.push(Change::Put(Key::Snapshot, snapshot_json(&json, *cursor)));
                    next.snapshot = Some((json, *cursor));
                    next.events.clear();
                }
                StorageOp::ClearConfirmed => {
                    next.snapshot = None;
                    next.events.clear();
                    next.epoch = None;
                    batch.extend([
                        Change::Delete(Key::Snapshot),
                        Change::ClearEvents,
                        Change::Delete(Key::Epoch),
                    ]);
                }
                StorageOp::SetEpoch(e) => {
                    next.epoch = Some(*e);
                    batch.push(Change::Put(Key::Epoch, serde_json::to_string(e)?));
                }
                StorageOp::PutOutbox(e) => {
                    put(&mut next.outbox, e.command_id, serde_json::to_string(e)?);
                    outbox_changed = true;
                }
                StorageOp::RemoveOutbox(id) => {
                    next.outbox.retain(|(k, _)| k != id);
                    outbox_changed = true;
                }
                StorageOp::PutFailed(f) => {
                    put(
                        &mut next.failed,
                        f.entry.command_id,
                        serde_json::to_string(f)?,
                    );
                    failed_changed = true;
                }
                StorageOp::RemoveFailed(id) => {
                    next.failed.retain(|(k, _)| k != id);
                    failed_changed = true;
                }
            }
        }
        for (changed, key, list) in [
            (outbox_changed, Key::Outbox, &next.outbox),
            (failed_changed, Key::Failed, &next.failed),
        ] {
            if changed {
                batch.push(if list.is_empty() {
                    Change::Delete(key)
                } else {
                    Change::Put(key, join(list))
                });
            }
        }
        g.image = next;
        if !batch.is_empty() {
            g.journal.push_back(batch);
        }
        drop(g);
        self.written.notify();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_round_trip() {
        for k in Key::ALL {
            assert_eq!(Key::from_name(k.name()), Some(k));
        }
        assert_eq!(Key::from_name("nope"), None);
    }

    #[test]
    fn an_empty_image_loads_empty() {
        let mut s = MirrorStorage::from_persisted(Persisted::default()).unwrap();
        assert_eq!(s.load().unwrap(), Loaded::default());
        assert!(!s.has_pending());
    }

    #[test]
    fn a_corrupt_image_is_refused() {
        let mut p = Persisted::default();
        p.kv.insert(Key::Outbox, "{".into());
        assert!(MirrorStorage::from_persisted(p).is_err());
    }
}
