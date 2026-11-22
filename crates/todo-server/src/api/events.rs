//! `GET /api/v1/events`: JSON pages of the log, or a Server-Sent Events stream.
//!
//! The stream subscribes to the live broadcast *before* replaying from the database, so nothing
//! committed in between is lost. Live events at or below the last sent sequence number are skipped;
//! a lagged receiver or a gap in sequence numbers is repaired by re-reading from the database.
//!
//! Every event carries the `command_id` (and `command_idx`, its index among that command's events)
//! that produced it, so a client recognises its own commands by ID.
//!
//! # Whole batches
//!
//! A command's events are written in one transaction, consecutively, but a page or a stream can end
//! between them. So every event also carries `command_len`, how many events its command produced: the
//! event with `command_idx == command_len - 1` ends the batch. A client applies a batch only once it
//! has all of it (`todo-client` holds the start until the rest arrives), so it never shows half a
//! command, or an undo predicted from part of one. The field is optional: older clients ignore it,
//! and it is left out for agents, which may not see every event of a command (counting the hidden
//! ones would tell them about events they can't read).
//!
//! # Log epoch
//!
//! The log has an *epoch*: a random ID created with the database and rotated when a backup is
//! restored (`todo-server rotate-log-epoch`). Sequence numbers only mean something within one epoch.
//! Every JSON page carries `epoch`; a stream sends it in the [`LOG_EPOCH_HEADER`] response header and
//! in a first [`HELLO`] frame (data: [`Hello`], no `id`). A client passes the epoch its cursor belongs
//! to as `epoch`; if the log has another one, the request gets 409 `epoch_mismatch` (`details.epoch`:
//! the current one) before anything else is checked, and the client must resync from 0.
//!
//! # Catching up
//!
//! A stream replays at most `limits.max_replay_events` events and `limits.max_replay_bytes` bytes. A
//! client further behind than that catches up through JSON pages instead:
//!
//! 1. **At connect**, a resume point more than `max_replay_events` behind the head gets 409
//!    `catch_up_required` (`details`: `since`, `head_seq`). A resume point ahead of the head gets 400
//!    `invalid_since`. Neither takes a stream slot or reads the database.
//! 2. **During a stream** (the byte budget ran out, or the client lagged too far), the server sends one
//!    [`CATCH_UP`] frame (data: [`CatchUp`]) and closes the stream. It also marks the principal: a
//!    reconnect at or before that position (what `EventSource` does by itself) gets the same 409 at
//!    once. A 409 makes `EventSource` stop retrying.
//! 3. **The client** captures `head_seq`, pages `GET /api/v1/events?since=` (JSON) following
//!    `next_since` until it reaches that head, then opens a fresh stream with `since` set to where it
//!    got to (or `Last-Event-ID`). The mark is the highest point any of the principal's streams gave
//!    up at; streams from later points may connect meanwhile, and a JSON page reaching past the mark
//!    clears it.
//!
//! # Access
//!
//! Agents receive only the events they may read ([`visibility::event_visible`]), in JSON pages and on
//! the stream alike. Hidden events still advance `next_since` (and the stream's position), so a
//! client never re-reads them. Visibility follows grant changes at once.
//!
//! A stream closes as soon as its token is revoked, and at the next heartbeat after it expires. It
//! subscribes to revocations before it checks its caller one last time, so a revocation between
//! authentication and the start of the stream is not missed. Replay reads, sends blocked on a slow
//! client, and the live loop all stop on a revocation, and frames already queued for the client but
//! not yet written are dropped.
//!
//! # Bounds
//!
//! - **Connects.** Each principal may (re)connect `limits.sse_connects_per_minute` times per minute
//!   (burst `limits.sse_connect_burst`); more gets 429 with `Retry-After`. This is checked first.
//! - **Slots.** The stream slot is owned by the response body, so it is freed when the body is dropped:
//!   when the stream ends, the client disconnects, or the connection is closed for making no write
//!   progress (see [`crate::conn`]).
//! - **Backpressure.** Every send waits at most `limits.sse_stall_timeout_ms` for the client to take
//!   frames, and gives up at shutdown.
//! - **Reads** take a permit from the shared read pool (held inside the blocking read). Waiting for
//!   the permit and for the read itself ends on disconnect, shutdown or the stall deadline. Each read
//!   is bounded by the remaining byte budget before rows are decoded.

use super::dto::{EventPage, EventsParams};
use super::error::{ApiError, ErrorBody};
use super::{ApiQuery, Work, blocking, visibility};
use crate::app::AppState;
use crate::auth::service::{AuthService, Standing};
use crate::auth::{Authenticated, Principal};
use crate::db::CappedEvents;
use crate::engine::{Engine, LiveEvent};
use crate::ratelimit::CatchUpMarks;
use crate::scope::EventScope;
use axum::Json;
use axum::extract::State;
use axum::http::header::ACCEPT;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::Stream;
use serde::Serialize;
use serde_json::json;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use todo_core::Actor;
use todo_core::event::Event;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;

