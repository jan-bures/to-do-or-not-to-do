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
//! The event loop subscribes to `GET /api/v1/events` (SSE) from the persisted cursor and applies each
//! event to the confirmed state and to storage. It handles the server's catch-up protocol: a
//! `catch_up` frame or a 409 `catch_up_required` makes it page the JSON endpoint up to the head and
//! reconnect; a 400 `invalid_since` (the cursor is ahead of the server, for example after a restore
//! from backup) makes it forget the confirmed state and resync from 0, keeping the outbox. Lost
//! connections are retried with backoff from the cursor. A stream that shows no sign of life (no
//! event and no heartbeat) for [`ClientConfig::stream_idle_timeout`] is dropped and reopened.

use crate::notify::{Backoff, Notify, sleep_or_wake, timeout};
use crate::outbox::{CommandId, FailedEntry, Operation, OutboxEntry, Rejection};
use crate::platform::{
    Clock, Entropy, EventStream, Response, SseFrame, StreamOpen, Transport, TransportError,
};
use crate::protocol::{
    self, CATCH_UP_FRAME, CatchUpNotice, EVENT_FRAME, EventPage, SendOutcome, error_detail,
};
use crate::replica::{Ack, ConfirmError, OpenError, Replica, SubmitError};
use crate::storage::{Storage, StorageError};
use futures_util::future::{Either, join, select};
use std::fmt;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use todo_core::event::Event;
use todo_core::ids::uuid_v7;
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
}

