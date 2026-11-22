//! The single writer: validates commands and undos with `todo-core`, stores the events and projections
//! atomically, keeps the authoritative in-memory [`State`], and broadcasts new events.
//!
//! Every write goes through one mutex, so commands are strictly serialised. The in-memory state is an
//! immutable snapshot (`Arc<State>`): readers clone the `Arc` and never hold a lock while they work. A
//! write applies its events to a private copy of the snapshot, commits the SQLite transaction, and only
//! then swaps the copy in. A failed write therefore never leaves uncommitted changes visible; if the
//! connection cannot even roll back, the engine is *poisoned* and refuses all further work (503) until
//! it is restarted.
//!
//! The engine owns the database exclusively: [`Engine::open`] (and [`Engine::rebuild_projections`])
//! take an OS file lock next to the database file before reading it, so a second server process on the
//! same data directory fails fast with [`StoreError::Locked`].
//!
//! ## Cancellation
//!
//! Every write takes the request's [`Cancel`] (see [`crate::cancel`]). It checks it before waiting for
//! the writer and after getting it, and immediately before `COMMIT` it claims the commit with
//! [`Cancel::begin_commit`], an atomic pending → committing transition that is refused once the request
//! was cancelled or its absolute deadline passed. So a write either never commits (the client gets 408),
//! or it owns its outcome and the request waits for it (see the deadline middleware in [`crate::app`]).
//! If even that wait runs out, the client gets 503 `outcome_unknown` and a retry with the same
//! `command_id` returns the stored result: a client that retries with the same `command_id` always gets
//! exactly one execution.
//!
//! All methods block: call them from the blocking pool (see [`crate::api::blocking`]).

pub use crate::cancel::{Cancel, CancelOnDrop};
use crate::db::{self, CommandRecord, LogEvent, StoreError};
use crate::scope::{
    EventScope, ScopeBuilder, Whereabouts, location, record_whereabouts, whereabouts_change,
};
use crate::summary;
use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use todo_core::command::Command;
use todo_core::error::{DomainError, UndoError};
use todo_core::event::Event;
use todo_core::ids::{EntityId, EventId, TaskId};
use todo_core::time::Timestamp;
use todo_core::undo::{UndoOptions, undo_batch};
use todo_core::{Actor, AgentId, Grants, State, apply, decide};
use tokio::sync::broadcast;
use uuid::Uuid;

/// Source of the current time.
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// The system clock.
#[must_use]
pub fn system_clock() -> Clock {
    Arc::new(Timestamp::now)
}

/// A committed event and its scope, as broadcast to live subscribers.
#[derive(Debug, Clone)]
pub struct LiveEvent {
    /// The event.
    pub event: LogEvent,
    /// Where it happened (see [`crate::scope`]).
    pub scope: EventScope,
}

/// Capacity of the live event broadcast channel. Slower subscribers lag and re-read from the database.
pub const BROADCAST_CAPACITY: usize = 1024;

/// Reference to a stored event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EventRef {
    /// Position in the log.
    pub seq: u64,
    /// Event ID.
    pub id: EventId,
}

/// What a command or undo produced. Stored, and returned verbatim when the command is retried.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CommandResult {
    /// The client-chosen command ID.
    pub command_id: Uuid,
    /// The stored events, in log order. Empty if the command changed nothing.
    pub events: Vec<EventRef>,
    /// One-line description for the activity feed.
    pub summary: String,
}

/// A command result plus whether it was replayed from an earlier identical request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The result.
    pub result: CommandResult,
    /// True if this was a retry and nothing new was stored.
    pub replayed: bool,
}

/// Why a command or undo was not applied.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The command failed validation.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// The undo was refused. `seqs` maps event IDs mentioned in the error to their sequence numbers.
    #[error("{error}")]
    Undo {
        /// The refusal.
        error: UndoError,
        /// Sequence numbers of the events the error refers to.
        seqs: BTreeMap<EventId, u64>,
    },
    /// No event has this sequence number.
    #[error("event {0} not found")]
    EventNotFound(u64),
    /// The request was made against another event log (see [`Engine::epoch`]).
    #[error("the request refers to another event log")]
    EpochMismatch {
        /// This log's epoch.
        current: Uuid,
    },
    /// The event at the undo's sequence number is not the one the client meant.
    #[error("event {seq} is not the expected event")]
    UndoTargetMismatch {
        /// The sequence number.
        seq: u64,
        /// The event the client expected there.
        expected: EventId,
        /// The event that is there.
        found: EventId,
    },
    /// The command ID was used before for a different request.
    #[error("command id {0} was already used for a different request")]
    CommandIdReused(Uuid),
    /// The request was abandoned (its deadline passed) before anything was stored.
    #[error("the request was cancelled before it was stored")]
    Cancelled,
    /// The engine was closed, or failed in a way that needs a restart.
    #[error("the server cannot accept writes right now")]
    Unavailable,
    /// Storage failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The unique key of an [`Engine::execute_unique`] write was already taken. Handled inside the
    /// engine (the caller gets [`UniqueOutcome::Duplicate`]); never returned.
    #[error("the unique key is already taken")]
    DuplicateKey,
    /// The admission check of an [`Engine::execute_unique`] write said no. Handled inside the engine
    /// (the caller gets [`UniqueOutcome::Refused`]); never returned.
    #[error("the write was refused")]
    Refused,
    /// The write would make an attachment live again whose content the garbage collector is deleting
    /// (see [`Engine::set_commit_check`] and [`crate::blobs`]).
    #[error("the content of {0} is being deleted; it cannot be restored now")]
    ContentPurging(EntityId),
}

