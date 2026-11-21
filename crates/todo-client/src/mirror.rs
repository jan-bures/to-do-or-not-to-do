//! A [`Storage`] for platforms whose durable storage is asynchronous (IndexedDB in the browser).
//!
//! The engine calls [`Storage`] synchronously while it holds its state lock, so such a platform keeps
//! the data in memory and persists it in the background. [`MirrorStorage`] is that memory: it applies
//! every [`write`](Storage::write) at once, and records it as one [`Batch`] of [`Change`]s in a
//! journal. The platform drains the journal ([`MirrorStorage::take_batches`]) and writes each batch
//! atomically, in order (one IndexedDB transaction per batch). On start it reads the persisted
//! [`Persisted`] image back and opens [`MirrorStorage::recover`] on it.
//!
//! # The persisted image
//!
//! | Part | Holds |
//! |---|---|
//! | [`Key::Snapshot`] | `{"cursor": n, "state": <State>}` |
//! | [`Key::Epoch`] | the log epoch |
//! | events | each [`LogEvent`](crate::protocol::LogEvent) under its `seq` |
//! | outbox | each [`OutboxEntry`] under its command ID |
//! | failed | each [`FailedEntry`] ("couldn't sync") under its command ID |
//! | damaged | records that could not be read, set aside (never deleted automatically) |
//!
//! # Several tabs
//!
//! Every tab of the app has its own `MirrorStorage` over the same database, and none of them knows
//! what the others wrote. So outbox and failed entries are stored **one record per command**, and a
//! write only puts or deletes the records of the commands it touches: a tab never overwrites another
//! tab's unsent work. On start the entries of every tab are read back, ordered by submission time.
//! The confirmed part (snapshot, events, epoch) is a function of the server's log, so whichever tab
//! wrote it last is right; the replica skips stored events it already has and refetches after a gap.
//!
//! # Recovery
//!
//! [`MirrorStorage::recover`] never fails. What cannot be read is handled by what it holds:
//!
//! - an unreadable snapshot or epoch drops the confirmed part (snapshot, events, epoch), which is
//!   fetched from the server again; an unreadable event drops the stored events after the snapshot;
//! - an unreadable outbox or failed entry is moved to the damaged area, with its original key, so
//!   unsent work is never deleted.
//!
//! The repairs are the first batch in the journal. What is lost if the page closes before a batch
//! lands is described on [`crate::storage`]: confirmed events are fetched again, and a lost outbox
//! removal only leads to an idempotent resend. Lost outbox *additions* are the exception, so a
//! platform should flush promptly (the browser flushes on every write, and again when the page is
//! hidden).

use crate::outbox::{CommandId, Epoch, FailedEntry, OutboxEntry};

use crate::notify::Notify;
use crate::protocol::LogEvent;
use crate::storage::{Loaded, Storage, StorageError, StorageOp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use todo_core::state::State;

/// A key of the persisted key-value part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// The snapshot of the confirmed state and its cursor.
    Snapshot,
    /// The log epoch.
    Epoch,
    /// The whole outbox as one JSON array, as the first version of the web app stored it. Read
    /// (and split into entries) on start, never written.
    LegacyOutbox,
    /// The whole "couldn't sync" list as one JSON array (first version). Read on start, never written.
    LegacyFailed,
}

impl Key {
    /// Every key.
    pub const ALL: [Key; 4] = [
        Key::Snapshot,
        Key::Epoch,
        Key::LegacyOutbox,
        Key::LegacyFailed,
    ];

    /// The key's name in the backend.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Key::Snapshot => "snapshot",
            Key::Epoch => "epoch",
            Key::LegacyOutbox => "outbox",
            Key::LegacyFailed => "failed",
        }
    }

    /// The key with this name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Key> {
        Key::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// Which per-command store a record belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Area {
    /// The outbox.
    Outbox,
    /// The "couldn't sync" list.
    Failed,
}

impl Area {
    /// The store's name in the backend.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Area::Outbox => "outbox",
            Area::Failed => "failed",
        }
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
    /// Store an outbox or failed entry (JSON) under its key (the command ID).
    PutEntry(Area, String, String),
    /// Remove an outbox or failed entry (no-op if absent).
    DeleteEntry(Area, String),
    /// Set a record aside under a key (see [`damaged_key`]).
    PutDamaged(String, String),
}

/// The changes of one [`Storage::write`], to be applied atomically.
pub type Batch = Vec<Change>;

