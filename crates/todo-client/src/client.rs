//! The sync client: a [`Replica`] driven by two loops, one sending the outbox and one following the
//! server's event stream.
//!
//! The client spawns nothing. The app drives [`SyncClient::run`] on its own executor (for example
//! `tokio::spawn(async move { client.run().await })` on the desktop) and calls [`SyncClient::stop`] to
//! end it. Every other method is synchronous and cheap, so a UI thread can call it directly.
//!
//! # Sending
//!
//! Outbox entries go out in order, one at a time. A network error, a timeout, 408, 429 or any 5xx
//! (including 503 `outcome_unknown`) is retried with the **same** command ID after a jittered,
//! exponential backoff; the server is idempotent by command ID, so a command is never lost and never
//! applied twice. A definitive rejection moves the entry to the "couldn't sync" list
//! ([`SyncClient::failed`]), which the app shows until the user dismisses it.
//!
//! # Following the log
//!
//! The event loop subscribes to `GET /api/v1/events` (SSE) from the persisted cursor and log epoch and
//! applies each event to the confirmed state and to storage. It handles the server's catch-up
//! protocol: a `catch_up` frame or a 409 `catch_up_required` makes it page the JSON endpoint up to the
//! head and reconnect. Lost connections are retried with backoff from the cursor. A stream that shows
//! no sign of life (no event and no heartbeat) for [`ClientConfig::stream_idle_timeout`] is dropped
//! and reopened; a frame larger than the transport's cap ends it, and the events are paged instead.
//!
//! # Resyncing
//!
//! The confirmed state is forgotten and the whole log fetched again, keeping the outbox, when the
//! server's log is not the one the replica followed: the server reports another log epoch (409
//! `epoch_mismatch`, or a page or `hello` frame with another epoch; a restore rotates it), the cursor
//! is ahead of the server (400 `invalid_since`), an event does not apply, or an acknowledgement names
//! an event the confirmed state has another event for. If the same event fails to apply again right
//! after a resync, the client reports a protocol error and retries with backoff instead. A reset that
//! cannot be stored is retried (with backoff) before anything else.
//!
//! Acknowledgements are bound to the log epoch the server names in its response: one from another
//! epoch is not recorded, and a resync forgets the recorded ones, so their entries are sent again
//! (idempotently) to the log the client now follows.

use crate::activity::{self, ActivityError, ActivityPage, ActivityQuery};
use crate::notify::{Backoff, Notify, sleep_or_wake, timeout};
use crate::outbox::{CommandId, Epoch, FailedEntry, Operation, OutboxEntry, Rejection};
use crate::platform::{
    Clock, Entropy, EventStream, Response, SseFrame, StreamOpen, Transport, TransportError,
};
use crate::protocol::{
    self, CATCH_UP_FRAME, CatchUpNotice, EPOCH_MISMATCH, EVENT_FRAME, EVENT_MISMATCH, EventPage,
    HELLO_FRAME, Hello, LogEvent, SendOutcome, error_detail,
};
use crate::replica::{
    Ack, ConfirmError, OpenError, Replica, SubmitError, TrackedCommand, TrackedUndo,
};
use crate::storage::{Storage, StorageError};
use futures_util::future::{Either, join, select};
use std::fmt;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use todo_core::ids::{EventId, uuid_v7};
use todo_core::time::Timestamp;
use todo_core::{Actor, Command, State};
use uuid::Uuid;

/// Settings of a [`SyncClient`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientConfig {
    /// The actor the server knows this client's token as (the owner on this device). Pending commands
    /// are decided as this actor for the optimistic view.
    pub actor: Actor,
    /// Deadline for one request, and for the response head of the event stream.
    pub request_timeout: Duration,
    /// An open stream with no frame (the server sends a heartbeat every 25 s by default) for this long
    /// is considered dead.
    pub stream_idle_timeout: Duration,
    /// Retry delays.
    pub backoff: Backoff,
    /// Page size for catching up through `GET /api/v1/events`.
    pub page_limit: usize,
    /// Confirmed events stored between snapshots of the confirmed state.
    pub snapshot_every: usize,
}

impl ClientConfig {
    /// Defaults for `actor`.
    #[must_use]
    pub fn new(actor: Actor) -> Self {
        Self {
            actor,
            request_timeout: Duration::from_secs(20),
            stream_idle_timeout: Duration::from_secs(60),
            backoff: Backoff::default(),
            page_limit: 500,
            snapshot_every: crate::replica::SNAPSHOT_EVERY,
        }
    }
}

/// Connectivity as the UI shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    /// The server cannot be reached (or the client is not running).
    Offline,
    /// Connected, but catching up or sending pending changes (or the server is refusing requests for
    /// now).
    Syncing,
    /// Following the live event stream with nothing left to send.
    Online,
}

/// How far the replica has come since the client started: whether what it shows includes what the
/// server had when this client first reached it. Unlike [`Connection::Online`] (which the client
/// reports as soon as the stream opens, before its `hello` frame and backlog), this says the backlog
/// is in: a client acting on the replica on its own (reminders) can wait for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// No attempt to reach the server has ended yet (or a full resync started over).
    Starting,
    /// An attempt failed (offline, refused) before the replica caught up.
    Failed,
    /// The replica has every event up to the server's head when it was reached: the `hello` frame's
    /// `head_seq`, or the end of a catch-up. It stays so until a full resync.
    CaughtUp,
}

/// What kind of problem [`Status::last_error`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// No connection, or no response in time.
    Network,
    /// The server rejected the token (401). Needs re-pairing.
    Unauthorized,
    /// The server answered with an error that is not about a particular command (5xx, 429, ...).
    Server,
    /// The server's answer was not understood.
    Protocol,
    /// Local storage failed.
    Storage,
}

/// A problem the client ran into. The message never contains task content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncError {
    /// The kind.
    pub kind: ErrorKind,
    /// Description.
    pub message: String,
    /// When it happened.
    pub at: Timestamp,
}