/// Events read from the database per replay page.
const REPLAY_PAGE: usize = 500;
/// Largest JSON page of `GET /api/v1/events`.
pub const MAX_EVENTS_PAGE: usize = 1000;
/// Most bytes of one JSON page, envelope included (at least one event is returned unless that event
/// alone is larger, which is a 500 `item_too_large` whose details carry the `next_since` to skip it).
pub const MAX_EVENTS_PAGE_BYTES: usize = 4 * 1024 * 1024;
/// Bytes of a JSON page reserved for the envelope around the events.
const PAGE_ENVELOPE: usize = 1024;
/// SSE frames buffered per client before the producer waits.
const CLIENT_BUFFER: usize = 64;
/// The comment text of heartbeat frames (`:heartbeat`).
pub const HEARTBEAT: &str = "heartbeat";
/// The SSE event name of the "too far behind" notice. Its data is a [`CatchUp`].
pub const CATCH_UP: &str = "catch_up";
/// The SSE event name of the first frame of every stream. Its data is a [`Hello`].
pub const HELLO: &str = "hello";
/// Initial replay is complete on a stream that doesn't expose the head sequence number.
pub const BACKLOG_END: &str = "backlog_end";
/// The response header that carries the log epoch of a stream.
pub const LOG_EPOCH_HEADER: &str = "log-epoch";

/// Data of the [`HELLO`] SSE event, the first frame of every stream.
#[derive(Debug, Clone, Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct Hello {
    /// The log epoch (see the module documentation). Sequence numbers belong to it.
    pub epoch: uuid::Uuid,
    /// The newest sequence number when the stream opened. Absent for agents: the newest event of the
    /// whole log would tell them how busy lists they cannot read are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_seq: Option<u64>,
}

/// True if the client asked for an event stream.
#[must_use]
pub fn wants_sse(headers: &HeaderMap) -> bool {
    headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.to_ascii_lowercase().contains("text/event-stream"))
}

fn last_event_id(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
}

/// Data of the [`CATCH_UP`] SSE event: the client is too far behind to be replayed over the stream.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CatchUp {
    /// `too_far_behind` (event count) or `too_many_bytes` (byte budget).
    pub reason: &'static str,
    /// The last sequence number this stream delivered (or the one it started from).
    pub since: u64,
    /// The newest sequence number when the stream gave up. Page up to here.
    pub head_seq: u64,
    /// What to do.
    pub hint: String,
}