/// The key a damaged record is set aside under: where it was, and its key there.
#[must_use]
pub fn damaged_key(from: &str, key: &str) -> String {
    format!("{from}/{key}")
}

/// The persisted image: what a backend holds, and what it reads back on start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Persisted {
    /// The key-value part.
    pub kv: BTreeMap<Key, String>,
    /// Events by sequence number.
    pub events: BTreeMap<u64, String>,
    /// Outbox entries by key (the command ID).
    pub outbox: BTreeMap<String, String>,
    /// "Couldn't sync" entries by key (the command ID).
    pub failed: BTreeMap<String, String>,
    /// Records set aside because they could not be read.
    pub damaged: BTreeMap<String, String>,
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
            Change::PutEntry(a, k, v) => {
                self.area(*a).insert(k.clone(), v.clone());
            }
            Change::DeleteEntry(a, k) => {
                self.area(*a).remove(k);
            }
            Change::PutDamaged(k, v) => {
                self.damaged.insert(k.clone(), v.clone());
            }
        }
    }

    fn area(&mut self, a: Area) -> &mut BTreeMap<String, String> {
        match a {
            Area::Outbox => &mut self.outbox,
            Area::Failed => &mut self.failed,
        }
    }
}

/// What [`MirrorStorage::recover`] had to repair.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    /// The confirmed state could not be read and is fetched from the server again.
    pub dropped_confirmed: bool,
    /// Stored events could not be read and are fetched again.
    pub dropped_events: bool,
    /// Keys of the records set aside (see [`damaged_key`]) by this start.
    pub damaged: Vec<String>,
    /// Records already set aside by an earlier start.
    pub damaged_before: usize,
}