impl Shared {
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
    /// Bumped by [`SyncClient::nudge`]: cut retry delays short.
    wake: Notify,
    /// Bumped once by [`SyncClient::stop`].
    stop: Notify,
    /// Bumped when [`Shared::resync_requested`] is set.
    resync: Notify,
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
                }),
                outbox_changed: Notify::default(),
                wake: Notify::default(),
                stop: Notify::default(),
                resync: Notify::default(),
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

    fn enqueue(&self, op: Operation) -> Result<CommandId, SubmitError> {
        let command_id = self.new_uuid();
        let now = self.inner.clock.now();
        self.with(|s| {
            // Strictly increasing submission times keep two identical commands submitted within one
            // millisecond from deciding to identical events in the view.
            let ms = now.as_millisecond().max(s.last_submitted_ms + 1);
            let submitted_at = Timestamp::from_millisecond(ms).unwrap_or(now);
            s.replica.submit(OutboxEntry {
                command_id,
                op,
                submitted_at,
                attempts: 0,
                acked: None,
            })?;
            s.last_submitted_ms = ms;
            Ok::<_, SubmitError>(())
        })?;
        self.inner.outbox_changed.notify();
        self.changed();
        Ok(command_id)
    }

    /// Queues a command and applies it to the view at once.
    ///
    /// Fails without queueing if the command is invalid against the current view, or if the outbox
    /// cannot be written.
    pub fn submit(&self, command: Command) -> Result<CommandId, SubmitError> {
        self.enqueue(Operation::Command { command })
    }

    /// Queues an undo of the command that produced event `seq` (or a redo, if `seq` is a compensating
    /// event). It shows in the view once the server confirms it.
    pub fn undo(&self, seq: u64, force: bool) -> Result<CommandId, SubmitError> {
        self.enqueue(Operation::Undo { seq, force })
    }

    /// The confirmed state with pending commands applied.
    #[must_use]
    pub fn view(&self) -> Arc<State> {
        self.with(|s| s.replica.view())
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

    /// A handle that waits for changes of the view or the status.
    #[must_use]
    pub fn changes(&self) -> Changes {
        Changes {
            notify: self.inner.changes.clone(),
            seen: self.inner.changes.generation(),
        }
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

    async fn send_loop(&self) {
        let mut failures = 0u32;
        loop {
            if self.is_stopped() {
                return;
            }
            let seen = self.inner.outbox_changed.generation();
            let next = self.with(|s| s.replica.next_to_send());
            let entry = match next {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    let _ = self
                        .unless_stopped(self.inner.outbox_changed.changed(seen))
                        .await;
                    continue;
                }
                Err(err) => {
                    let error = self.error(ErrorKind::Storage, err.to_string());
                    tracing::warn!(error = %err, "cannot update the outbox");
                    self.with(|s| s.last_error = Some(error));
                    self.changed();
                    self.back_off(&mut failures, None).await;
                    continue;
                }
            };
            let Some(result) = self.unless_stopped(self.send_entry(&entry)).await else {
                return;
            };
            match result {
                SendResult::Accepted(r) => {
                    failures = 0;
                    let stored = self.with(|s| {
                        s.send = SendState::Ok;
                        s.replica.acknowledge(entry.command_id, &r)
                    });
                    if let Ok(Ack::Diverged) = stored {
                        tracing::warn!(
                            "the server's log differs from the confirmed state; resyncing"
                        );
                        self.with(|s| s.resync_requested = true);
                        self.inner.resync.notify();
                        self.changed();
                        // The entry is resent once the resync caught up (the server replays the
                        // result); until then, do not hammer it.
                        self.back_off(&mut failures, None).await;
                        continue;
                    }
                    if let Err(err) = stored {
                        // The entry stays in the outbox and is resent; the server replays the result.
                        tracing::warn!(error = %err, "cannot record an acknowledgement");
                        let error = self.error(ErrorKind::Storage, err.to_string());
                        self.with(|s| s.last_error = Some(error));
                    }
                    tracing::debug!(command_id = %entry.command_id, events = r.events.len(), "command acknowledged");
                    self.changed();
                }
                SendResult::Rejected(rejection) => {
                    failures = 0;
                    tracing::info!(
                        command_id = %entry.command_id,
                        status = rejection.status,
                        code = %rejection.code,
                        "the server rejected a command"
                    );
                    let now = self.inner.clock.now();
                    let stored = self.with(|s| {
                        s.send = SendState::Ok;
                        s.replica.reject(entry.command_id, rejection, now)
                    });
                    if let Err(err) = stored {
                        tracing::warn!(error = %err, "cannot record a rejection");
                        let error = self.error(ErrorKind::Storage, err.to_string());
                        self.with(|s| s.last_error = Some(error));
                    }
                    self.changed();
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
        loop {
            if self.is_stopped() {
                return;
            }
            match self.connect_once().await {
                Step::Stopped => return,
                Step::Reconnect => failures = 0,
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
            return self.resync().await;
        }
        let since = self.cursor();
        let opened = self
            .unless_stopped(timeout(
                &self.inner.clock,
                self.inner.config.request_timeout,
                self.inner
                    .transport
                    .open_stream(protocol::stream_request(since)),
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

    /// Handles a stream (or page) request the server refused.
    async fn refused(&self, res: &Response) -> Step {
        let detail = error_detail(res);
        let code = detail.as_ref().map_or("", |d| d.code.as_str());
        match (res.status, code) {
            (409, "catch_up_required") => {
                let before = self.cursor();
                match self.catch_up().await {
                    Step::Reconnect if self.cursor() == before => self.fail(
                        ErrorKind::Protocol,
                        "the server asked to catch up, but there was nothing to fetch",
                        false,
                    ),
                    step => step,
                }
            }
            (400, "invalid_since") => self.resync().await,
            (401, _) => Step::Fail {
                error: self.error(
                    ErrorKind::Unauthorized,
                    "the server rejected the token (401)",
                ),
                offline: false,
                retry_after: res.retry_after,
                progressed: false,
            },
            (status, code) => Step::Fail {
                error: self.error(
                    if status >= 500 || status == 429 {
                        ErrorKind::Server
                    } else {
                        ErrorKind::Protocol
                    },
                    format!("the event stream was refused: {status} {code}"),
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

    /// Forgets the confirmed state and refetches the whole log, keeping the outbox.
    async fn resync(&self) -> Step {
        tracing::info!("the server's log is behind this client's cursor; resyncing from 0");
        if let Err(err) = self.with(|s| s.replica.reset()) {
            return self.fail(ErrorKind::Storage, err.to_string(), false);
        }
        // Acknowledged entries are sent again.
        self.inner.outbox_changed.notify();
        self.changed();
        self.catch_up().await
    }

    /// Applies confirmed events. A gap or a divergence is reported to the caller, which pages or
    /// resyncs.
    fn confirm(&self, events: &[Event]) -> Result<(), ConfirmError> {
        let result = self.with(|s| s.replica.apply_confirmed(events));
        let result = match result {
            Err(ConfirmError::Storage(err)) => {
                // Applied in memory; the next write stores a snapshot.
                tracing::warn!(error = %err, "cannot store confirmed events");
                let error = self.error(ErrorKind::Storage, err.to_string());
                self.with(|s| s.last_error = Some(error));
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        };
        self.changed();
        result
    }

    /// Pages `GET /api/v1/events` from the cursor until the server reports no more.
    async fn catch_up(&self) -> Step {
        self.with(|s| s.stream = StreamState::CatchingUp);
        self.changed();
        loop {
            let since = self.cursor();
            let request = protocol::events_page_request(since, self.inner.config.page_limit);
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
                return Box::pin(self.refused_page(&res)).await;
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
            match self.confirm(&page.events) {
                Ok(()) => {}
                Err(ConfirmError::Diverged(err)) => {
                    tracing::warn!(error = %err, "confirmed state diverged; resyncing");
                    return Box::pin(self.resync()).await;
                }
                Err(e) => return self.fail(ErrorKind::Protocol, e.to_string(), false),
            }
            if !page.has_more {
                return Step::Reconnect;
            }
            if self.cursor() == since {
                return self.fail(
                    ErrorKind::Protocol,
                    "the server reported more events but sent none",
                    false,
                );
            }
        }
    }

    async fn refused_page(&self, res: &Response) -> Step {
        let code = error_detail(res).map(|d| d.code).unwrap_or_default();
        if res.status == 400 && code == "invalid_since" {
            return self.resync().await;
        }
        Step::Fail {
            error: self.error(
                if res.status == 401 {
                    ErrorKind::Unauthorized
                } else if res.status >= 500 || res.status == 429 {
                    ErrorKind::Server
                } else {
                    ErrorKind::Protocol
                },
                format!("a page of events was refused: {} {code}", res.status),
            ),
            offline: false,
            retry_after: res.retry_after,
            progressed: false,
        }
    }

    /// Follows an open stream until it ends.
    async fn follow(&self, mut stream: T::Stream) -> Step {
        self.with(|s| s.stream = StreamState::Live);
        self.changed();
        let mut progressed = false;
        loop {
            let seen = self.inner.resync.generation();
            if self.take_resync_request() {
                return self.resync().await;
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
                Some(Some(Err(e))) => return self.down(&e.to_string(), true, progressed),
                Some(Some(Ok(frame))) => frame,
            };
            progressed = true;
            let SseFrame::Event { event, data, .. } = frame else {
                continue;
            };
            if event == EVENT_FRAME {
                let ev: Event = match serde_json::from_str(&data) {
                    Ok(ev) => ev,
                    Err(e) => {
                        return self.fail(
                            ErrorKind::Protocol,
                            format!("unreadable event: {e}"),
                            false,
                        );
                    }
                };
                match self.confirm(std::slice::from_ref(&ev)) {
                    Ok(()) => {}
                    Err(ConfirmError::Gap { .. }) => {
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
                    Err(ConfirmError::Diverged(err)) => {
                        tracing::warn!(error = %err, "confirmed state diverged; resyncing");
                        return self.resync().await;
                    }
                    Err(e) => return self.fail(ErrorKind::Protocol, e.to_string(), false),
                }
            } else if event == CATCH_UP_FRAME {
                let target = serde_json::from_str::<CatchUpNotice>(&data).map(|n| n.head_seq);
                tracing::debug!(head_seq = ?target.ok(), "the server asked to catch up");
                return self.catch_up().await;
            }
        }
    }
}