/// The event log since a sequence number, as JSON or as an SSE stream (`Accept: text/event-stream`).
///
/// SSE frames carry `id: <seq>`, `event: event` and the event JSON as `data`; the first frame is
/// `event: hello` with the log epoch. On agent streams, `event: backlog_end` marks the end of the
/// initial replay without disclosing a sequence number. A `:heartbeat` comment is sent periodically.
/// On reconnect, `Last-Event-ID` takes precedence over `since`. See the module documentation for the
/// log epoch (409 `epoch_mismatch`) and catch-up protocol (409 `catch_up_required`, `catch_up` frame).
#[utoipa::path(
    get, path = "/api/v1/events", tag = "events", params(EventsParams),
    responses(
        (status = 200, description = "A page of events, or an SSE stream when `Accept: text/event-stream`", body = EventPage),
        (status = 400, description = "`since` is ahead of the log (`invalid_since`)", body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 409, description = "`epoch` is not this log's epoch (`epoch_mismatch`; resync from 0). Stream only: too far behind to replay, or reconnecting to where the last stream sent `catch_up` (`catch_up_required`); page the JSON form first", body = ErrorBody),
        (status = 429, description = "Too many stream (re)connects; see `Retry-After`", body = ErrorBody),
        (status = 500, description = "One event is larger than the page limit (`item_too_large`, with `next_since` to skip it)", body = ErrorBody),
        (status = 503, description = "Too many open streams, or the server is busy", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn get_events(
    auth: Authenticated,
    State(app): State<AppState>,
    headers: HeaderMap,
    ApiQuery(q): ApiQuery<EventsParams>,
) -> Result<Response, ApiError> {
    if wants_sse(&headers) {
        // Cheap checks first: rate limit, health, position, catch-up mark; then the slot.
        app.sse_connects
            .check(&auth.principal)
            .map_err(ApiError::rate_limited)?;
        if !app.engine.is_healthy() {
            return Err(ApiError::unavailable());
        }
        check_epoch(&app.engine, q.epoch)?;
        // A reconnecting browser keeps the original URL (with its `since`) and adds Last-Event-ID,
        // which is the newer position.
        let since = last_event_id(&headers).or(q.since).unwrap_or(0);
        let head_seq = app.engine.head_seq();
        if since > head_seq {
            return Err(ApiError::invalid_since(since, head_seq));
        }
        if head_seq - since > app.limits.max_replay_events as u64 {
            return Err(ApiError::catch_up_required(
                since,
                head_seq,
                "too_far_behind",
            ));
        }
        if app.catch_up_marks.must_catch_up(&auth.principal, since) {
            return Err(ApiError::catch_up_required(
                since,
                head_seq,
                "catch_up_sent",
            ));
        }
        let permit = app
            .sse_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::too_many_streams())?;
        return Ok(stream(&app, auth.principal, auth.actor, since, permit).into_response());
    }
    check_epoch(&app.engine, q.epoch)?;
    let since = q.since.unwrap_or(0);
    let limit = q.limit.unwrap_or(REPLAY_PAGE).clamp(1, MAX_EVENTS_PAGE);
    let engine = app.engine.clone();
    let head_seq = engine.head_seq();
    if since > head_seq {
        return Err(ApiError::invalid_since(since, head_seq));
    }
    let actor = auth.actor.clone();
    let (page, next_since, scanned) = blocking(&app, Work::Read, move || {
        let mut page =
            engine.events_since_capped(since, limit, MAX_EVENTS_PAGE_BYTES - PAGE_ENVELOPE)?;
        // Where the next page starts: after the last event read, visible or not.
        let next_since = page
            .events
            .last()
            .and_then(|e| e.event.seq)
            .unwrap_or(since);
        let scanned = page.events.len();
        if !visibility::sees_all(&actor) {
            let state = engine.state();
            let gone = engine.whereabouts();
            let scopes = std::mem::take(&mut page.scopes);
            page.events.retain(|e| {
                let scope = e.event.seq.and_then(|seq| scopes.get(&seq));
                visibility::event_visible(&state, &gone, &actor, &e.event, scope)
            });
            for e in &mut page.events {
                e.command_len = None;
            }
        }
        Ok::<_, crate::db::StoreError>((page, next_since, scanned))
    })
    .await??;
    if let Some(seq) = page.oversized {
        tracing::warn!(seq, "an event is larger than the page limit");
        return Err(ApiError::item_too_large("next_since", json!(seq)));
    }
    app.catch_up_marks.paged(&auth.principal, next_since);
    Ok(Json(EventPage {
        has_more: page.truncated || scanned == limit,
        events: page.events,
        next_since,
        head_seq: head_seq.max(next_since),
        epoch: app.engine.epoch(),
    })
    .into_response())
}

/// 409 `epoch_mismatch` unless `epoch` is absent or this log's epoch.
fn check_epoch(engine: &Engine, epoch: Option<uuid::Uuid>) -> Result<(), ApiError> {
    match epoch {
        Some(e) if e != engine.epoch() => Err(ApiError::epoch_mismatch(engine.epoch())),
        _ => Ok(()),
    }
}

type Tx = mpsc::Sender<Queued>;

/// A frame waiting for the client. Event frames for agents carry the event and its scope, so the body
/// can filter them again when it hands them out.
#[derive(Debug)]
pub struct Queued {
    frame: SseEvent,
    check: Option<Box<(Event, EventScope)>>,
}

impl From<SseEvent> for Queued {
    fn from(frame: SseEvent) -> Self {
        Self { frame, check: None }
    }
}

/// The SSE response body. It owns the stream slot, so the slot is freed exactly when the body is
/// dropped, however the connection ends. Once `revoked` is set it ends, dropping any queued frames.
///
/// Frames wait in a queue between the pump's filter and the client. So before handing one out, the
/// body re-checks the caller (`access`): a changed standing ends the stream, and an agent's event
/// frame is filtered again against the engine's state and grants as they are now; one that no longer
/// passes is dropped (as if the pump had skipped it: the next frame's `id` moves past it, the way a
/// JSON page's `next_since` moves past events the agent cannot see).
struct Body {
    rx: ReceiverStream<Queued>,
    revoked: Arc<AtomicBool>,
    access: Option<Caller>,
    _slot: OwnedSemaphorePermit,
}

/// Whose stream it is, and the access it started with.
struct Caller {
    authsvc: Arc<AuthService>,
    engine: Arc<Engine>,
    principal: Principal,
    actor: Actor,
    standing: Option<Standing>,
}

impl Caller {
    /// True if the frame may still go out: the caller's access is unchanged and (for an agent's event
    /// frame) the event is visible now.
    fn still_sees(&self, q: &Queued) -> bool {
        q.check.as_deref().is_none_or(|(event, scope)| {
            visibility::event_visible(
                &self.engine.state(),
                &self.engine.whereabouts(),
                &self.actor,
                event,
                Some(scope),
            )
        })
    }
}

impl Stream for Body {
    type Item = Result<SseEvent, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if this.revoked.load(Ordering::Acquire) {
                return Poll::Ready(None);
            }
            let q = match Pin::new(&mut this.rx).poll_next(cx) {
                Poll::Ready(Some(q)) => q,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            };
            if let Some(c) = &this.access {
                if lost_access(&c.authsvc, &c.principal, c.standing.as_ref()) {
                    this.revoked.store(true, Ordering::Release);
                    return Poll::Ready(None);
                }
                if !c.still_sees(&q) {
                    continue;
                }
            }
            return Poll::Ready(Some(Ok(q.frame)));
        }
    }
}

/// True if `principal` lost access since the stream started with `at_start`: its token was revoked
/// or expired, or (for an agent) its grants changed in any way.
fn lost_access(authsvc: &AuthService, principal: &Principal, at_start: Option<&Standing>) -> bool {
    match (authsvc.standing(principal), at_start) {
        (Some(now), Some(then)) => now != *then,
        _ => true,
    }
}

/// Resolves once `principal` loses access (see [`lost_access`]; checked at every revocation or
/// grant change notice), or if notices stop.
async fn until_revoked(
    notices: &mut watch::Receiver<u64>,
    authsvc: &AuthService,
    principal: &Principal,
    at_start: Option<&Standing>,
) {
    loop {
        if notices.changed().await.is_err() || lost_access(authsvc, principal, at_start) {
            return;
        }
    }
}

/// Why a send did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The client went away.
    Closed,
    /// The server is shutting down.
    Shutdown,
    /// The client did not take the frame within the stall deadline.
    Stalled,
}