/// A last check of every write, inside the writer, against the state before and after it. An error
/// rolls the write back. See [`Engine::set_commit_check`].
pub type CommitCheck = Arc<dyn Fn(&State, &State) -> Result<(), EngineError> + Send + Sync>;

/// What [`Engine::execute_unique`] stores: commands that create one task, applied as one group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniquePlan {
    /// The task the commands create (recorded with the key).
    pub task: TaskId,
    /// The commands, decided in order against the state the previous ones produce.
    pub commands: Vec<Command>,
}

/// The result of [`Engine::execute_unique`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniqueOutcome {
    /// The key was new: the commands were stored.
    Created {
        /// The created task.
        task: TaskId,
        /// The stored events.
        result: CommandResult,
    },
    /// The key was taken: nothing was stored.
    Duplicate {
        /// The task stored with the key the first time.
        task: TaskId,
    },
    /// The admission check, made inside the transaction, refused the write: nothing was stored.
    Refused,
}

struct Writer {
    /// `None` once [`Engine::close`] closed it.
    conn: Option<Connection>,
    /// Milliseconds of the latest event time handed out, so event times strictly increase.
    last_ms: i64,
    /// The exclusive ownership lock on the database; released by [`Engine::close`].
    owner_lock: Option<db::OwnerLock>,
    /// Set by [`Engine::close`]: no more writes.
    closed: bool,
}

/// The authoritative state and the single writer.
pub struct Engine {
    path: PathBuf,
    state: RwLock<Arc<State>>,
    /// Last known locations of entities no longer in `state` (see [`crate::scope`]).
    gone: RwLock<Arc<Whereabouts>>,
    writer: Mutex<Writer>,
    readers: Mutex<Vec<Connection>>,
    live: broadcast::Sender<Arc<LiveEvent>>,
    head: AtomicU64,
    /// The log epoch (see [`db::log_epoch`]).
    epoch: Uuid,
    clock: Clock,
    /// Set when a failed write could not be rolled back.
    poisoned: AtomicBool,
    /// Test hook: milliseconds to sleep between claiming a commit and committing.
    #[cfg(feature = "test-hooks")]
    commit_delay_ms: AtomicU64,
    /// Test hook: a claimed commit waits for this lock (see [`Engine::hold_commits`]).
    #[cfg(feature = "test-hooks")]
    commit_gate: Mutex<()>,
    /// Writes waiting for the writer (see [`Engine::waiting_writes`]).
    waiting: AtomicUsize,
    /// See [`Engine::set_commit_check`].
    commit_check: RwLock<Option<CommitCheck>>,
}

/// Keeps claimed commits from committing (see [`Engine::hold_commits`]).
#[cfg(feature = "test-hooks")]
#[derive(Debug)]
#[must_use]
pub struct CommitHold<'a> {
    _guard: MutexGuard<'a, ()>,
}

/// Holds the writer lock (see [`Engine::hold_writer`]).
pub struct WriterHold<'a> {
    _guard: MutexGuard<'a, Writer>,
    engine: &'a Engine,
}

impl std::fmt::Debug for WriterHold<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WriterHold")
    }
}

impl WriterHold<'_> {
    /// Replaces the agents' grants like [`Engine::set_grants`], while the writer stays held. For tests
    /// that change access between a write's pre-checks and its turn at the writer.
    #[doc(hidden)]
    pub fn set_grants(&self, grants: BTreeMap<AgentId, Grants>) {
        self.engine.replace_grants(grants);
    }
}

/// What a write's check sees inside the writer (see [`Engine::execute_checked`]): the state the write
/// is decided against and the matching whereabouts, neither of which can change until the write is
/// done, and the log through the writer's connection.
pub struct Locked<'a> {
    /// The state the idempotency lookup and `decide` use.
    pub state: &'a State,
    /// The last known locations of entities no longer in `state`.
    pub gone: &'a Whereabouts,
    conn: &'a Connection,
}

impl std::fmt::Debug for Locked<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Locked")
    }
}

impl Locked<'_> {
    /// The event at `seq` with its recorded scope (see [`crate::scope`]).
    pub fn event_with_scope(
        &self,
        seq: u64,
    ) -> Result<Option<(Event, Option<EventScope>)>, StoreError> {
        let Some(event) = db::event_by_seq(self.conn, seq)? else {
            return Ok(None);
        };
        let scope = db::scopes_between(self.conn, seq, seq)?.remove(&seq);
        Ok(Some((event, scope)))
    }
}

impl Writer {
    fn conn(&self) -> Result<&Connection, EngineError> {
        self.conn.as_ref().ok_or(EngineError::Unavailable)
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("path", &self.path)
            .field("head", &self.head_seq())
            .finish_non_exhaustive()
    }
}

/// A pooled read-only connection, returned to the pool on drop.
struct Reader<'a> {
    engine: &'a Engine,
    conn: Option<Connection>,
}