impl Recovery {
    /// True if unsent work could not be read (it was set aside, now or earlier).
    #[must_use]
    pub fn has_damaged(&self) -> bool {
        !self.damaged.is_empty() || self.damaged_before > 0
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

fn put(list: &mut Vec<(CommandId, String)>, id: CommandId, json: String) {
    match list.iter_mut().find(|(k, _)| *k == id) {
        Some(slot) => slot.1 = json,
        None => list.push((id, json)),
    }
}

/// The record key of a command.
fn entry_key(id: CommandId) -> String {
    id.hyphenated().to_string()
}

/// Reads the per-command records of one area (plus the legacy array, if any). What cannot be read is
/// set aside; legacy entries are rewritten as records. Returns the entries in submission order.
fn recover_entries<T: serde::de::DeserializeOwned>(
    area: Area,
    records: BTreeMap<String, String>,
    legacy: Option<(Key, String)>,
    id_of: impl Fn(&T) -> CommandId,
    order: impl Fn(&T) -> i64,
    repairs: &mut Batch,
    damaged: &mut Vec<String>,
) -> Vec<(CommandId, String)> {
    let mut found: BTreeMap<CommandId, (T, String)> = BTreeMap::new();
    for (key, json) in records {
        match serde_json::from_str::<T>(&json) {
            Ok(v) if entry_key(id_of(&v)) == key => {
                found.insert(id_of(&v), (v, json));
            }
            _ => {
                let to = damaged_key(area.name(), &key);
                repairs.push(Change::PutDamaged(to.clone(), json));
                repairs.push(Change::DeleteEntry(area, key));
                damaged.push(to);
            }
        }
    }
    if let Some((key, list)) = legacy {
        match serde_json::from_str::<Vec<Box<serde_json::value::RawValue>>>(&list) {
            Ok(items) => {
                for (i, raw) in items.into_iter().enumerate() {
                    let json = raw.get().to_owned();
                    match serde_json::from_str::<T>(&json) {
                        Ok(v) => {
                            if let std::collections::btree_map::Entry::Vacant(slot) =
                                found.entry(id_of(&v))
                            {
                                repairs.push(Change::PutEntry(
                                    area,
                                    entry_key(*slot.key()),
                                    json.clone(),
                                ));
                                slot.insert((v, json));
                            }
                        }
                        Err(_) => {
                            let to = damaged_key(key.name(), &i.to_string());
                            repairs.push(Change::PutDamaged(to.clone(), json));
                            damaged.push(to);
                        }
                    }
                }
            }
            Err(_) => {
                let to = damaged_key(key.name(), "all");
                repairs.push(Change::PutDamaged(to.clone(), list));
                damaged.push(to);
            }
        }
        repairs.push(Change::Delete(key));
    }
    let mut out: Vec<_> = found
        .into_iter()
        .map(|(id, (v, json))| (order(&v), id, json))
        .collect();
    out.sort_by_key(|(at, id, _)| (*at, *id));
    out.into_iter().map(|(_, id, json)| (id, json)).collect()
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

    /// Storage holding what a backend read back, with what had to be repaired. Never fails: see the
    /// module docs. The repairs are the first batch of the journal, so the backend applies them
    /// before anything newer.
    #[must_use]
    pub fn recover(p: Persisted) -> (Self, Recovery) {
        let mut image = Image::default();
        let mut repairs = Batch::new();
        let mut rec = Recovery {
            damaged_before: p.damaged.len(),
            ..Recovery::default()
        };
        let mut kv = p.kv;

        // The confirmed part: snapshot and epoch, then the events after the snapshot.
        let snapshot = kv.remove(&Key::Snapshot).map(|s| {
            serde_json::from_str::<SnapshotRecord<'_>>(&s)
                .ok()
                .filter(|r| serde_json::from_str::<State>(r.state.get()).is_ok())
                .map(|r| (r.state.get().to_owned(), r.cursor))
        });
        let epoch = kv
            .remove(&Key::Epoch)
            .map(|e| serde_json::from_str::<Epoch>(&e).ok());
        if matches!(snapshot, Some(None)) || matches!(epoch, Some(None)) {
            rec.dropped_confirmed = true;
            repairs.extend([
                Change::Delete(Key::Snapshot),
                Change::ClearEvents,
                Change::Delete(Key::Epoch),
            ]);
        } else {
            image.snapshot = snapshot.flatten();
            image.epoch = epoch.flatten();
            if p.events
                .values()
                .all(|e| serde_json::from_str::<LogEvent>(e).is_ok())
            {
                image.events = p.events;
            } else {
                rec.dropped_events = true;
                repairs.push(Change::ClearEvents);
            }
        }

        // Unsent work: never dropped.
        image.outbox = recover_entries::<OutboxEntry>(
            Area::Outbox,
            p.outbox,
            kv.remove(&Key::LegacyOutbox)
                .map(|v| (Key::LegacyOutbox, v)),
            |e| e.command_id,
            |e| e.submitted_at.as_millisecond(),
            &mut repairs,
            &mut rec.damaged,
        );
        image.failed = recover_entries::<FailedEntry>(
            Area::Failed,
            p.failed,
            kv.remove(&Key::LegacyFailed)
                .map(|v| (Key::LegacyFailed, v)),
            |f| f.entry.command_id,
            |f| f.failed_at.as_millisecond(),
            &mut repairs,
            &mut rec.damaged,
        );

        let s = Self::new();
        {
            let mut g = s.lock();
            g.image = image;
            if !repairs.is_empty() {
                g.journal.push_back(repairs);
            }
        }
        (s, rec)
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

    /// The whole image as a backend would hold it once every batch has landed (without the damaged
    /// area, which only the backend holds).
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
        for (id, json) in &g.image.outbox {
            p.outbox.insert(entry_key(*id), json.clone());
        }
        for (id, json) in &g.image.failed {
            p.failed.insert(entry_key(*id), json.clone());
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
                    let json = serde_json::to_string(e)?;
                    put(&mut next.outbox, e.command_id, json.clone());
                    batch.push(Change::PutEntry(
                        Area::Outbox,
                        entry_key(e.command_id),
                        json,
                    ));
                }
                StorageOp::RemoveOutbox(id) => {
                    next.outbox.retain(|(k, _)| k != id);
                    batch.push(Change::DeleteEntry(Area::Outbox, entry_key(*id)));
                }
                StorageOp::PutFailed(f) => {
                    let json = serde_json::to_string(f)?;
                    put(&mut next.failed, f.entry.command_id, json.clone());
                    batch.push(Change::PutEntry(
                        Area::Failed,
                        entry_key(f.entry.command_id),
                        json,
                    ));
                }
                StorageOp::RemoveFailed(id) => {
                    next.failed.retain(|(k, _)| k != id);
                    batch.push(Change::DeleteEntry(Area::Failed, entry_key(*id)));
                }
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
        let (mut s, rec) = MirrorStorage::recover(Persisted::default());
        assert_eq!(s.load().unwrap(), Loaded::default());
        assert!(!s.has_pending());
        assert_eq!(rec, Recovery::default());
    }
}