/// Sends a frame, waiting at most `stall` for room and giving up at shutdown.
pub async fn send_frame(
    tx: &Tx,
    shutdown: &mut watch::Receiver<bool>,
    stall: Duration,
    frame: impl Into<Queued>,
) -> Result<(), SendError> {
    tokio::select! {
        r = tx.send(frame.into()) => r.map_err(|_| SendError::Closed),
        _ = shutdown.changed() => Err(SendError::Shutdown),
        () = tokio::time::sleep(stall) => Err(SendError::Stalled),
    }
}

struct Pump {
    engine: Arc<Engine>,
    reads: Arc<Semaphore>,
    marks: Arc<CatchUpMarks>,
    principal: Principal,
    /// The principal's access when the stream was opened; any change ends the stream.
    standing: Option<Standing>,
    actor: Actor,
    authsvc: Arc<AuthService>,
    revocations: watch::Receiver<u64>,
    /// Set when the caller lost access: the body drops what is queued.
    revoked: Arc<AtomicBool>,
    shutdown: watch::Receiver<bool>,
    tx: Tx,
    /// The last sequence number delivered (or the starting point).
    last: u64,
    stall: Duration,
    max_events: usize,
    max_bytes: usize,
}

impl Pump {
    /// What the body needs to filter an agent's event frame again (nothing for the owner).
    fn check(&self, event: &Event, scope: Option<&EventScope>) -> Option<Box<(Event, EventScope)>> {
        if visibility::sees_all(&self.actor) {
            return None;
        }
        Some(Box::new((
            event.clone(),
            scope.cloned().unwrap_or_default(),
        )))
    }

    fn lost(&self) -> bool {
        lost_access(&self.authsvc, &self.principal, self.standing.as_ref())
    }

    /// Ends the stream for a caller that lost access, dropping queued frames.
    fn revoke(&self) {
        tracing::debug!("event stream closed: the caller's access changed");
        self.revoked.store(true, Ordering::Release);
    }

    async fn send(&mut self, frame: impl Into<Queued>) -> bool {
        let frame = frame.into();
        let sent = tokio::select! {
            r = send_frame(&self.tx, &mut self.shutdown, self.stall, frame) => r,
            () = until_revoked(&mut self.revocations, &self.authsvc, &self.principal, self.standing.as_ref()) => {
                self.revoke();
                return false;
            }
        };
        match sent {
            Ok(()) => true,
            Err(SendError::Stalled) => {
                tracing::debug!("event stream stalled; closing it");
                false
            }
            Err(_) => false,
        }
    }