impl std::ops::Deref for Reader<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        // Always `Some` until drop.
        self.conn.as_ref().unwrap_or_else(|| unreachable!())
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            let mut pool = lock(&self.engine.readers);
            if pool.len() < 8 {
                pool.push(c);
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn request_hash(kind: &str, actor: &Actor, body: &impl Serialize) -> Result<String, StoreError> {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b"\0");
    h.update(serde_json::to_vec(actor)?);
    h.update(b"\0");
    h.update(serde_json::to_vec(body)?);
    Ok(hex::encode(h.finalize()))
}

/// What a write records besides its events.
struct Pending<'a> {
    actor: &'a Actor,
    command_id: Uuid,
    now: Timestamp,
    kind: &'static str,
    request_hash: String,
    cancel: &'a Cancel,
}

#[derive(Serialize)]
struct UndoRequestBody {
    seq: u64,
    force: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_event_id: Option<EventId>,
}

/// What the client expects an undo's target to be. Checked inside the writer, so nothing can change
/// between the check and the undo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndoExpect {
    /// The ID of the event at the given sequence number.
    pub event_id: EventId,
    /// The log epoch the client saw it in.
    pub epoch: Option<Uuid>,
}

fn millis(ts: Timestamp) -> i64 {
    ts.as_millisecond()
}

impl Engine {
    /// Opens the database, runs migrations, folds the event log into memory and checks the projections
    /// against it, rebuilding them if they differ.
    ///
    /// Fails with [`StoreError::Locked`] if another engine (in this or another process) owns the
    /// database.
    pub fn open(path: &Path, clock: Clock) -> Result<Self, StoreError> {
        let owner_lock = db::lock_database(path)?;
        let mut conn = db::open_writer(path)?;
        let backfilled = db::backfill_scopes(&mut conn)?;
        if backfilled > 0 {
            tracing::info!(events = backfilled, "recorded the scopes of earlier events");
        }
        let (state, gone, ledger) = db::fold_all(&conn)?;
        let head = db::head_seq(&conn)?;
        let projected = db::load_projections(&conn)?;
        let proj_seq = db::projection_seq(&conn)?;
        if !projected.data_eq(&state) || proj_seq.unwrap_or(0) != head {
            tracing::warn!(
                head,
                projection_seq = ?proj_seq,
                "projections do not match the event log; rebuilding them"
            );
            db::write_all_projections(&mut conn, &state, head)?;
        }
        if db::sync_ledger(&mut conn, &ledger)? {
            tracing::warn!(
                messages = ledger.len(),
                "the email Message-ID ledger did not match the event log; rebuilt it"
            );
        }
        let epoch = match db::log_epoch(&conn)? {
            Some(e) => e,
            None => {
                let e = Uuid::new_v4();
                db::set_log_epoch(&conn, e)?;
                e
            }
        };
        let last_ms = match db::events_before(&conn, None, 1)?.first() {
            Some(e) => millis(e.at),
            None => 0,
        };
        tracing::info!(head, "event log loaded");
        let (live, _) = broadcast::channel(BROADCAST_CAPACITY);
        Ok(Self {
            path: path.to_owned(),
            state: RwLock::new(Arc::new(state)),
            gone: RwLock::new(Arc::new(gone)),
            writer: Mutex::new(Writer {
                conn: Some(conn),
                last_ms,
                owner_lock: Some(owner_lock),
                closed: false,
            }),
            readers: Mutex::new(Vec::new()),
            live,
            head: AtomicU64::new(head),
            epoch,
            clock,
            poisoned: AtomicBool::new(false),
            #[cfg(feature = "test-hooks")]
            commit_delay_ms: AtomicU64::new(0),
            #[cfg(feature = "test-hooks")]
            commit_gate: Mutex::new(()),
            waiting: AtomicUsize::new(0),
            commit_check: RwLock::new(None),
        })
    }

    /// Rebuilds the projection tables of the database at `path` from its event log. Takes the same
    /// ownership lock as [`open`](Self::open), so it fails with [`StoreError::Locked`] while a server
    /// runs on the database.
    pub fn rebuild_projections(path: &Path) -> Result<u64, StoreError> {
        let _owner_lock = db::lock_database(path)?;
        let mut conn = db::open_writer(path)?;
        db::rebuild_projections(&mut conn)?;
        db::head_seq(&conn)
    }

    /// Gives the database at `path` a new log epoch and returns it. Run it after restoring a backup
    /// (with the server stopped): clients that followed the old log then resync from scratch instead
    /// of mixing two histories. Takes the ownership lock, like [`open`](Self::open).
    pub fn rotate_log_epoch(path: &Path) -> Result<Uuid, StoreError> {
        let _owner_lock = db::lock_database(path)?;
        let conn = db::open_writer(path)?;
        let epoch = Uuid::new_v4();
        db::set_log_epoch(&conn, epoch)?;
        Ok(epoch)
    }

    /// The log epoch: a random ID created with the database and rotated after a restore. A client
    /// whose cursor belongs to another epoch must resync from 0.
    #[must_use]
    pub fn epoch(&self) -> Uuid {
        self.epoch
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The highest committed sequence number.
    #[must_use]
    pub fn head_seq(&self) -> u64 {
        self.head.load(Ordering::Acquire)
    }

    /// The current time from the engine's clock.
    #[must_use]
    pub fn now(&self) -> Timestamp {
        (self.clock)()
    }

    /// Subscribes to events committed from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<LiveEvent>> {
        self.live.subscribe()
    }