/// A snapshot of the client's sync status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// Online, syncing or offline.
    pub connection: Connection,
    /// Entries in the outbox (not yet confirmed).
    pub pending: usize,
    /// Entries in the "couldn't sync" list.
    pub failed: usize,
    /// The last confirmed sequence number.
    pub cursor: u64,
    /// The most recent problem; cleared once the client is online again.
    pub last_error: Option<SyncError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamState {
    /// `run` has not started (or has ended).
    Idle,
    Connecting,
    CatchingUp,
    Live,
    Down {
        offline: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SendState {
    Ok,
    Failing { offline: bool },
}

struct Shared {
    replica: Replica,
    stream: StreamState,
    send: SendState,
    last_error: Option<SyncError>,
    /// Submission time of the newest entry, to keep them strictly increasing.
    last_submitted_ms: i64,
    /// The send loop found that the server's log differs from the confirmed state.
    resync_requested: bool,
    readiness: Readiness,
}

impl Shared {
    fn enqueue(
        &mut self,
        op: Operation,
        command_id: CommandId,
        now: Timestamp,
    ) -> Result<(), SubmitError> {
        // Strictly increasing submission times keep identical commands in one millisecond from
        // deciding to identical events in the view.
        let ms = now.as_millisecond().max(self.last_submitted_ms + 1);
        self.replica.submit(OutboxEntry {
            command_id,
            op,
            submitted_at: Timestamp::from_millisecond(ms).unwrap_or(now),
            attempts: 0,
            acked: None,
            acked_epoch: None,
        })?;
        self.last_submitted_ms = ms;
        Ok(())
    }

    fn pending_undo(&self, event: EventId) -> Option<CommandId> {
        self.replica
            .outbox()
            .iter()
            .find_map(|entry| (entry.undo_target() == Some(event)).then_some(entry.command_id))
    }

    fn connection(&self) -> Connection {
        let offline = matches!(
            self.stream,
            StreamState::Idle | StreamState::Down { offline: true }
        ) || self.send == SendState::Failing { offline: true };
        if offline {
            Connection::Offline
        } else if self.stream == StreamState::Live
            && self.send == SendState::Ok
            && self.replica.outbox().is_empty()
        {
            Connection::Online
        } else {
            Connection::Syncing
        }
    }

    fn status(&self) -> Status {
        Status {
            connection: self.connection(),
            pending: self.replica.outbox().len(),
            failed: self.replica.failed().len(),
            cursor: self.replica.cursor(),
            last_error: self.last_error.clone(),
        }
    }

    fn settle(&mut self) {
        if self.connection() == Connection::Online {
            self.last_error = None;
        }
    }
}

struct Inner<T, C> {
    transport: T,
    clock: C,
    entropy: Box<dyn Entropy>,
    config: ClientConfig,
    shared: Mutex<Shared>,
    /// Bumped when there may be something new to send.
    outbox_changed: Notify,
    /// The storage's [`Storage::saved`]: bumped when held-back entries became durable.
    saved: Option<Notify>,
    /// False while another tab sends for this one (see [`SyncClient::set_sending`]).
    sending: AtomicBool,
    /// Bumped by [`SyncClient::nudge`]: cut retry delays short.
    wake: Notify,
    /// Bumped once by [`SyncClient::stop`].
    stop: Notify,
    /// Bumped when [`Shared::resync_requested`] is set.
    resync: Notify,
    /// Bumped after every resync (the confirmed state was forgotten).
    resynced: Notify,
    stopped: AtomicBool,
    /// Bumped whenever the view or the status may have changed.
    changes: Notify,
}

/// The sync engine. Cheap to clone; clones share everything.
pub struct SyncClient<T, C> {
    inner: Arc<Inner<T, C>>,
}

impl<T, C> Clone for SyncClient<T, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T, C> fmt::Debug for SyncClient<T, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncClient")
            .field("actor", &self.inner.config.actor)
            .finish_non_exhaustive()
    }
}

/// Waits for changes of a client's view or status (see [`SyncClient::changes`]).
#[derive(Debug)]
pub struct Changes {
    notify: Notify,
    seen: u64,
}

impl Changes {
    /// Completes at the next change after the previous call (or after [`SyncClient::changes`]).
    pub async fn next(&mut self) {
        self.seen = self.notify.changed(self.seen).await;
    }
}

/// What a step of the event loop asks for next.
enum Step {
    /// Reconnect at once (progress was made).
    Reconnect,
    /// Forget the confirmed state and fetch the whole log again (keeping the outbox).
    Resync {
        /// The event that did not apply, if that is why.
        diverged_at: Option<u64>,
    },
    /// Back off, then reconnect.
    Fail {
        error: SyncError,
        offline: bool,
        retry_after: Option<Duration>,
        /// The stream delivered something before failing, so the backoff starts over.
        progressed: bool,
    },
    /// The client was stopped.
    Stopped,
}

enum SendResult {
    Accepted(protocol::CommandResult),
    Rejected(Rejection),
    Retry {
        error: SyncError,
        offline: bool,
        after: Option<Duration>,
    },
}

fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<T: Transport, C: Clock> SyncClient<T, C> {
    /// Loads the replica from `storage`. Call [`run`](Self::run) to start syncing.
    pub fn new(
        transport: T,
        clock: C,
        entropy: impl Entropy,
        storage: impl Storage + 'static,
        config: ClientConfig,
    ) -> Result<Self, OpenError> {
        let saved = storage.saved();
        let mut replica = Replica::open(config.actor.clone(), Box::new(storage))?;
        replica.set_snapshot_every(config.snapshot_every);
        let last_submitted_ms = replica
            .outbox()
            .iter()
            .map(|e| e.submitted_at.as_millisecond())
            .max()
            .unwrap_or(0);
        Ok(Self {
            inner: Arc::new(Inner {
                transport,
                clock,
                entropy: Box::new(entropy),
                config,
                shared: Mutex::new(Shared {
                    replica,
                    stream: StreamState::Idle,
                    send: SendState::Ok,
                    last_error: None,
                    last_submitted_ms,
                    resync_requested: false,
                    readiness: Readiness::Starting,
                }),
                outbox_changed: Notify::default(),
                saved,
                sending: AtomicBool::new(true),
                wake: Notify::default(),
                stop: Notify::default(),
                resync: Notify::default(),
                resynced: Notify::default(),
                stopped: AtomicBool::new(false),
                changes: Notify::default(),
            }),
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let mut g = lock(&self.inner.shared);
        f(&mut g)
    }

    fn changed(&self) {
        self.with(Shared::settle);
        self.inner.changes.notify();
    }

    fn random_u64(&self) -> u64 {
        let mut b = [0u8; 8];
        self.inner.entropy.fill(&mut b);
        u64::from_le_bytes(b)
    }

    fn error(&self, kind: ErrorKind, message: impl Into<String>) -> SyncError {
        SyncError {
            kind,
            message: message.into(),
            at: self.inner.clock.now(),
        }
    }

    /// A fresh UUIDv7, for the IDs of entities a command creates (`TaskId::from_uuid(client.new_uuid())`).
    #[must_use]
    pub fn new_uuid(&self) -> Uuid {
        let ms = u64::try_from(self.inner.clock.now().as_millisecond()).unwrap_or(0);
        let mut rand = [0u8; 10];
        self.inner.entropy.fill(&mut rand);
        uuid_v7(ms, rand)
    }

    fn enqueue<R>(
        &self,
        op: Operation,
        result: impl FnOnce(&mut Replica, CommandId) -> R,
    ) -> Result<R, SubmitError> {
        let command_id = self.new_uuid();
        let now = self.inner.clock.now();
        let result = self.with(|s| {
            s.enqueue(op, command_id, now)?;
            Ok::<_, SubmitError>(result(&mut s.replica, command_id))
        })?;
        self.inner.outbox_changed.notify();
        self.changed();
        Ok(result)
    }

    /// Queues a command and applies it to the view at once.
    ///
    /// Fails without queueing if the command is invalid against the current view, or if the outbox
    /// cannot be written.
    pub fn submit(&self, command: Command) -> Result<CommandId, SubmitError> {
        self.enqueue(Operation::Command { command }, |_, id| id)
    }

    /// Submits a command for which the UI offers Undo, but only if `guard` holds on the view at the
    /// moment it is queued, under the same lock that applies synced events: nothing can change the
    /// view between the check and the command. `Ok(None)` when the guard refused (nothing queued).
    pub fn submit_tracked_if(
        &self,
        command: Command,
        guard: impl FnOnce(&State) -> bool,
    ) -> Result<Option<TrackedCommand>, SubmitError> {
        let command_id = self.new_uuid();
        let now = self.inner.clock.now();
        let tracked = self.with(|s| {
            if !guard(&s.replica.view()) {
                return Ok(None);
            }
            s.enqueue(Operation::Command { command }, command_id, now)?;
            Ok::<_, SubmitError>(Some(s.replica.track(command_id)))
        })?;
        if tracked.is_some() {
            self.inner.outbox_changed.notify();
            self.changed();
        }
        Ok(tracked)
    }

    /// Submits a command for which the UI offers Undo. The returned handle correlates the command
    /// with the authoritative log, not with similar commands from other devices. For an outbox
    /// shared by multiple tabs, undo it with [`Self::compensate_tracked`].
    pub fn submit_tracked(&self, command: Command) -> Result<TrackedCommand, SubmitError> {
        self.enqueue(Operation::Command { command }, Replica::track)
    }

    /// Cancels an unsent command or durably queues a non-forced undo of this exact command.
    /// If its outcome is still unknown, the undo waits in the outbox for an authoritative receipt.
    /// The view shows the predicted inverse while the undo waits, also after a restart (see the
    /// replica's module docs for when it can't).
    pub fn undo_tracked(&self, target: &TrackedCommand) -> Result<TrackedUndo, SubmitError> {
        self.request_tracked_undo(target, true)
    }

    /// Queues a checked undo of the exact command without ever cancelling it locally. Use this
    /// for shared outboxes: another tab may have already picked up the original command even when
    /// this replica hasn't seen a send attempt.
    pub fn compensate_tracked(&self, target: &TrackedCommand) -> Result<TrackedUndo, SubmitError> {
        self.request_tracked_undo(target, false)
    }

    fn request_tracked_undo(
        &self,
        target: &TrackedCommand,
        allow_cancel: bool,
    ) -> Result<TrackedUndo, SubmitError> {
        let command_id = self.new_uuid();
        let now = self.inner.clock.now();
        let result = self.with(|s| {
            let ms = now.as_millisecond().max(s.last_submitted_ms + 1);
            let result = s
                .replica
                .undo_tracked(target, allow_cancel, |op| OutboxEntry {
                    command_id,
                    op,
                    submitted_at: Timestamp::from_millisecond(ms).unwrap_or(now),
                    attempts: 0,
                    acked: None,
                    acked_epoch: None,
                })?;
            if matches!(result, TrackedUndo::Queued(_)) {
                s.last_submitted_ms = ms;
            }
            Ok::<_, SubmitError>(result)
        })?;
        if matches!(result, TrackedUndo::Cancelled | TrackedUndo::Queued(_)) {
            self.inner.outbox_changed.notify();
            self.changed();
        }
        Ok(result)
    }

    /// Queues an undo of the command that produced event `seq` (or a redo, if `seq` is a compensating
    /// event). `expected_event_id` is the ID of the event at `seq` the user means (from the activity
    /// feed or the confirmed log); with the current log epoch, it lets the server refuse the undo if its
    /// log was replaced meanwhile, and the entry then lands in the "couldn't sync" list instead of
    /// undoing some other event. If the original's batch isn't available locally, the inverse
    /// shows in the view once the server confirms it.
    pub fn undo(
        &self,
        seq: u64,
        expected_event_id: EventId,
        force: bool,
    ) -> Result<CommandId, SubmitError> {
        let epoch = self.with(|s| s.replica.epoch());
        self.enqueue(
            Operation::Undo {
                seq,
                force,
                expected_event_id,
                epoch,
            },
            |_, id| id,
        )
    }

    /// Reads a page of confirmed history through the same transport as sync. The caller supplies
    /// the next cursor (and the same agent filter) to read older activity.
    pub async fn activity(&self, query: &ActivityQuery) -> Result<ActivityPage, ActivityError> {
        let response = timeout(
            &self.inner.clock,
            self.inner.config.request_timeout,
            self.inner.transport.send(query.request()),
        )
        .await
        .and_then(Result::ok)
        .ok_or(ActivityError::Unavailable)?;
        activity::read_response(&response)
    }

    /// Requests a non-forced compensation for the command that produced a confirmed feed event.
    /// Unlike `compensate_tracked`, this doesn't need a local submission handle: the event already
    /// exists in the server's log. The durable outbox still sends the checked undo, not a local
    /// cancellation, and the server compensates the entire original batch. If the outbox already
    /// holds an undo for this event, returns its command ID without queueing another.
    pub fn undo_activity(&self, event: protocol::EventRef) -> Result<CommandId, SubmitError> {
        let command_id = self.new_uuid();
        let now = self.inner.clock.now();
        let existing = self.with(|s| {
            if let Some(existing) = s.pending_undo(event.id) {
                return Ok(Some(existing));
            }
            s.enqueue(
                Operation::Undo {
                    seq: event.seq,
                    force: false,
                    expected_event_id: event.id,
                    epoch: s.replica.epoch(),
                },
                command_id,
                now,
            )?;
            Ok::<_, SubmitError>(None)
        })?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        self.inner.outbox_changed.notify();
        self.changed();
        Ok(command_id)
    }

    /// The queued undo for this exact event, including one recovered from storage or adopted.
    #[must_use]
    pub fn pending_undo(&self, event: EventId) -> Option<CommandId> {
        self.with(|s| s.pending_undo(event))
    }

    /// Pending (`None`) and refused (plain wording) feed undos by exact event ID. Read both
    /// lists under one lock without cloning unrelated queued commands or their payloads.
    #[must_use]
    pub fn activity_undos(&self) -> std::collections::BTreeMap<EventId, Option<&'static str>> {
        self.with(|s| {
            let refused = s.replica.failed().iter().filter_map(|f| {
                Some((
                    f.entry.undo_target()?,
                    Some(crate::failure::undo_message(&f.rejection.code)),
                ))
            });
            let pending = s
                .replica
                .outbox()
                .iter()
                .filter_map(|e| Some((e.undo_target()?, None)));
            refused.chain(pending).collect()
        })
    }

    /// Revision of the confirmed log. A cursor advances within an epoch; only an epoch change
    /// invalidates the sequence numbers of previously read activity pages.
    #[must_use]
    pub fn activity_revision(&self) -> (Option<Epoch>, u64) {
        self.with(|s| (s.replica.epoch(), s.replica.cursor()))
    }

    /// What this client knows of a queued undo (or any command), from the outbox and the
    /// "couldn't sync" list alone:
    ///
    /// - `None` while the command is still in the outbox;
    /// - `Some(Err(_))` while it is in the "couldn't sync" list, with the refusal in plain words
    ///   ([`crate::failure::undo_message`], never the server's own text);
    /// - `Some(Ok(()))` otherwise. That includes a command the server took, but also one this client
    ///   never queued and a refusal that has since been dismissed: it is not proof that the server
    ///   accepted the command.
    #[must_use]
    pub fn undo_result(&self, command: CommandId) -> Option<Result<(), &'static str>> {
        self.with(|s| {
            if s.replica.outbox().iter().any(|e| e.command_id == command) {
                return None;
            }
            Some(
                (s.replica.failed().iter())
                    .find(|f| f.entry.command_id == command)
                    .map_or(Ok(()), |f| {
                        Err(crate::failure::undo_message(&f.rejection.code))
                    }),
            )
        })
    }

    /// The confirmed journal's view of a feed event: see [`activity::confirmed`].
    #[must_use]
    pub fn activity_confirmed(&self, event: EventId, compensating: bool) -> Option<(bool, bool)> {
        self.with_confirmed(|state| activity::confirmed(&state.journal, event, compensating))
    }

    /// The log epoch the confirmed state belongs to.
    #[must_use]
    pub fn epoch(&self) -> Option<Epoch> {
        self.with(|s| s.replica.epoch())
    }

    /// The confirmed state with pending commands applied.
    #[must_use]
    pub fn view(&self) -> Arc<State> {
        self.with(|s| s.replica.view())
    }

    /// How far the replica has come since starting ([`Readiness`]).
    #[must_use]
    pub fn readiness(&self) -> Readiness {
        self.with(|s| s.readiness)
    }

    /// The view and the [`Readiness`] it was read with, together.
    pub fn view_and_readiness(&self) -> (Arc<State>, Readiness) {
        self.with(|s| (s.replica.view(), s.readiness))
    }

    /// The replica has caught up with the server's head as it was reached.
    fn reached_head(&self) {
        let changed = self.with(|s| std::mem::replace(&mut s.readiness, Readiness::CaughtUp))
            != Readiness::CaughtUp;
        if changed {
            self.changed();
        }
    }

    /// Runs `f` on the confirmed state (the server's state as of [`cursor`](Self::cursor)).
    pub fn with_confirmed<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        self.with(|s| f(s.replica.confirmed()))
    }

    /// The last confirmed sequence number.
    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.with(|s| s.replica.cursor())
    }

    /// Where fetching continues (see [`Replica::received`]).
    fn received(&self) -> u64 {
        self.with(|s| s.replica.received())
    }

    /// The same paired transport for direct transfers that do not go into the outbox.
    pub fn transport(&self) -> &T {
        &self.inner.transport
    }

    /// The current status.
    #[must_use]
    pub fn status(&self) -> Status {
        self.with(|s| s.status())
    }

    /// The outbox, oldest first.
    #[must_use]
    pub fn pending(&self) -> Vec<OutboxEntry> {
        self.with(|s| s.replica.outbox().to_vec())
    }

    /// The "couldn't sync" list, oldest first.
    #[must_use]
    pub fn failed(&self) -> Vec<FailedEntry> {
        self.with(|s| s.replica.failed().to_vec())
    }

    /// Removes an entry from the "couldn't sync" list. Returns false if there was none.
    pub fn dismiss(&self, command_id: CommandId) -> Result<bool, StorageError> {
        let removed = self.with(|s| s.replica.dismiss(command_id))?;
        if removed {
            self.changed();
        }
        Ok(removed)
    }

    /// Removes the given entries from the "couldn't sync" list at once (the ones the app showed; one
    /// refused since stays). Returns how many were removed.
    pub fn dismiss_many(&self, command_ids: &[CommandId]) -> Result<usize, StorageError> {
        let removed = self.with(|s| s.replica.dismiss_many(command_ids))?;
        if removed > 0 {
            self.changed();
        }
        Ok(removed)
    }

    /// A handle that waits for changes of the view or the status.
    #[must_use]
    pub fn changes(&self) -> Changes {
        Changes {
            notify: self.inner.changes.clone(),
            seen: self.inner.changes.generation(),
        }
    }

    /// Whether this client sends its outbox (the default). In the browser, only one tab sends for all
    /// of them, so commands from every tab reach the server in submission order; the others queue
    /// their commands (in shared storage) and keep following the event stream, which confirms them.
    /// The sending tab takes the others' entries in with [`adopt`](Self::adopt).
    pub fn set_sending(&self, sending: bool) {
        self.inner.sending.store(sending, Ordering::SeqCst);
        self.inner.outbox_changed.notify();
        self.changed();
    }

    /// True if this client sends its outbox (see [`set_sending`](Self::set_sending)).
    #[must_use]
    pub fn is_sending(&self) -> bool {
        self.inner.sending.load(Ordering::SeqCst)
    }

    /// Takes in outbox entries another tab queued in shared storage, in submission order behind
    /// anything already sent (see [`Replica::adopt`]). Returns how many were new.
    pub fn adopt(&self, entries: Vec<OutboxEntry>) -> Result<usize, StorageError> {
        let added = self.with(|s| {
            let added = s.replica.adopt(entries)?;
            if let Some(ms) = s
                .replica
                .outbox()
                .iter()
                .map(|e| e.submitted_at.as_millisecond())
                .max()
            {
                s.last_submitted_ms = s.last_submitted_ms.max(ms);
            }
            Ok::<_, StorageError>(added)
        })?;
        if added > 0 {
            self.inner.outbox_changed.notify();
            self.changed();
        }
        Ok(added)
    }

    /// Takes in entries another tab moved to the "couldn't sync" list (see [`Replica::adopt_failed`]).
    pub fn adopt_failed(&self, failed: Vec<FailedEntry>) -> Result<usize, StorageError> {
        let added = self.with(|s| s.replica.adopt_failed(failed))?;
        if added > 0 {
            self.changed();
        }
        Ok(added)
    }

    /// Retries now instead of waiting out the backoff (call it when the network comes back).
    pub fn nudge(&self) {
        self.inner.wake.notify();
    }

    /// Ends [`run`](Self::run). In-flight requests are abandoned; the server's idempotency makes that
    /// safe.
    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        self.inner.stop.notify();
    }

    fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    /// Syncs until [`stop`](Self::stop) is called.
    pub async fn run(&self) {
        if self.is_stopped() {
            return;
        }
        self.with(|s| s.stream = StreamState::Connecting);
        self.changed();
        join(pin!(self.send_loop()), pin!(self.event_loop())).await;
        self.with(|s| s.stream = StreamState::Idle);
        self.changed();
    }

    /// Runs `fut` unless the client is stopped first.
    async fn unless_stopped<F>(&self, fut: F) -> Option<F::Output>
    where
        F: Future + crate::platform::MaybeSend,
    {
        if self.is_stopped() {
            return None;
        }
        let stopped = pin!(self.inner.stop.changed(0));
        match select(pin!(fut), stopped).await {
            Either::Left((v, _)) => Some(v),
            Either::Right(_) => None,
        }
    }

    /// Sleeps out a retry delay; a nudge or a stop cuts it short.
    async fn back_off(&self, failures: &mut u32, retry_after: Option<Duration>) {
        let delay = self
            .inner
            .config
            .backoff
            .delay(*failures, self.random_u64(), retry_after);
        *failures = failures.saturating_add(1);
        let seen = self.inner.wake.generation();
        let _ = self
            .unless_stopped(sleep_or_wake(
                &self.inner.clock,
                delay,
                &self.inner.wake,
                seen,
            ))
            .await;
    }

    fn storage_failed(&self, what: &str, err: &StorageError) {
        tracing::warn!(error = %err, "cannot {what}");
        let error = self.error(ErrorKind::Storage, format!("cannot {what}: {err}"));
        self.with(|s| s.last_error = Some(error));
        self.changed();
    }

    /// Asks the event loop to resync and waits until it did.
    async fn await_resync(&self) {
        let seen = self.inner.resynced.generation();
        self.with(|s| s.resync_requested = true);
        self.inner.resync.notify();
        self.changed();
        let _ = self.unless_stopped(self.inner.resynced.changed(seen)).await;
    }

    async fn send_loop(&self) {
        // Reset only after durable progress: a storage failure backs off like a network failure.
        let mut failures = 0u32;
        loop {
            if self.is_stopped() {
                return;
            }
            let seen = self.inner.outbox_changed.generation();
            let saved_seen = self.inner.saved.as_ref().map(Notify::generation);
            let next = if self.is_sending() {
                let (next, failed_undo) = self.with(|s| {
                    let before = s.replica.failed().len();
                    let next = s.replica.next_to_send();
                    (next, before != s.replica.failed().len())
                });
                if failed_undo {
                    self.changed();
                }
                next
            } else {
                Ok(None)
            };
            let entry = match next {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    // Nothing to send, the next entry is not durable yet, or another tab sends.
                    let outbox = pin!(self.inner.outbox_changed.changed(seen));
                    let _ = match (&self.inner.saved, saved_seen) {
                        (Some(saved), Some(g)) => self
                            .unless_stopped(select(outbox, pin!(saved.changed(g))))
                            .await
                            .map(drop),
                        _ => self.unless_stopped(outbox).await.map(drop),
                    };
                    continue;
                }
                Err(err) => {
                    self.storage_failed("update the outbox", &err);
                    self.back_off(&mut failures, None).await;
                    continue;
                }
            };
            let Some(result) = self.unless_stopped(self.send_entry(&entry)).await else {
                return;
            };
            match result {
                SendResult::Accepted(r) => {
                    let stored = self.with(|s| {
                        s.send = SendState::Ok;
                        s.replica.acknowledge(entry.command_id, &r)
                    });
                    match stored {
                        Ok(Ack::Recorded) => {
                            failures = 0;
                            tracing::debug!(command_id = %entry.command_id, events = r.events.len(), "command acknowledged");
                            self.changed();
                        }
                        Ok(Ack::OtherEpoch) => {
                            // A response from before a resync, or from a log this client has yet
                            // to switch to: not recorded. The entry is sent again (the server
                            // replays the result if it has the command, or applies it if it lost it),
                            // after a backoff so a pending switch can happen first.
                            tracing::debug!(command_id = %entry.command_id, "acknowledgement from another log epoch; sending again");
                            self.back_off(&mut failures, None).await;
                        }
                        Ok(Ack::Diverged) => {
                            tracing::warn!(
                                "the server's log differs from the confirmed state; resyncing"
                            );
                            // The entry is sent again after the resync (the server replays the
                            // result), not before.
                            self.await_resync().await;
                        }
                        Err(err) => {
                            // The entry stays unacknowledged and is sent again; the server replays.
                            self.storage_failed("record an acknowledgement", &err);
                            self.back_off(&mut failures, None).await;
                        }
                    }
                }
                SendResult::Rejected(rejection) => {
                    tracing::info!(
                        command_id = %entry.command_id,
                        status = rejection.status,
                        code = %rejection.code,
                        "the server rejected a command"
                    );
                    let replaced = rejection.code == EPOCH_MISMATCH;
                    let rejection = explain(&entry, rejection);
                    let now = self.inner.clock.now();
                    let stored = self.with(|s| {
                        s.send = SendState::Ok;
                        s.replica.reject(entry.command_id, rejection, now)
                    });
                    match stored {
                        Ok(()) => {
                            failures = 0;
                            self.changed();
                        }
                        Err(err) => {
                            self.storage_failed("record a rejection", &err);
                            self.back_off(&mut failures, None).await;
                            continue;
                        }
                    }
                    if replaced {
                        // This replica follows a log the server no longer has.
                        self.with(|s| s.resync_requested = true);
                        self.inner.resync.notify();
                    }
                }
                SendResult::Retry {
                    error,
                    offline,
                    after,
                } => {
                    tracing::debug!(command_id = %entry.command_id, attempt = entry.attempts, reason = %error.message, "command not confirmed; retrying");
                    self.with(|s| {
                        s.send = SendState::Failing { offline };
                        s.last_error = Some(error);
                    });
                    self.changed();
                    self.back_off(&mut failures, after).await;
                }
            }
        }
    }

    async fn send_entry(&self, entry: &OutboxEntry) -> SendResult {
        let request = match protocol::entry_request(entry.command_id, &entry.op) {
            Ok(r) => r,
            Err(e) => {
                return SendResult::Rejected(Rejection {
                    status: 0,
                    code: "encode".into(),
                    message: e.to_string(),
                });
            }
        };
        let response = timeout(
            &self.inner.clock,
            self.inner.config.request_timeout,
            self.inner.transport.send(request),
        )
        .await;
        match response {
            None => SendResult::Retry {
                error: self.error(ErrorKind::Network, TransportError::Timeout.to_string()),
                offline: true,
                after: None,
            },
            Some(Err(e)) => SendResult::Retry {
                error: self.error(ErrorKind::Network, e.to_string()),
                offline: true,
                after: None,
            },
            Some(Ok(res)) => match protocol::classify_send(&res) {
                SendOutcome::Accepted(r) => SendResult::Accepted(r),
                SendOutcome::Rejected(r) => SendResult::Rejected(r),
                SendOutcome::Retry {
                    reason,
                    after,
                    unauthorized,
                } => SendResult::Retry {
                    error: self.error(
                        if unauthorized {
                            ErrorKind::Unauthorized
                        } else {
                            ErrorKind::Server
                        },
                        reason,
                    ),
                    offline: false,
                    after,
                },
            },
        }
    }

    async fn event_loop(&self) {
        let mut failures = 0u32;
        // The event the last (durably completed) divergence-triggered resync was for.
        let mut last_divergence: Option<u64> = None;
        // A resync whose reset failed: retried before anything else.
        let mut pending_reset: Option<Option<u64>> = None;
        loop {
            if self.is_stopped() {
                return;
            }
            let step = match pending_reset.take() {
                Some(diverged_at) => Step::Resync { diverged_at },
                None => match self.connect_once().await {
                    Step::Resync {
                        diverged_at: Some(seq),
                    } if last_divergence == Some(seq) => Step::Fail {
                        error: self.error(
                            ErrorKind::Protocol,
                            format!("event {seq} does not apply, even after a full resync"),
                        ),
                        offline: false,
                        retry_after: None,
                        progressed: false,
                    },
                    step => step,
                },
            };
            match step {
                Step::Stopped => return,
                Step::Reconnect => failures = 0,
                Step::Resync { diverged_at } => {
                    tracing::info!(
                        ?diverged_at,
                        "the server's log is not the one this client followed; resyncing from 0"
                    );
                    match self.with(|s| s.replica.reset()) {
                        Ok(()) => {
                            // Starting over from nothing: not caught up until it has the log again.
                            self.with(|s| s.readiness = Readiness::Starting);
                            // Only a durable reset counts: it arms the repeat-divergence guard and
                            // releases a sender waiting to resend a divergent acknowledgement.
                            last_divergence = diverged_at;
                            self.inner.resynced.notify();
                            // Acknowledged entries are sent again.
                            self.inner.outbox_changed.notify();
                            self.changed();
                        }
                        Err(err) => {
                            pending_reset = Some(diverged_at);
                            self.storage_failed("forget the confirmed state", &err);
                            self.back_off(&mut failures, None).await;
                        }
                    }
                }
                Step::Fail {
                    error,
                    offline,
                    retry_after,
                    progressed,
                } => {
                    if progressed {
                        failures = 0;
                    }
                    tracing::debug!(reason = %error.message, "event stream down");
                    self.with(|s| {
                        s.stream = StreamState::Down { offline };
                        s.last_error = Some(error);
                        if s.readiness == Readiness::Starting {
                            s.readiness = Readiness::Failed;
                        }
                    });
                    self.changed();
                    self.back_off(&mut failures, retry_after).await;
                }
            }
        }
    }

    /// The stream ended; reconnect after a backoff that starts over if it delivered anything.
    fn down(&self, message: &str, offline: bool, progressed: bool) -> Step {
        Step::Fail {
            error: self.error(ErrorKind::Network, message),
            offline,
            retry_after: None,
            progressed,
        }
    }

    fn fail(&self, kind: ErrorKind, message: impl Into<String>, offline: bool) -> Step {
        Step::Fail {
            error: self.error(kind, message),
            offline,
            retry_after: None,
            progressed: false,
        }
    }

    /// Opens the stream once and follows it until it ends.
    async fn connect_once(&self) -> Step {
        if self.take_resync_request() {
            return Step::Resync { diverged_at: None };
        }
        let (since, epoch) = self.with(|s| (s.replica.received(), s.replica.epoch()));
        let opened = self
            .unless_stopped(timeout(
                &self.inner.clock,
                self.inner.config.request_timeout,
                self.inner
                    .transport
                    .open_stream(protocol::stream_request(since, epoch)),
            ))
            .await;
        match opened {
            None => Step::Stopped,
            Some(None) => self.fail(
                ErrorKind::Network,
                "opening the event stream timed out",
                true,
            ),
            Some(Some(Err(e))) => self.fail(ErrorKind::Network, e.to_string(), true),
            Some(Some(Ok(StreamOpen::Refused(res)))) => self.refused(&res).await,
            Some(Some(Ok(StreamOpen::Open(stream)))) => self.follow(stream).await,
        }
    }

    /// Handles a stream request the server refused.
    async fn refused(&self, res: &Response) -> Step {
        let detail = error_detail(res);
        let code = detail.as_ref().map_or("", |d| d.code.as_str());
        if (res.status, code) == (409, "catch_up_required") {
            let before = self.received();
            return match self.catch_up().await {
                Step::Reconnect if self.received() == before => self.fail(
                    ErrorKind::Protocol,
                    "the server asked to catch up, but there was nothing to fetch",
                    false,
                ),
                step => step,
            };
        }
        self.refused_request("the event stream", res)
    }

    /// What to do about a refused stream or page request (other than `catch_up_required`).
    fn refused_request(&self, what: &str, res: &Response) -> Step {
        let code = error_detail(res).map(|d| d.code).unwrap_or_default();
        match (res.status, code.as_str()) {
            (400, "invalid_since") | (409, EPOCH_MISMATCH) => Step::Resync { diverged_at: None },
            (status, code) => Step::Fail {
                error: self.error(
                    if status == 401 {
                        ErrorKind::Unauthorized
                    } else if status >= 500 || status == 429 {
                        ErrorKind::Server
                    } else {
                        ErrorKind::Protocol
                    },
                    if status == 401 {
                        "the server rejected the token (401)".to_owned()
                    } else {
                        format!("{what} was refused: {status} {code}")
                    },
                ),
                offline: false,
                retry_after: res.retry_after,
                progressed: false,
            },
        }
    }

    fn take_resync_request(&self) -> bool {
        self.with(|s| std::mem::take(&mut s.resync_requested))
    }

    /// Applies confirmed events from the log with `epoch` (or only checks the epoch). Returns the step
    /// to take if they could not all be applied, except for a gap, which the caller handles.
    fn confirm(&self, epoch: Epoch, events: &[LogEvent]) -> Result<(), ConfirmFailure> {
        let (result, has_outbox) = self.with(|s| {
            let result = s.replica.apply_confirmed(epoch, events);
            (result, !s.replica.outbox().is_empty())
        });
        // A confirmation may have resolved the next waiting undo.
        if has_outbox && !events.is_empty() {
            self.inner.outbox_changed.notify();
        }
        self.changed();
        match result {
            Ok(_) => Ok(()),
            Err(ConfirmError::Gap { .. }) => Err(ConfirmFailure::Gap),
            Err(ConfirmError::EpochChanged) => {
                Err(ConfirmFailure::Step(Step::Resync { diverged_at: None }))
            }
            Err(ConfirmError::Diverged { seq, error }) => {
                tracing::warn!(seq, error = %error, "a confirmed event does not apply");
                Err(ConfirmFailure::Step(Step::Resync {
                    diverged_at: Some(seq),
                }))
            }
            Err(ConfirmError::Storage(err)) => {
                // Nothing was applied: the events are fetched again after the backoff.
                tracing::warn!(error = %err, "cannot store confirmed events");
                Err(ConfirmFailure::Step(self.fail(
                    ErrorKind::Storage,
                    format!("cannot store confirmed events: {err}"),
                    false,
                )))
            }
            Err(e @ ConfirmError::MissingSeq) => Err(ConfirmFailure::Step(self.fail(
                ErrorKind::Protocol,
                e.to_string(),
                false,
            ))),
        }
    }

    /// Pages `GET /api/v1/events` from the cursor until the server reports no more.
    async fn catch_up(&self) -> Step {
        self.with(|s| s.stream = StreamState::CatchingUp);
        self.changed();
        loop {
            let (since, epoch) = self.with(|s| (s.replica.received(), s.replica.epoch()));
            let request = protocol::events_page_request(since, self.inner.config.page_limit, epoch);
            let response = self
                .unless_stopped(timeout(
                    &self.inner.clock,
                    self.inner.config.request_timeout,
                    self.inner.transport.send(request),
                ))
                .await;
            let res = match response {
                None => return Step::Stopped,
                Some(None) => {
                    return self.fail(ErrorKind::Network, "a page of events timed out", true);
                }
                Some(Some(Err(e))) => return self.fail(ErrorKind::Network, e.to_string(), true),
                Some(Some(Ok(res))) => res,
            };
            if res.status != 200 {
                return self.refused_request("a page of events", &res);
            }
            let page: EventPage = match serde_json::from_slice(&res.body) {
                Ok(p) => p,
                Err(e) => {
                    return self.fail(
                        ErrorKind::Protocol,
                        format!("unreadable event page: {e}"),
                        false,
                    );
                }
            };
            match self.confirm(page.epoch, &page.events) {
                Ok(()) => {}
                Err(ConfirmFailure::Gap) => {
                    return self.fail(
                        ErrorKind::Protocol,
                        "the server sent a page with a gap",
                        false,
                    );
                }
                Err(ConfirmFailure::Step(step)) => return step,
            }
            if !page.has_more {
                self.reached_head();
                return Step::Reconnect;
            }
            if self.received() == since {
                return self.fail(
                    ErrorKind::Protocol,
                    "the server reported more events but sent none",
                    false,
                );
            }
        }
    }

    /// Follows an open stream until it ends.
    async fn follow(&self, mut stream: T::Stream) -> Step {
        self.with(|s| s.stream = StreamState::Live);
        self.changed();
        let mut progressed = false;
        // From the `hello` frame, which comes first.
        let mut epoch: Option<Epoch> = None;
        // The server's head as the `hello` frame gave it, until the stream has delivered it.
        let mut head: Option<u64> = None;
        loop {
            let seen = self.inner.resync.generation();
            if self.take_resync_request() {
                return Step::Resync { diverged_at: None };
            }
            let next = timeout(
                &self.inner.clock,
                self.inner.config.stream_idle_timeout,
                stream.next_frame(),
            );
            let resync = self.inner.resync.changed(seen);
            let frame = self
                .unless_stopped(async {
                    match select(pin!(next), pin!(resync)).await {
                        Either::Left((frame, _)) => Some(frame),
                        Either::Right(_) => None,
                    }
                })
                .await;
            let frame = match frame {
                None => return Step::Stopped,
                // A resync was requested: the loop top handles it.
                Some(None) => continue,
                Some(Some(frame)) => frame,
            };
            let frame = match frame {
                None => {
                    return self.down("the event stream went quiet", true, progressed);
                }
                Some(None) => {
                    return self.down("the server closed the event stream", false, progressed);
                }
                Some(Some(Err(TransportError::Protocol(message)))) => {
                    // Typically a frame over the size cap. Page past it instead of reconnecting
                    // into it again.
                    tracing::warn!(reason = %message, "the event stream broke the protocol; paging instead");
                    let before = self.received();
                    return match self.catch_up().await {
                        Step::Reconnect if self.received() == before => {
                            self.fail(ErrorKind::Protocol, message, false)
                        }
                        step => step,
                    };
                }
                Some(Some(Err(e))) => return self.down(&e.to_string(), true, progressed),
                Some(Some(Ok(frame))) => frame,
            };
            progressed = true;
            let SseFrame::Event { event, data, .. } = frame else {
                continue;
            };
            if event == HELLO_FRAME {
                let hello: Hello = match serde_json::from_str(&data) {
                    Ok(h) => h,
                    Err(e) => {
                        return self.fail(
                            ErrorKind::Protocol,
                            format!("unreadable hello frame: {e}"),
                            false,
                        );
                    }
                };
                if let Err(ConfirmFailure::Step(step)) = self.confirm(hello.epoch, &[]) {
                    return step;
                }
                epoch = Some(hello.epoch);
                match hello.head_seq {
                    Some(h) if self.received() < h => head = Some(h),
                    // Already there, or a server that doesn't say (agent tokens).
                    _ => self.reached_head(),
                }
            } else if event == EVENT_FRAME {
                let Some(epoch) = epoch else {
                    return self.fail(
                        ErrorKind::Protocol,
                        "the event stream sent an event before its hello frame",
                        false,
                    );
                };
                let ev: LogEvent = match serde_json::from_str(&data) {
                    Ok(ev) => ev,
                    Err(e) => {
                        return self.fail(
                            ErrorKind::Protocol,
                            format!("unreadable event: {e}"),
                            false,
                        );
                    }
                };
                match self.confirm(epoch, std::slice::from_ref(&ev)) {
                    Ok(()) => {
                        if head.is_some_and(|h| self.received() >= h) {
                            head = None;
                            self.reached_head();
                        }
                    }
                    Err(ConfirmFailure::Gap) => {
                        // Fill the gap from the JSON endpoint, then go on with the stream (it skips
                        // what is now below the cursor).
                        match self.catch_up().await {
                            Step::Reconnect => {
                                self.with(|s| s.stream = StreamState::Live);
                                self.changed();
                            }
                            step => return step,
                        }
                    }
                    Err(ConfirmFailure::Step(step)) => return step,
                }
            } else if event == CATCH_UP_FRAME {
                let target = serde_json::from_str::<CatchUpNotice>(&data).map(|n| n.head_seq);
                tracing::debug!(head_seq = ?target.ok(), "the server asked to catch up");
                return self.catch_up().await;
            }
        }
    }
}

/// Why [`SyncClient::confirm`] did not apply everything.
enum ConfirmFailure {
    /// Events are missing before the ones given: page them.
    Gap,
    /// Anything else, with the step to take.
    Step(Step),
}

/// Replaces the server's message of a stale undo with one that says what happened.
fn explain(entry: &OutboxEntry, mut rejection: Rejection) -> Rejection {
    if let Operation::Undo { seq, .. } = entry.op
        && (rejection.code == EPOCH_MISMATCH || rejection.code == EVENT_MISMATCH)
    {
        rejection.message = format!(
            "not undone: the server's history was replaced (restored from a backup) after this undo was queued, so event {seq} is no longer the change it was meant for"
        );
    }
    rejection
}