    /// Reads a page of the log within `max_bytes`. The read permit is taken first (waiting at most the
    /// stall deadline) and held by the blocking read itself; waiting for either ends early when the
    /// client goes away or the server shuts down.
    async fn read_page(
        &mut self,
        after: u64,
        limit: usize,
        max_bytes: usize,
    ) -> Option<CappedEvents> {
        let permit = tokio::select! {
            p = self.reads.clone().acquire_owned() => p.ok()?,
            () = self.tx.closed() => return None,
            _ = self.shutdown.changed() => return None,
            () = until_revoked(&mut self.revocations, &self.authsvc, &self.principal, self.standing.as_ref()) => {
                self.revoke();
                return None;
            }
            () = tokio::time::sleep(self.stall) => {
                tracing::debug!("event stream: no read permit; closing it");
                return None;
            }
        };
        let engine = self.engine.clone();
        let read = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            engine.events_since_capped(after, limit, max_bytes)
        });
        tokio::select! {
            r = read => match r {
                Ok(Ok(page)) => Some(page),
                Ok(Err(err)) => {
                    tracing::error!(error = %err, "event stream: cannot read events");
                    None
                }
                Err(_) => None,
            },
            () = self.tx.closed() => None,
            _ = self.shutdown.changed() => None,
            () = until_revoked(&mut self.revocations, &self.authsvc, &self.principal, self.standing.as_ref()) => {
                self.revoke();
                None
            }
            () = tokio::time::sleep(self.stall) => {
                tracing::debug!("event stream: a read took too long; closing it");
                None
            }
        }
    }

    /// Tells the client to catch up through JSON pages and marks the principal; the caller then ends
    /// the stream.
    async fn too_far_behind(&mut self, reason: &'static str) {
        self.marks.set(&self.principal, self.last);
        let head_seq = self.engine.head_seq();
        let notice = CatchUp {
            reason,
            since: self.last,
            head_seq,
            hint: format!(
                "page through GET /api/v1/events?since={} up to head_seq, then reconnect from there",
                self.last
            ),
        };
        if let Ok(data) = serde_json::to_string(&notice) {
            let _ = self
                .send(SseEvent::default().event(CATCH_UP).data(data))
                .await;
        }
    }

    /// Sends every stored event after `self.last`, within the replay budget. Returns false if the
    /// stream must end.
    async fn catch_up(&mut self) -> bool {
        if self.engine.head_seq().saturating_sub(self.last) > self.max_events as u64 {
            self.too_far_behind("too_far_behind").await;
            return false;
        }
        let (mut events, mut bytes) = (0usize, 0usize);
        loop {
            let want = REPLAY_PAGE.min(self.max_events - events + 1);
            let budget = self.max_bytes.saturating_sub(bytes);
            let Some(page) = self.read_page(self.last, want, budget).await else {
                return false;
            };
            let full = page.events.len() == want;
            let state = self.engine.state();
            let gone = self.engine.whereabouts();
            for ev in &page.events {
                let Some(seq) = ev.event.seq else { continue };
                let scope = page.scopes.get(&seq);
                if !visibility::event_visible(&state, &gone, &self.actor, &ev.event, scope) {
                    events += 1;
                    self.last = seq;
                    continue;
                }
                let data = match serde_json::to_string(&visibility::log_event_for(&self.actor, ev))
                {
                    Ok(d) => d,
                    Err(err) => {
                        tracing::error!(error = %err, seq, "cannot encode event");
                        return false;
                    }
                };
                events += 1;
                bytes = bytes.saturating_add(data.len());
                if events > self.max_events {
                    self.too_far_behind("too_far_behind").await;
                    return false;
                }
                if bytes > self.max_bytes {
                    self.too_far_behind("too_many_bytes").await;
                    return false;
                }
                let check = self.check(&ev.event, scope);
                if !self
                    .send(Queued {
                        frame: frame(seq, data),
                        check,
                    })
                    .await
                {
                    return false;
                }
                self.last = seq;
            }
            if page.truncated || page.oversized.is_some() {
                self.too_far_behind("too_many_bytes").await;
                return false;
            }
            if !full {
                return true;
            }
        }
    }

    async fn run(mut self, mut live: broadcast::Receiver<Arc<LiveEvent>>, heartbeat: Duration) {
        // Revocation notices are subscribed already (in `stream`): a revocation since the caller was
        // authenticated shows here, and any later one wakes the waits below. Nothing, not even the
        // hello frame, is sent before this check.
        if self.lost() {
            self.revoke();
            return;
        }
        let hello = Hello {
            epoch: self.engine.epoch(),
            head_seq: visibility::sees_all(&self.actor).then(|| self.engine.head_seq()),
        };
        let Ok(data) = serde_json::to_string(&hello) else {
            return;
        };
        if !self.send(SseEvent::default().event(HELLO).data(data)).await {
            return;
        }
        if !self.catch_up().await {
            return;
        }
        if hello.head_seq.is_none()
            && !self
                .send(SseEvent::default().event(BACKLOG_END).data("{}"))
                .await
        {
            return;
        }
        let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
        beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.tx.closed() => return,
                _ = self.shutdown.changed() => return,
                r = self.revocations.changed() => {
                    if r.is_err() || self.lost() {
                        self.revoke();
                        return;
                    }
                }
                _ = beat.tick() => {
                    // Expiry is not announced: check it with every heartbeat.
                    if self.lost() {
                        self.revoke();
                        return;
                    }
                    if !self.send(SseEvent::default().comment(HEARTBEAT)).await {
                        return;
                    }
                }
                msg = live.recv() => match msg {
                    Ok(live_event) => {
                        let ev = &live_event.event;
                        let Some(seq) = ev.event.seq else { continue };
                        if seq <= self.last {
                            continue;
                        }
                        if seq > self.last + 1 {
                            // A gap: re-read from the database (which includes this event).
                            if !self.catch_up().await {
                                return;
                            }
                            continue;
                        }
                        if !visibility::event_visible(
                            &self.engine.state(),
                            &self.engine.whereabouts(),
                            &self.actor,
                            &ev.event,
                            Some(&live_event.scope),
                        ) {
                            self.last = seq;
                            continue;
                        }
                        let data = match serde_json::to_string(&visibility::log_event_for(&self.actor, ev)) {
                            Ok(d) => d,
                            Err(err) => {
                                tracing::error!(error = %err, seq, "cannot encode event");
                                return;
                            }
                        };
                        if data.len() > self.max_bytes {
                            self.too_far_behind("too_many_bytes").await;
                            return;
                        }
                        let check = self.check(&ev.event, Some(&live_event.scope));
                        if !self.send(Queued { frame: frame(seq, data), check }).await {
                            return;
                        }
                        self.last = seq;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "event stream lagged; re-reading from the database");
                        if !self.catch_up().await {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    }
}

fn frame(seq: u64, data: String) -> SseEvent {
    SseEvent::default()
        .id(seq.to_string())
        .event("event")
        .data(data)
}

fn stream(
    app: &AppState,
    principal: Principal,
    actor: Actor,
    since: u64,
    slot: OwnedSemaphorePermit,
) -> impl IntoResponse + use<> {
    let (tx, rx) = mpsc::channel(CLIENT_BUFFER);
    // Subscribe before replaying so nothing committed in between is missed, and to revocations before
    // the pump's first access check.
    let live = app.engine.subscribe();
    let revocations = app.authsvc.revocations();
    let revoked = Arc::new(AtomicBool::new(false));
    let mut shutdown = app.shutdown.clone();
    shutdown.mark_unchanged();
    let standing = app.authsvc.standing(&principal);
    let caller = Caller {
        authsvc: app.authsvc.clone(),
        engine: app.engine.clone(),
        principal: principal.clone(),
        actor: actor.clone(),
        standing: standing.clone(),
    };
    let pump = Pump {
        engine: app.engine.clone(),
        reads: app.reads.clone(),
        marks: app.catch_up_marks.clone(),
        standing,
        principal,
        actor,
        authsvc: app.authsvc.clone(),
        revocations,
        revoked: revoked.clone(),
        shutdown,
        tx,
        last: since,
        stall: app.limits.sse_stall_timeout(),
        max_events: app.limits.max_replay_events,
        max_bytes: app.limits.max_replay_bytes,
    };
    let epoch = app.engine.epoch();
    tokio::spawn(pump.run(live, app.limits.sse_heartbeat()));
    let epoch = HeaderValue::from_str(&epoch.to_string()).ok();
    (
        epoch
            .map(|v| [(HeaderName::from_static(LOG_EPOCH_HEADER), v)])
            .into_iter()
            .flatten()
            .collect::<HeaderMap>(),
        Sse::new(Body {
            rx: ReceiverStream::new(rx),
            revoked,
            access: Some(caller),
            _slot: slot,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_send_to_a_stuck_client_gives_up_after_the_stall_deadline() {
        let (tx, _rx) = mpsc::channel(1);
        let (_stop, mut shutdown) = watch::channel(false);
        let stall = Duration::from_millis(100);
        send_frame(&tx, &mut shutdown, stall, SseEvent::default().data("1"))
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let r = send_frame(&tx, &mut shutdown, stall, SseEvent::default().data("2")).await;
        assert_eq!(r, Err(SendError::Stalled));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    struct Rig {
        _dir: tempfile::TempDir,
        engine: Arc<Engine>,
        svc: Arc<AuthService>,
        token_id: i64,
        principal: Principal,
        shutdown: watch::Sender<bool>,
    }

    /// An engine with `n` tasks in a list, and an agent that can read everything.
    fn rig(n: usize) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::Config {
            data_dir: dir.path().to_owned(),
            ..crate::Config::default()
        };
        let engine =
            Arc::new(Engine::open(&config.database_path(), crate::engine::system_clock()).unwrap());
        let svc = Arc::new(AuthService::new(&config, engine.clone(), None).unwrap());
        let agent = todo_core::AgentId::new("bot").unwrap();
        let (token_id, _) = svc
            .create_agent(&agent, todo_core::AccessLevel::Read, None, None, None)
            .unwrap();
        let owner = Actor::owner("pc").unwrap();
        let list = uuid::Uuid::new_v4();
        let run = |cmd: serde_json::Value| {
            let cmd = serde_json::from_value(cmd).unwrap();
            engine.execute(&owner, uuid::Uuid::new_v4(), &cmd).unwrap();
        };
        run(json!({ "cmd": "create_list", "id": list, "name": "l" }));
        for i in 0..n {
            run(
                json!({ "cmd": "create_task", "id": uuid::Uuid::new_v4(), "list": list, "title": format!("t{i}") }),
            );
        }
        Rig {
            _dir: dir,
            engine,
            svc,
            token_id,
            principal: Principal::Agent {
                agent,
                token: token_id,
            },
            shutdown: watch::channel(false).0,
        }
    }

    impl Rig {
        /// The body's view of the rig's agent, with its standing as of now.
        fn caller(&self) -> Caller {
            Caller {
                authsvc: self.svc.clone(),
                engine: self.engine.clone(),
                principal: self.principal.clone(),
                actor: Actor::agent("bot").unwrap(),
                standing: self.svc.standing(&self.principal),
            }
        }
    }

    /// A pump for the rig's agent, subscribed as `stream` does.
    fn pump(
        r: &Rig,
        tx: Tx,
        stall: Duration,
    ) -> (Pump, Arc<AtomicBool>, broadcast::Receiver<Arc<LiveEvent>>) {
        let live = r.engine.subscribe();
        let revoked = Arc::new(AtomicBool::new(false));
        let shutdown = r.shutdown.subscribe();
        let pump = Pump {
            engine: r.engine.clone(),
            reads: Arc::new(Semaphore::new(4)),
            marks: Arc::new(CatchUpMarks::default()),
            standing: r.svc.standing(&r.principal),
            principal: r.principal.clone(),
            actor: Actor::agent("bot").unwrap(),
            authsvc: r.svc.clone(),
            revocations: r.svc.revocations(),
            revoked: revoked.clone(),
            shutdown,
            tx,
            last: 0,
            stall,
            max_events: 1000,
            max_bytes: 64 * 1024 * 1024,
        };
        (pump, revoked, live)
    }

    #[tokio::test]
    async fn a_revocation_before_the_stream_starts_is_not_missed() {
        let r = rig(3);
        let (tx, mut rx) = mpsc::channel(64);
        let (pump, revoked, live) = pump(&r, tx, Duration::from_secs(3600));
        // Revoked after the request was authenticated, before the pump runs.
        assert!(r.svc.revoke_agent(r.token_id, None).unwrap());
        tokio::time::timeout(
            Duration::from_secs(2),
            pump.run(live, Duration::from_secs(3600)),
        )
        .await
        .unwrap();
        assert!(revoked.load(Ordering::Acquire));
        assert!(rx.try_recv().is_err(), "nothing was sent, not even hello");
    }

    #[tokio::test]
    async fn an_agent_stream_marks_backlog_end_without_exposing_the_head() {
        let r = rig(3);
        let (tx, mut rx) = mpsc::channel(64);
        let (pump, _, live) = pump(&r, tx, Duration::from_secs(3600));
        let task = tokio::spawn(pump.run(live, Duration::from_secs(3600)));
        let hello = rx.recv().await.unwrap().frame;
        let text = format!("{hello:?}");
        assert!(
            text.contains("epoch") && !text.contains("head_seq"),
            "{text}"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            for _ in 0..4 {
                let frame = rx.recv().await.unwrap();
                assert!(
                    frame.check.is_some(),
                    "a list and three tasks precede the marker"
                );
            }
            let marker = rx.recv().await.unwrap();
            let text = format!("{:?}", marker.frame);
            assert!(text.contains(BACKLOG_END), "{text}");
            assert!(marker.check.is_none());
            assert!(!text.contains("head_seq") && !text.contains("id:"));
        })
        .await
        .expect("received replay and its end marker");
        task.abort();
    }

    #[tokio::test]
    async fn a_replay_blocked_on_a_slow_client_stops_at_revocation() {
        let r = rig(20);
        // Room for one frame, and nobody reading: the replay blocks on its second send.
        let (tx, _rx) = mpsc::channel(1);
        let (pump, revoked, live) = pump(&r, tx, Duration::from_secs(3600));
        let task = tokio::spawn(pump.run(live, Duration::from_secs(3600)));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!task.is_finished());
        assert!(r.svc.revoke_agent(r.token_id, None).unwrap());
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("the pump stopped at the revocation")
            .unwrap();
        assert!(
            revoked.load(Ordering::Acquire),
            "queued frames will be dropped"
        );
    }

    #[tokio::test]
    async fn a_revoked_body_drops_queued_frames() {
        use futures_util::StreamExt as _;
        let (tx, rx) = mpsc::channel(4);
        for i in 0..3 {
            tx.send(SseEvent::default().data(i.to_string()).into())
                .await
                .unwrap();
        }
        let revoked = Arc::new(AtomicBool::new(false));
        let slot = Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap();
        let mut body = Body {
            rx: ReceiverStream::new(rx),
            revoked: revoked.clone(),
            access: None,
            _slot: slot,
        };
        assert!(body.next().await.is_some());
        revoked.store(true, Ordering::Release);
        assert!(body.next().await.is_none(), "queued frames are dropped");
    }

    #[tokio::test]
    async fn lowering_grants_drops_frames_already_queued() {
        use futures_util::StreamExt as _;
        let r = rig(0);
        let (tx, rx) = mpsc::channel(4);
        for i in 0..3 {
            tx.send(SseEvent::default().data(i.to_string()).into())
                .await
                .unwrap();
        }
        let slot = Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap();
        let mut body = Body {
            rx: ReceiverStream::new(rx),
            revoked: Arc::new(AtomicBool::new(false)),
            access: Some(r.caller()),
            _slot: slot,
        };
        assert!(body.next().await.is_some());
        // Filtered under Read everywhere; now the agent may read nothing. Nothing else is delivered,
        // although no pump has noticed yet.
        let none = todo_core::Grants {
            default: todo_core::AccessLevel::None,
            ..todo_core::Grants::default()
        };
        assert!(r.svc.set_grants(r.token_id, &none, None).unwrap());
        assert!(body.next().await.is_none(), "queued frames are dropped");
    }

    /// Runs owner commands on the rig's engine.
    fn owner_runs(r: &Rig, cmds: &[serde_json::Value]) {
        let owner = Actor::owner("pc").unwrap();
        for c in cmds {
            let cmd = serde_json::from_value(c.clone()).unwrap();
            r.engine
                .execute(&owner, uuid::Uuid::new_v4(), &cmd)
                .unwrap();
        }
    }

    /// The agent's event frames after `since`, queued as the pump queues them (filtered now).
    async fn queue_events(r: &Rig, since: u64, tx: &Tx) -> usize {
        let page = r.engine.events_since_capped(since, 100, 1 << 20).unwrap();
        let (state, gone) = (r.engine.state(), r.engine.whereabouts());
        let actor = Actor::agent("bot").unwrap();
        let mut n = 0;
        for ev in &page.events {
            let seq = ev.event.seq.unwrap();
            let scope = page.scopes.get(&seq);
            if !visibility::event_visible(&state, &gone, &actor, &ev.event, scope) {
                continue;
            }
            let check = Some(Box::new((ev.event.clone(), scope.cloned().unwrap())));
            let data = serde_json::to_string(&visibility::log_event_for(&actor, ev)).unwrap();
            tx.send(Queued {
                frame: frame(seq, data),
                check,
            })
            .await
            .unwrap();
            n += 1;
        }
        n
    }

    fn private(list: uuid::Uuid) -> todo_core::Grants {
        let mut g = todo_core::Grants {
            default: todo_core::AccessLevel::Read,
            ..todo_core::Grants::default()
        };
        g.lists.insert(
            serde_json::from_value(json!(list)).unwrap(),
            todo_core::AccessLevel::None,
        );
        g
    }

    #[tokio::test]
    async fn a_queued_frame_is_dropped_if_its_task_moved_out_of_sight_meanwhile() {
        use futures_util::StreamExt as _;
        let r = rig(0);
        let (open, hidden) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        owner_runs(
            &r,
            &[
                json!({ "cmd": "create_list", "id": open, "name": "open" }),
                json!({ "cmd": "create_list", "id": hidden, "name": "hidden" }),
            ],
        );
        assert!(
            r.svc
                .set_grants(r.token_id, &private(hidden), None)
                .unwrap()
        );
        let since = r.engine.head_seq();
        let (moves, stays) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        owner_runs(
            &r,
            &[
                json!({ "cmd": "create_task", "id": moves, "list": open, "title": "MOVES" }),
                json!({ "cmd": "create_task", "id": stays, "list": open, "title": "STAYS" }),
            ],
        );
        // Both creations pass the filter and wait in the queue (the client is not reading).
        let (tx, rx) = mpsc::channel(8);
        assert_eq!(queue_events(&r, since, &tx).await, 2);
        drop(tx);
        let mut body = Body {
            rx: ReceiverStream::new(rx),
            revoked: Arc::new(AtomicBool::new(false)),
            access: Some(r.caller()),
            _slot: Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap(),
        };
        // Meanwhile the owner moves one task into the private list.
        owner_runs(
            &r,
            &[json!({ "cmd": "move_task", "id": moves, "list": hidden })],
        );
        let out: Vec<String> = body
            .by_ref()
            .map(|f| format!("{:?}", f.unwrap()))
            .collect()
            .await;
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(
            out[0].contains("STAYS") && !out[0].contains("MOVES"),
            "{out:?}"
        );
    }

    #[tokio::test]
    async fn the_engine_filters_with_new_grants_before_any_stream_sees_them() {
        use futures_util::StreamExt as _;
        let r = rig(0);
        let hidden = uuid::Uuid::new_v4();
        let since = r.engine.head_seq();
        owner_runs(
            &r,
            &[
                json!({ "cmd": "create_list", "id": hidden, "name": "soon hidden" }),
                json!({ "cmd": "create_task", "id": uuid::Uuid::new_v4(), "list": hidden, "title": "SECRET" }),
            ],
        );
        // Filtered under the old grants (everything readable): queued.
        let (tx, rx) = mpsc::channel(8);
        assert_eq!(queue_events(&r, since, &tx).await, 2);
        drop(tx);
        let before = r.svc.standing(&r.principal);

        // Wait for the engine update, but keep the auth snapshot paused until we read it.
        let (paused, resume) = r.svc.pause_next_publish();
        let (svc, token) = (r.svc.clone(), r.token_id);
        let reduce = std::thread::spawn(move || svc.set_grants(token, &private(hidden), None));
        tokio::time::timeout(Duration::from_secs(10), paused)
            .await
            .unwrap()
            .unwrap();
        let agent = todo_core::AgentId::new("bot").unwrap();
        assert_eq!(
            r.engine.state().grants.get(&agent),
            Some(&private(hidden)),
            "the engine has the new grants first"
        );
        assert_eq!(
            r.svc.standing(&r.principal),
            before,
            "streams still see the old standing, which ends them at the swap"
        );
        // A stream that starts now records the old standing.
        let during = r.caller();
        drop(resume);
        assert!(reduce.join().unwrap().unwrap());

        // Whether the stream started during the publish or after it, the queued frames about the
        // now-hidden list are not delivered.
        let mut started_during = Body {
            rx: ReceiverStream::new(rx),
            revoked: Arc::new(AtomicBool::new(false)),
            access: Some(during),
            _slot: Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap(),
        };
        assert!(started_during.next().await.is_none());

        let (tx, rx) = mpsc::channel(8);
        // Queue them as if filtered under the old grants, for a stream with the new standing.
        let page = r.engine.events_since_capped(since, 100, 1 << 20).unwrap();
        for ev in &page.events {
            let seq = ev.event.seq.unwrap();
            let check = Some(Box::new((ev.event.clone(), page.scopes[&seq].clone())));
            let data = serde_json::to_string(ev).unwrap();
            tx.send(Queued {
                frame: frame(seq, data),
                check,
            })
            .await
            .unwrap();
        }
        drop(tx);
        let started_after = Body {
            rx: ReceiverStream::new(rx),
            revoked: Arc::new(AtomicBool::new(false)),
            access: Some(r.caller()),
            _slot: Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap(),
        };
        let out: Vec<_> = started_after.collect().await;
        assert!(out.is_empty(), "{out:?}");
    }

    #[tokio::test]
    async fn a_grant_change_ends_the_stream() {
        let r = rig(1);
        let (tx, mut rx) = mpsc::channel(64);
        let (pump, revoked, live) = pump(&r, tx, Duration::from_secs(3600));
        let task = tokio::spawn(pump.run(live, Duration::from_secs(3600)));
        // The hello frame and the replay arrive; then the stream waits for live events.
        assert!(rx.recv().await.is_some());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!task.is_finished());
        let more = todo_core::Grants {
            default: todo_core::AccessLevel::Plan,
            ..todo_core::Grants::default()
        };
        assert!(r.svc.set_grants(r.token_id, &more, None).unwrap());
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("the stream ended at the grant change")
            .unwrap();
        assert!(revoked.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn a_blocked_send_ends_at_shutdown() {
        let (tx, _rx) = mpsc::channel(1);
        let (stop, mut shutdown) = watch::channel(false);
        tx.send(SseEvent::default().data("1").into()).await.unwrap();
        let send = tokio::spawn(async move {
            send_frame(
                &tx,
                &mut shutdown,
                Duration::from_secs(3600),
                SseEvent::default().data("2"),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.send(true).unwrap();
        let r = tokio::time::timeout(Duration::from_secs(2), send)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r, Err(SendError::Shutdown));
    }
}