    /// The current state: an immutable snapshot of everything committed so far.
    #[must_use]
    pub fn state(&self) -> Arc<State> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The last known locations of entities that are no longer in the state (see
    /// [`crate::scope::Whereabouts`]). Read it after [`state`](Self::state): an entity removed by a
    /// commit in between is then in neither, and history about it is hidden rather than shown.
    #[must_use]
    pub fn whereabouts(&self) -> Arc<Whereabouts> {
        self.gone
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the agents' access grants in the state (they are managed by the auth service, not
    /// event-sourced). Takes the writer lock, so no write in progress can put back the old grants.
    pub fn set_grants(&self, grants: BTreeMap<AgentId, Grants>) {
        let _w = lock(&self.writer);
        self.replace_grants(grants);
    }

    /// [`set_grants`](Self::set_grants) without the lock: the caller holds the writer.
    fn replace_grants(&self, grants: BTreeMap<AgentId, Grants>) {
        let mut next = (*self.state()).clone();
        next.grants = grants;
        *self.state.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
    }

    /// False once a failed write could not be rolled back; every request then gets 503.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        !self.poisoned.load(Ordering::Acquire)
    }

    /// Stops accepting writes, closes the writer connection and the pooled readers, and only then
    /// releases the ownership lock, so another engine can open the database. Waits for a running write
    /// to finish. Readers in use (event streams) close when they are returned.
    pub fn close(&self) {
        let mut w = lock(&self.writer);
        w.closed = true;
        drop(w.conn.take());
        lock(&self.readers).clear();
        // Dropping the guard unlocks at once (see `db::OwnerLock`).
        w.owner_lock = None;
    }

    /// Test hook: sleep this long between claiming a commit and committing, to widen the race between
    /// a commit and the request deadline. Only with the `test-hooks` feature.
    #[cfg(feature = "test-hooks")]
    pub fn set_commit_delay(&self, delay: std::time::Duration) {
        self.commit_delay_ms.store(
            u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    /// Test hook: until the returned guard is dropped, a write that claimed its commit waits (holding
    /// the writer) before it commits. For tests that need a commit to outlive the request deadline
    /// without betting on how long a fixed delay takes.
    /// Only with the `test-hooks` feature.
    #[cfg(feature = "test-hooks")]
    pub fn hold_commits(&self) -> CommitHold<'_> {
        CommitHold {
            _guard: lock(&self.commit_gate),
        }
    }

    #[cfg(feature = "test-hooks")]
    fn test_hooks_before_commit(&self) {
        let delay = self.commit_delay_ms.load(Ordering::Relaxed);
        if delay > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        drop(lock(&self.commit_gate));
    }

    /// Installs a check that every write (command, undo, unique write) must pass, run inside the
    /// writer with the state before and after the write. Used by [`crate::blobs`] so that no write can
    /// make an attachment live again while its content is being deleted.
    pub fn set_commit_check(&self, check: CommitCheck) {
        let _w = lock(&self.writer);
        *self
            .commit_check
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(check);
    }

    /// Runs `f` on the current state while holding the writer, so no write can commit until it
    /// returns. For decisions that must not go stale, like the garbage collector's last check before
    /// it deletes an object. Keep `f` short: every write waits for it.
    pub fn with_writer<R>(&self, f: impl FnOnce(&State) -> R) -> Result<R, EngineError> {
        let _w = self.writer(&Cancel::new())?;
        let state = self.state();
        Ok(f(&state))
    }

    /// Blocks every write until the returned guard is dropped. For tests that need a slow writer.
    #[doc(hidden)]
    pub fn hold_writer(&self) -> WriterHold<'_> {
        WriterHold {
            _guard: lock(&self.writer),
            engine: self,
        }
    }

    /// How many writes are waiting for the writer right now. For tests that must know a write got past
    /// its pre-checks before they change something under a [`hold_writer`](Self::hold_writer).
    #[doc(hidden)]
    #[must_use]
    pub fn waiting_writes(&self) -> usize {
        self.waiting.load(Ordering::Acquire)
    }

    fn writer(&self, cancel: &Cancel) -> Result<MutexGuard<'_, Writer>, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let w = lock(&self.writer);
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        if w.closed || w.conn.is_none() || !self.is_healthy() {
            return Err(EngineError::Unavailable);
        }
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        Ok(w)
    }

    fn reader(&self) -> Result<Reader<'_>, StoreError> {
        let pooled = lock(&self.readers).pop();
        let conn = match pooled {
            Some(c) => c,
            None => db::open_reader(&self.path)?,
        };
        Ok(Reader {
            engine: self,
            conn: Some(conn),
        })
    }

    /// Events after `since`, oldest first.
    pub fn events_since(&self, since: u64, limit: usize) -> Result<Vec<Event>, StoreError> {
        db::events_after(&*self.reader()?, since, limit)
    }

    /// The stored outcome of `command_id` if `actor` sent it, with when it was handled. Lets an
    /// adapter rebuild request defaults it resolved on the first attempt (see [`crate::mcp::tools`]).
    pub fn stored_command(
        &self,
        actor: &Actor,
        command_id: Uuid,
    ) -> Result<Option<(CommandResult, Timestamp)>, StoreError> {
        let Some(rec) = db::find_command_of(&*self.reader()?, &command_id.to_string(), actor)?
        else {
            return Ok(None);
        };
        let result: CommandResult = serde_json::from_str(&rec.result)?;
        Ok(Some((result, rec.received_at)))
    }

    /// Events after `since`, oldest first, within a byte budget (see [`db::events_after_capped`]).
    pub fn events_since_capped(
        &self,
        since: u64,
        limit: usize,
        max_bytes: usize,
    ) -> Result<db::CappedEvents, StoreError> {
        db::events_after_capped(&*self.reader()?, since, limit, max_bytes)
    }

    /// The event at `seq` with its recorded scope (see [`crate::scope`]).
    pub fn event_with_scope(
        &self,
        seq: u64,
    ) -> Result<Option<(Event, Option<EventScope>)>, StoreError> {
        let conn = self.reader()?;
        let Some(event) = db::event_by_seq(&conn, seq)? else {
            return Ok(None);
        };
        let scope = db::scopes_between(&conn, seq, seq)?.remove(&seq);
        Ok(Some((event, scope)))
    }

    /// Events before `before`, newest first, within a byte budget and optionally for one actor.
    pub fn events_before_capped(
        &self,
        before: Option<u64>,
        limit: usize,
        max_bytes: usize,
        actor: Option<&Actor>,
    ) -> Result<db::CappedEvents, StoreError> {
        db::events_before_capped(&*self.reader()?, before, limit, max_bytes, actor)
    }

    /// Events before `before` (or the newest), newest first.
    pub fn events_before(
        &self,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Event>, StoreError> {
        db::events_before(&*self.reader()?, before, limit)
    }

    /// Dumps the projection tables (for tests and diagnostics).
    pub fn dump_projections(&self) -> Result<db::ProjectionDump, StoreError> {
        db::dump_projections(&*self.reader()?)
    }

    fn replay(
        &self,
        conn: &Connection,
        command_id: Uuid,
        kind: &str,
        hash: &str,
    ) -> Result<Option<Outcome>, EngineError> {
        let Some(rec) = db::find_command(conn, &command_id.to_string())? else {
            return Ok(None);
        };
        if rec.kind != kind || rec.request_hash != hash {
            return Err(EngineError::CommandIdReused(command_id));
        }
        let result: CommandResult = serde_json::from_str(&rec.result).map_err(StoreError::from)?;
        Ok(Some(Outcome {
            result,
            replayed: true,
        }))
    }

    /// Validates and applies a command. Retrying the same `command_id` with the same command returns
    /// the original result without storing anything.
    pub fn execute(
        &self,
        actor: &Actor,
        command_id: Uuid,
        command: &Command,
    ) -> Result<Outcome, EngineError> {
        self.execute_cancellable(actor, command_id, command, &Cancel::new())
    }

    /// [`execute`](Self::execute), giving up without storing anything once `cancel` is set (see the
    /// module docs for the exact guarantee).
    pub fn execute_cancellable(
        &self,
        actor: &Actor,
        command_id: Uuid,
        command: &Command,
        cancel: &Cancel,
    ) -> Result<Outcome, EngineError> {
        self.execute_checked(actor, command_id, command, cancel, |_| Ok(()))
    }

    /// [`execute_cancellable`](Self::execute_cancellable) that first runs `check` inside the writer,
    /// against the exact state the idempotency lookup and `decide` then use, so nothing can change
    /// between the check and the write. An error from `check` is returned as is and nothing is stored;
    /// it runs before the idempotency lookup, so it also refuses a retry.
    pub fn execute_checked(
        &self,
        actor: &Actor,
        command_id: Uuid,
        command: &Command,
        cancel: &Cancel,
        check: impl FnOnce(&Locked<'_>) -> Result<(), EngineError>,
    ) -> Result<Outcome, EngineError> {
        let mut w = self.writer(cancel)?;
        let base = self.state();
        self.check_locked(&w, &base, check)?;
        let hash = request_hash("command", actor, command)?;
        if let Some(o) = self.replay(w.conn()?, command_id, "command", &hash)? {
            return Ok(o);
        }
        let now = self.next_now(&mut w);
        let events = decide(&base, actor, command, now)?;
        self.commit(
            &mut w,
            &base,
            Pending {
                actor,
                command_id,
                now,
                kind: "command",
                request_hash: hash,
                cancel,
            },
            events,
            |s, ev| summary::command(s, command, ev),
            |_| Ok(()),
        )
    }

    /// Stores the commands of `plan` as one undoable group, at most once per `key`.
    ///
    /// Inside the single writer: if `key` is already recorded, nothing is decided or stored and the
    /// task recorded with it is returned. Otherwise `plan` builds the commands from the current state
    /// (and the writer connection, for server-side lookups); they are decided in order, each against
    /// the state the previous ones produce. The key is then inserted first in the same transaction as
    /// the events, under a unique constraint, so two writes with one key can never both commit.
    ///
    /// `admit` runs in that transaction right after the key is inserted. The insert has taken the
    /// database's write lock, so `admit` reads every change committed before it (also through other
    /// connections, like the admin pages'), and none can commit until this write has: a check there
    /// cannot go stale before the commit. If it returns `false`, nothing is stored.
    pub fn execute_unique<F, G>(
        &self,
        actor: &Actor,
        key: &str,
        cancel: &Cancel,
        plan: F,
        admit: G,
    ) -> Result<UniqueOutcome, EngineError>
    where
        F: FnOnce(&State, &Connection) -> Result<UniquePlan, EngineError>,
        G: FnOnce(&Connection) -> Result<bool, StoreError>,
    {
        let mut w = self.writer(cancel)?;
        if let Some(task) = db::unique_task(w.conn()?, key)? {
            return Ok(UniqueOutcome::Duplicate { task });
        }
        let now = self.next_now(&mut w);
        let base = self.state();
        let plan = plan(&base, w.conn()?)?;
        let mut sim = (*base).clone();
        let mut events = Vec::new();
        for c in &plan.commands {
            let decided = decide(&sim, actor, c, now)?;
            for e in &decided {
                apply(&mut sim, e).map_err(|err| {
                    StoreError::Corrupt(format!("decided event does not apply: {err}"))
                })?;
            }
            events.extend(decided);
        }
        if let Some(first) = events.first().map(|e| e.id) {
            for e in &mut events {
                e.group = first;
            }
        }
        let command_id = Uuid::new_v4();
        let hash = request_hash("unique", actor, &(key, &plan.commands))?;
        let first = plan.commands.first().cloned();
        let task = plan.task;
        let stored = self.commit(
            &mut w,
            &base,
            Pending {
                actor,
                command_id,
                now,
                kind: "unique",
                request_hash: hash,
                cancel,
            },
            events,
            |s, ev| first.map_or_else(String::new, |c| summary::command(s, &c, ev)),
            |tx| {
                if !db::insert_unique(tx, key, task)? {
                    return Err(EngineError::DuplicateKey);
                }
                if !admit(tx)? {
                    return Err(EngineError::Refused);
                }
                Ok(())
            },
        );
        match stored {
            Ok(o) => Ok(UniqueOutcome::Created {
                task,
                result: o.result,
            }),
            Err(EngineError::DuplicateKey) => {
                let task = db::unique_task(w.conn()?, key)?
                    .ok_or_else(|| StoreError::Corrupt("a taken unique key has no row".into()))?;
                Ok(UniqueOutcome::Duplicate { task })
            }
            Err(EngineError::Refused) => Ok(UniqueOutcome::Refused),
            Err(e) => Err(e),
        }
    }

    /// Undoes the command group that the event `seq` belongs to. Undoing a compensating event redoes
    /// the original change.
    pub fn undo(
        &self,
        actor: &Actor,
        seq: u64,
        force: bool,
        command_id: Option<Uuid>,
    ) -> Result<Outcome, EngineError> {
        self.undo_cancellable(actor, seq, force, command_id, &Cancel::new())
    }

    /// [`undo`](Self::undo), giving up without storing anything once `cancel` is set.
    pub fn undo_cancellable(
        &self,
        actor: &Actor,
        seq: u64,
        force: bool,
        command_id: Option<Uuid>,
        cancel: &Cancel,
    ) -> Result<Outcome, EngineError> {
        self.undo_expecting(actor, seq, force, command_id, None, cancel)
    }

    /// [`undo_cancellable`](Self::undo_cancellable) that first checks, inside the writer, that the
    /// log has the expected epoch and the event at `seq` is the expected one. A retry with the same
    /// `command_id` returns the stored result before checking.
    pub fn undo_expecting(
        &self,
        actor: &Actor,
        seq: u64,
        force: bool,
        command_id: Option<Uuid>,
        expect: Option<UndoExpect>,
        cancel: &Cancel,
    ) -> Result<Outcome, EngineError> {
        self.undo_checked(actor, seq, force, command_id, expect, cancel, |_| Ok(()))
    }

    /// [`undo_expecting`](Self::undo_expecting) that first runs `check` inside the writer, before the
    /// idempotency lookup and the expected-event check (see [`execute_checked`](Self::execute_checked)).
    #[allow(clippy::too_many_arguments)] // `undo_expecting`'s arguments and the check.
    pub fn undo_checked(
        &self,
        actor: &Actor,
        seq: u64,
        force: bool,
        command_id: Option<Uuid>,
        expect: Option<UndoExpect>,
        cancel: &Cancel,
        check: impl FnOnce(&Locked<'_>) -> Result<(), EngineError>,
    ) -> Result<Outcome, EngineError> {
        let mut w = self.writer(cancel)?;
        let base = self.state();
        self.check_locked(&w, &base, check)?;
        let body = UndoRequestBody {
            seq,
            force,
            expected_event_id: expect.map(|e| e.event_id),
        };
        let hash = request_hash("undo", actor, &body)?;
        let command_id = match command_id {
            Some(id) => {
                if let Some(o) = self.replay(w.conn()?, id, "undo", &hash)? {
                    return Ok(o);
                }
                id
            }
            None => Uuid::new_v4(),
        };
        if let Some(UndoExpect {
            epoch: Some(epoch), ..
        }) = expect
            && epoch != self.epoch
        {
            return Err(EngineError::EpochMismatch {
                current: self.epoch,
            });
        }
        let target = db::event_by_seq(w.conn()?, seq)?.ok_or(EngineError::EventNotFound(seq))?;
        if let Some(e) = expect
            && e.event_id != target.id
        {
            return Err(EngineError::UndoTargetMismatch {
                seq,
                expected: e.event_id,
                found: target.id,
            });
        }
        let group = db::events_in_group(w.conn()?, target.group)?;
        let now = self.next_now(&mut w);
        let comp = match undo_batch(&base, &group, actor, now, UndoOptions { force }) {
            Ok(c) => c,
            Err(error) => {
                let seqs = self.seqs_for(w.conn()?, &error)?;
                return Err(EngineError::Undo { error, seqs });
            }
        };
        self.commit(
            &mut w,
            &base,
            Pending {
                actor,
                command_id,
                now,
                kind: "undo",
                request_hash: hash,
                cancel,
            },
            comp,
            summary::undo,
            |_| Ok(()),
        )
    }

    /// Runs a write's `check` against `base`, the state read under the writer `w`.
    fn check_locked(
        &self,
        w: &Writer,
        base: &State,
        check: impl FnOnce(&Locked<'_>) -> Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        // `state` before `whereabouts`, as `whereabouts` asks; under the writer neither changes.
        let gone = self.whereabouts();
        check(&Locked {
            state: base,
            gone: &gone,
            conn: w.conn()?,
        })
    }

    fn seqs_for(
        &self,
        conn: &Connection,
        error: &UndoError,
    ) -> Result<BTreeMap<EventId, u64>, StoreError> {
        let mut ids = BTreeSet::new();
        match error {
            UndoError::Conflict(c) => {
                ids.insert(c.event);
                for f in &c.conflicts {
                    ids.extend(f.later_events.iter().copied());
                }
            }
            UndoError::UnknownEvent(e)
            | UndoError::AlreadyUndone(e)
            | UndoError::EventMismatch(e) => {
                ids.insert(*e);
            }
            _ => {}
        }
        let mut out = BTreeMap::new();
        for id in ids {
            if let Some(seq) = db::seq_of(conn, id)? {
                out.insert(id, seq);
            }
        }
        Ok(out)
    }

    /// A timestamp at least one millisecond after the previous one handed out. Event IDs are derived
    /// from the time, so this keeps two identical commands in the same millisecond from colliding.
    fn next_now(&self, w: &mut Writer) -> Timestamp {
        let now = self.now();
        let ms = millis(now).max(w.last_ms + 1);
        w.last_ms = ms;
        Timestamp::from_millisecond(ms).unwrap_or(now)
    }

    /// Stores `events`: applies them to a copy of `base`, writes events, projections and the
    /// idempotency record in one transaction, and swaps the copy in only after `COMMIT` succeeded.
    /// `first` runs at the start of the transaction; an error from it rolls everything back.
    fn commit(
        &self,
        w: &mut Writer,
        base: &State,
        p: Pending<'_>,
        mut events: Vec<Event>,
        summarize: impl FnOnce(&State, &[Event]) -> String,
        first: impl FnOnce(&Transaction<'_>) -> Result<(), EngineError>,
    ) -> Result<Outcome, EngineError> {
        let mut next = base.clone();
        let mut scopes = Vec::with_capacity(events.len());
        let mut moves = Vec::new();
        let command_id = p.command_id;
        let cid = command_id.to_string();
        let head_before = self.head_seq();
        let stored = (|| -> Result<CommandResult, EngineError> {
            let tx = w
                .conn
                .as_mut()
                .ok_or(EngineError::Unavailable)?
                .transaction()
                .map_err(StoreError::from)?;
            first(&tx)?;
            let mut touched = BTreeSet::<EntityId>::new();
            for (i, e) in events.iter_mut().enumerate() {
                let seq = db::insert_event(&tx, e, &cid, i)?;
                e.seq = Some(seq);
                let mut scope = ScopeBuilder::default();
                scope.observe(&next, &e.payload);
                let target = e.payload.target();
                let before = location(&next, target);
                apply(&mut next, e).map_err(|err| {
                    StoreError::Corrupt(format!("decided event does not apply: {err}"))
                })?;
                if let Some(change) = whereabouts_change(before, &next, target) {
                    moves.push((target, change));
                }
                scope.observe(&next, &e.payload);
                let scope = scope.finish();
                db::insert_scope(&tx, seq, &scope)?;
                scopes.push(scope);
                touched.insert(e.payload.target());
            }
            let check = self
                .commit_check
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(check) = check {
                check(base, &next)?;
            }
            db::sync_entities(&tx, &next, touched.iter().copied())?;
            let head = events.last().and_then(|e| e.seq).unwrap_or(head_before);
            db::set_projection_seq(&tx, head)?;
            let result = CommandResult {
                command_id: p.command_id,
                events: events
                    .iter()
                    .filter_map(|e| e.seq.map(|seq| EventRef { seq, id: e.id }))
                    .collect(),
                summary: summarize(&next, &events),
            };
            let record = CommandRecord {
                kind: p.kind.to_owned(),
                request_hash: p.request_hash,
                result: serde_json::to_string(&result).map_err(StoreError::from)?,
            };
            db::insert_command(&tx, &cid, p.actor, p.now, &record)?;
            // Claim the commit; if the request was cancelled or is past its deadline, dropping `tx`
            // rolls everything back.
            if !p.cancel.begin_commit() {
                return Err(EngineError::Cancelled);
            }
            #[cfg(feature = "test-hooks")]
            self.test_hooks_before_commit();
            tx.commit().map_err(StoreError::from)?;
            Ok(result)
        })();
        let result = match stored {
            Ok(r) => r,
            Err(e) => {
                if w.conn.as_ref().is_some_and(|c| !c.is_autocommit()) {
                    // The transaction could not be rolled back; the connection's view of the database
                    // is unknown. Refuse all further work until a restart re-reads the log.
                    self.poisoned.store(true, Ordering::Release);
                    tracing::error!(error = %e, "write failed and could not be rolled back; engine poisoned");
                } else if !matches!(
                    e,
                    EngineError::Cancelled
                        | EngineError::DuplicateKey
                        | EngineError::Refused
                        | EngineError::ContentPurging(_)
                ) {
                    tracing::error!(error = %e, "write failed; nothing was stored");
                }
                return Err(e);
            }
        };
        if !moves.is_empty() {
            let mut gone = self.gone.write().unwrap_or_else(PoisonError::into_inner);
            let changes_anything = moves.iter().any(|(id, change)| match change {
                Some(_) => true,
                None => gone.contains_key(id),
            });
            if changes_anything {
                let mut next_gone = (**gone).clone();
                for (id, change) in moves {
                    record_whereabouts(&mut next_gone, id, Some(change));
                }
                *gone = Arc::new(next_gone);
            }
        }
        *self.state.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
        if let Some(last) = result.events.last() {
            self.head.store(last.seq, Ordering::Release);
        }
        let command_len = Some(u32::try_from(events.len()).unwrap_or(u32::MAX));
        for (i, (event, scope)) in events.into_iter().zip(scopes).enumerate() {
            // No receivers is fine.
            let event = LogEvent {
                event,
                command_id,
                command_idx: u32::try_from(i).unwrap_or(u32::MAX),
                command_len,
            };
            let _ = self.live.send(Arc::new(LiveEvent { event, scope }));
        }
        Ok(Outcome {
            result,
            replayed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use todo_core::ids::ListId;

    fn open() -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(&dir.path().join("db.sqlite3"), system_clock()).unwrap();
        (dir, engine)
    }

    fn create_list() -> Command {
        serde_json::from_value(serde_json::json!({
            "cmd": "create_list", "id": ListId::from_uuid(Uuid::new_v4()), "name": "L"
        }))
        .unwrap()
    }

    fn actor() -> Actor {
        Actor::owner("pc").unwrap()
    }

    #[test]
    fn a_cancelled_write_stores_nothing() {
        let (_dir, engine) = open();
        let cancel = Cancel::new();
        cancel.cancel();
        let r = engine.execute_cancellable(&actor(), Uuid::new_v4(), &create_list(), &cancel);
        assert!(matches!(r, Err(EngineError::Cancelled)), "{r:?}");
        assert_eq!(engine.head_seq(), 0);
        assert!(engine.state().lists.is_empty());
    }

    #[test]
    fn a_write_cancelled_while_waiting_for_the_writer_stores_nothing() {
        let (_dir, engine) = open();
        let engine = Arc::new(engine);
        let cancel = Cancel::new();
        let hold = engine.hold_writer();
        let worker = {
            let (engine, cancel) = (engine.clone(), cancel.clone());
            std::thread::spawn(move || {
                engine.execute_cancellable(&actor(), Uuid::new_v4(), &create_list(), &cancel)
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        // The handler gives up (its future is dropped) while the write waits.
        drop(cancel.on_drop());
        drop(hold);
        let r = worker.join().unwrap();
        assert!(matches!(r, Err(EngineError::Cancelled)), "{r:?}");
        assert_eq!(engine.head_seq(), 0);
        // A disarmed guard does not cancel.
        let ok = Cancel::new();
        ok.on_drop().disarm();
        engine
            .execute_cancellable(&actor(), Uuid::new_v4(), &create_list(), &ok)
            .unwrap();
        assert_eq!(engine.head_seq(), 1);
    }

    #[test]
    fn a_write_that_cannot_roll_back_poisons_the_engine() {
        let (_dir, engine) = open();
        engine
            .execute(&actor(), Uuid::new_v4(), &create_list())
            .unwrap();
        let before = engine.state();
        // Simulate a connection stuck inside a transaction (a failed rollback): the next write cannot
        // start its own transaction, and the connection is not back in autocommit mode afterwards.
        lock(&engine.writer)
            .conn
            .as_ref()
            .unwrap()
            .execute_batch("BEGIN")
            .unwrap();
        let r = engine.execute(&actor(), Uuid::new_v4(), &create_list());
        assert!(matches!(r, Err(EngineError::Store(_))), "{r:?}");
        assert!(!engine.is_healthy());
        assert_eq!(*engine.state(), *before, "no uncommitted state is visible");
        let r = engine.execute(&actor(), Uuid::new_v4(), &create_list());
        assert!(matches!(r, Err(EngineError::Unavailable)), "{r:?}");
    }

    #[test]
    fn close_refuses_writes_and_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite3");
        let engine = Engine::open(&path, system_clock()).unwrap();
        assert!(matches!(
            Engine::open(&path, system_clock()),
            Err(StoreError::Locked { .. })
        ));
        engine.close();
        let r = engine.execute(&actor(), Uuid::new_v4(), &create_list());
        assert!(matches!(r, Err(EngineError::Unavailable)), "{r:?}");
        let second = Engine::open(&path, system_clock()).unwrap();
        second
            .execute(&actor(), Uuid::new_v4(), &create_list())
            .unwrap();
    }
}
