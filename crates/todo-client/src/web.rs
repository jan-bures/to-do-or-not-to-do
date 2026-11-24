//! Browser implementations of the platform traits (feature `web`, `wasm32` only): the web app's
//! [`FetchTransport`], [`BrowserClock`], [`BrowserEntropy`], and an IndexedDB backend for
//! [`MirrorStorage`].
//!
//! # Transport and authentication
//!
//! The web app is served by the server it talks to, so every request is same-origin and carries the
//! owner's session cookie (`credentials: same-origin`); no token is ever stored in the browser. Each
//! request also carries the session's CSRF token in `X-CSRF-Token`, which the server requires on
//! every session request (see `todo_server::auth::require_auth_or_session`). The browser adds
//! `Origin` to writes itself.
//!
//! **A stale token.** The session behind the cookie can change while a tab is open (the owner signs
//! in again in another tab, which replaces the cookie). The tab's token then no longer matches, and
//! the server answers `403 csrf`. The transport asks for the session's current token
//! ([`CSRF_PATH`]) and, if it differs from the token the refused request was sent with, sends the
//! same request once more with it (same body, so the same command ID: the server applies it once).
//! Comparing with the token *sent*, not the shared one, matters when several requests are refused at
//! once: see [`crate::session::with_fresh_token`]. If the token is unchanged, the refusal was about
//! something else (the `Origin`), and the `403` stands; the engine retries it later with backoff
//! (it is never a verdict on a command). If the refresh itself fails, the request counts as a network
//! failure and is retried later; a signed-out session is a `401`.
//!
//! The event stream is a streaming `fetch` read through [`SseParser`], not `EventSource`:
//! `EventSource` can send no custom header (so no CSRF token), hides the status of a refused stream
//! (the engine needs `409 catch_up_required`, `epoch_mismatch` and `Retry-After`), and reconnects on
//! its own, outside the engine's backoff and idle timeout.
//!
//! # Storage
//!
//! [`open_storage`] reads the IndexedDB database into a [`MirrorStorage`] and returns an
//! [`IdbFlusher`] whose [`run`](IdbFlusher::run) persists every write, one IndexedDB transaction per
//! batch, in order. The app spawns it next to the sync client.
//!
//! **Durability.** A batch leaves the journal only once its transaction's `complete` fired, and an
//! outbox entry is not sent before then (see [`crate::mirror`]): an edit made offline is either in
//! IndexedDB or still shown as unsaved, never "queued" in memory only. A failed write (out of quota,
//! say) is recorded ([`MirrorStorage::flush_error`]) and retried every second; the app shows it until
//! a write succeeds, and asks before the page is closed while anything is unsaved.
//!
//! **Upgrades.** Opening the database at a newer version waits for every other connection to close.
//! [`OpenHooks::blocked`] is told while it waits (the app asks the owner to close other tabs). Every
//! connection closes itself when another tab needs a newer version ([`OpenHooks::version_change`]
//! first, so the app stops making changes), but only once everything it wrote is in the database:
//! while a write is still failing it keeps the connection open, which keeps blocking the upgrade, and
//! [`OpenHooks::holding`] says so. A closed connection is reported to [`OpenHooks::closed`] (the app
//! gives up the sender lock and reloads). Version 1 kept the outbox and
//! the failed list as arrays in `kv`; they are split into records by the first write after the
//! upgrade, in one transaction that puts every record before it deletes the array, keyed by command
//! ID, so an interrupted conversion leaves either the array or the records and is simply done again.
//!
//! # Several tabs
//!
//! Every tab has its own replica over the same database. So that commands from all tabs reach the
//! server in the order they were made, one tab sends for all of them: the holder of the Web Lock
//! [`SENDER_LOCK`] ([`elect_sender`]). The others write their commands to the `outbox` store and say
//! so on the `BroadcastChannel` [`CHANNEL`] (the flusher posts after every committed batch that
//! touched the outbox or the failed list); the holder reads the store ([`IdbFlusher::queued`]) and
//! takes their entries in (`SyncClient::adopt`), in submission order. When the holder's tab closes,
//! the browser hands the lock to the next tab, which takes in whatever is still queued and carries
//! on. Every tab still follows the event stream, which confirms its own entries. A browser without
//! Web Locks sends from every tab, as before: each tab's commands stay in order, but two tabs' edits
//! to one task made while both are offline may reach the server in either order.
//!
//! The database has one object store per part of [`Persisted`]: `kv` (snapshot and epoch), `events`,
//! `outbox` and `failed` (one record per command, so tabs never overwrite each other's), and
//! `damaged` (records that could not be read, set aside). Nothing here ever deletes the database on
//! its own: [`delete_storage`] is for an explicit "reset local data" by the user.

use crate::mirror::{Area, Batch, Change, Key, MirrorStorage, Persisted, Recovery};
use crate::outbox::{FailedEntry, OutboxEntry};
use crate::platform::{
    Clock, Entropy, EventStream, Method, Request, Response, SseFrame, StreamOpen, Transport,
    TransportError,
};
use crate::session::{Attempt, Fresh, is_csrf_refusal, with_fresh_token};
use crate::sse::{DEFAULT_MAX_FRAME_BYTES, SseParser};
use js_sys::{Array, Promise, Reflect, Uint8Array};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use todo_core::ids::{AttachmentId, TaskId};
use todo_core::time::Timestamp;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, BroadcastChannel, Headers, IdbDatabase, IdbObjectStore, IdbOpenDbRequest,
    IdbRequest, IdbTransaction, IdbTransactionMode, ReadableStreamDefaultReader,
    RequestCredentials, RequestInit, RequestMode,
};

/// The header carrying the session's CSRF token.
pub const CSRF_HEADER: &str = "X-CSRF-Token";

/// Where the session's current CSRF token and owner scope are read (`GET`, the cookie only).
pub const CSRF_PATH: &str = "/auth/session";

pub use crate::session::CsrfToken;

fn js_err(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| {
            e.dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
        })
        .unwrap_or_else(|| format!("{e:?}"))
}

fn window() -> Result<web_sys::Window, TransportError> {
    web_sys::window().ok_or_else(|| TransportError::Network("no window".into()))
}

// ---- transport ---------------------------------------------------------------------------------

/// `fetch` with the session cookie and the CSRF token.
#[derive(Clone)]
pub struct FetchTransport {
    csrf: CsrfToken,
    owner_scope: Option<String>,
    max_frame_bytes: usize,
}

impl fmt::Debug for FetchTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the token.
        f.debug_struct("FetchTransport").finish_non_exhaustive()
    }
}

impl FetchTransport {
    /// A transport for this page's origin, sending `csrf` with every request (and updating it when
    /// the session's token changes).
    #[must_use]
    pub fn new(csrf: CsrfToken) -> Self {
        Self {
            csrf,
            owner_scope: None,
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }

    /// The signed-in owner of this document. A token for a different owner must not revive it.
    #[must_use]
    pub fn with_owner_scope(mut self, owner_scope: Option<String>) -> Self {
        self.owner_scope = owner_scope;
        self
    }

    /// Uploads a browser file as a raw body, without buffering its bytes in wasm memory or queuing
    /// them in the replica. A refresh resends the same file with the same `attachment` ID.
    /// The event stream brings the new attachment into the replica.
    pub async fn upload_attachment(
        &self,
        task: TaskId,
        attachment: AttachmentId,
        file: &web_sys::File,
    ) -> Result<Response, TransportError> {
        let path = crate::platform::attachment_upload_path(task, attachment, &file.name());
        let media = file.type_();
        let media = if media.is_empty() {
            "application/octet-stream"
        } else {
            &media
        };
        self.send_body(&path, Method::Post, Some((file.as_ref(), media)))
            .await
    }

    /// Asks the server for the session's current token (see the module docs).
    async fn refresh(&self) -> Result<Fresh, TransportError> {
        let abort = new_abort()?;
        // The refresh needs no token, but sending the current one does no harm.
        let res = read(
            &Self::fetch(
                CSRF_PATH,
                Method::Get,
                None,
                &self.csrf.get(),
                "application/json",
                &abort.0,
            )
            .await?,
        )
        .await?;
        match res.status {
            200 => {
                let body: serde_json::Value = serde_json::from_slice(&res.body)
                    .map_err(|_| TransportError::Protocol("unreadable session token".into()))?;
                if let Some(expected) = &self.owner_scope
                    && body.get("owner_scope").and_then(serde_json::Value::as_str)
                        != Some(expected.as_str())
                {
                    return Ok(Fresh::SignedOut(Response {
                        status: 401,
                        body: Vec::new(),
                        retry_after: None,
                    }));
                }
                body.get("csrf")
                    .and_then(serde_json::Value::as_str)
                    .filter(|t| !t.is_empty())
                    .map(|token| Fresh::Token(token.to_owned()))
                    .ok_or_else(|| TransportError::Protocol("unreadable session token".into()))
            }
            401 => Ok(Fresh::SignedOut(res)),
            s => Err(TransportError::Network(format!(
                "could not refresh the session token ({s})"
            ))),
        }
    }

    async fn send_body(
        &self,
        path: &str,
        method: Method,
        body: Option<(&JsValue, &str)>,
    ) -> Result<Response, TransportError> {
        with_fresh_token(
            &self.csrf,
            |token| async move {
                let abort = new_abort()?;
                let res = read(
                    &Self::fetch(path, method, body, &token, "application/json", &abort.0).await?,
                )
                .await?;
                Ok(if is_csrf_refusal(&res) {
                    Attempt::CsrfRefused(res)
                } else {
                    Attempt::Done(res)
                })
            },
            || self.refresh(),
            |signed_out| signed_out,
        )
        .await
    }

    async fn fetch(
        path: &str,
        method: Method,
        body: Option<(&JsValue, &str)>,
        token: &str,
        accept: &str,
        abort: &AbortController,
    ) -> Result<web_sys::Response, TransportError> {
        let headers = Headers::new().map_err(|e| TransportError::Network(js_err(&e)))?;
        let set = |k: &str, v: &str| {
            headers
                .set(k, v)
                .map_err(|e| TransportError::Network(js_err(&e)))
        };
        set(CSRF_HEADER, token)?;
        set("Accept", accept)?;
        let init = RequestInit::new();
        init.set_credentials(RequestCredentials::SameOrigin);
        init.set_mode(RequestMode::SameOrigin);
        init.set_signal(Some(&abort.signal()));
        match method {
            Method::Get => init.set_method("GET"),
            Method::Post => init.set_method("POST"),
            Method::Put => init.set_method("PUT"),
            Method::Delete => init.set_method("DELETE"),
        }
        if let Some((body, content_type)) = body {
            set("Content-Type", content_type)?;
            init.set_body(body);
        }
        init.set_headers(&headers);
        let req = web_sys::Request::new_with_str_and_init(path, &init)
            .map_err(|e| TransportError::Network(js_err(&e)))?;
        let res = JsFuture::from(window()?.fetch_with_request(&req))
            .await
            .map_err(|e| TransportError::Network(js_err(&e)))?;
        res.dyn_into::<web_sys::Response>()
            .map_err(|_| TransportError::Protocol("fetch did not return a Response".into()))
    }
}

/// One try at opening the event stream.
enum Opened {
    Open(web_sys::Response, Abort),
    Refused(Response),
}

/// Aborts a `fetch` when dropped (the engine gave up on it, or the stream ended).
struct Abort(AbortController);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn new_abort() -> Result<Abort, TransportError> {
    AbortController::new()
        .map(Abort)
        .map_err(|e| TransportError::Network(js_err(&e)))
}

async fn read(res: &web_sys::Response) -> Result<Response, TransportError> {
    let retry_after = res
        .headers()
        .get("retry-after")
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let status = res.status();
    let body = if status == 401 {
        // A dead session needs no error body. Do not lose the status if reading it fails.
        Vec::new()
    } else {
        let buf = JsFuture::from(
            res.array_buffer()
                .map_err(|e| TransportError::Network(js_err(&e)))?,
        )
        .await
        .map_err(|e| TransportError::Network(js_err(&e)))?;
        Uint8Array::new(&buf).to_vec()
    };
    Ok(Response {
        status,
        retry_after,
        body,
    })
}

/// A streaming `fetch` read as Server-Sent Events.
pub struct FetchEventStream {
    reader: ReadableStreamDefaultReader,
    parser: SseParser,
    queue: VecDeque<SseFrame>,
    _abort: Abort,
}

impl fmt::Debug for FetchEventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FetchEventStream")
            .field("queued", &self.queue.len())
            .finish_non_exhaustive()
    }
}

impl Drop for FetchEventStream {
    fn drop(&mut self) {
        let _ = self.reader.cancel();
    }
}

impl EventStream for FetchEventStream {
    async fn next_frame(&mut self) -> Option<Result<SseFrame, TransportError>> {
        loop {
            if let Some(frame) = self.queue.pop_front() {
                return Some(Ok(frame));
            }
            let chunk = match JsFuture::from(self.reader.read()).await {
                Ok(c) => c,
                Err(e) => return Some(Err(TransportError::Network(js_err(&e)))),
            };
            let done = Reflect::get(&chunk, &JsValue::from_str("done"))
                .ok()
                .and_then(|d| d.as_bool())
                .unwrap_or(true);
            if done {
                return None;
            }
            let Ok(value) = Reflect::get(&chunk, &JsValue::from_str("value")) else {
                return None;
            };
            let bytes = Uint8Array::new(&value).to_vec();
            match self.parser.push(&bytes) {
                Ok(frames) => self.queue.extend(frames),
                Err(e) => return Some(Err(TransportError::Protocol(e.to_string()))),
            }
        }
    }
}

impl Transport for FetchTransport {
    type Stream = FetchEventStream;

    async fn send(&self, request: Request) -> Result<Response, TransportError> {
        let json = request.body.as_deref().map(Uint8Array::from);
        let body = json.as_ref().map(|b| (b.as_ref(), "application/json"));
        self.send_body(&request.path, request.method, body).await
    }

    async fn open_stream(
        &self,
        request: Request,
    ) -> Result<StreamOpen<FetchEventStream>, TransportError> {
        let path = request.path.as_str();
        let method = request.method;
        let json = request.body.as_deref().map(Uint8Array::from);
        let body = json.as_ref().map(|b| (b.as_ref(), "application/json"));
        let opened = with_fresh_token(
            &self.csrf,
            |token| async move {
                let abort = new_abort()?;
                let res =
                    Self::fetch(path, method, body, &token, "text/event-stream", &abort.0).await?;
                if res.status() == 200 {
                    return Ok(Attempt::Done(Opened::Open(res, abort)));
                }
                let refused = read(&res).await?;
                Ok(if is_csrf_refusal(&refused) {
                    Attempt::CsrfRefused(Opened::Refused(refused))
                } else {
                    Attempt::Done(Opened::Refused(refused))
                })
            },
            || self.refresh(),
            Opened::Refused,
        )
        .await?;
        let (res, abort) = match opened {
            Opened::Open(res, abort) => (res, abort),
            Opened::Refused(r) => return Ok(StreamOpen::Refused(r)),
        };
        let body = res
            .body()
            .ok_or_else(|| TransportError::Protocol("the event stream has no body".into()))?;
        let reader = body
            .get_reader()
            .dyn_into::<ReadableStreamDefaultReader>()
            .map_err(|_| TransportError::Protocol("no stream reader".into()))?;
        Ok(StreamOpen::Open(FetchEventStream {
            reader,
            parser: SseParser::with_max_frame_bytes(self.max_frame_bytes),
            queue: VecDeque::new(),
            _abort: abort,
        }))
    }
}

// ---- clock and entropy -------------------------------------------------------------------------

/// `Date.now()` and `setTimeout`.
#[derive(Clone, Copy, Debug, Default)]
pub struct BrowserClock;

/// The browser's wall clock as a [`Timestamp`].
#[must_use]
pub fn now() -> Timestamp {
    #[allow(clippy::cast_possible_truncation)]
    let ms = js_sys::Date::now() as i64;
    Timestamp::from_millisecond(ms).unwrap_or(Timestamp::UNIX_EPOCH)
}

#[derive(Default)]
struct SleepState {
    done: bool,
    waker: Option<Waker>,
}

/// Completes after a `setTimeout`; clears the timer when dropped early.
pub struct Sleep {
    state: Rc<RefCell<SleepState>>,
    id: Option<i32>,
    _callback: Option<Closure<dyn FnMut()>>,
}

impl fmt::Debug for Sleep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sleep").field("id", &self.id).finish()
    }
}

/// Sleeps for `duration` on the browser's event loop.
#[must_use]
pub fn sleep(duration: Duration) -> Sleep {
    let state = Rc::new(RefCell::new(SleepState::default()));
    let s = state.clone();
    let callback = Closure::<dyn FnMut()>::new(move || {
        let mut g = s.borrow_mut();
        g.done = true;
        if let Some(w) = g.waker.take() {
            w.wake();
        }
    });
    let ms = i32::try_from(duration.as_millis()).unwrap_or(i32::MAX);
    let id = web_sys::window().and_then(|w| {
        w.set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            ms,
        )
        .ok()
    });
    if id.is_none() {
        state.borrow_mut().done = true;
    }
    Sleep {
        state,
        id,
        _callback: Some(callback),
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut g = self.state.borrow_mut();
        if g.done {
            Poll::Ready(())
        } else {
            g.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let (Some(id), false) = (self.id, self.state.borrow().done)
            && let Some(w) = web_sys::window()
        {
            w.clear_timeout_with_handle(id);
        }
    }
}

impl Clock for BrowserClock {
    fn now(&self) -> Timestamp {
        now()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        sleep(duration)
    }
}

/// `crypto.getRandomValues`.
#[derive(Clone, Copy, Debug, Default)]
pub struct BrowserEntropy;

impl Entropy for BrowserEntropy {
    fn fill(&self, buf: &mut [u8]) {
        let ok = web_sys::window()
            .and_then(|w| w.crypto().ok())
            .is_some_and(|c| c.get_random_values_with_u8_array(buf).is_ok());
        if !ok {
            // No Web Crypto (never in a supported browser): Math.random is still per-device.
            for b in buf.iter_mut() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    *b = (js_sys::Math::random() * 256.0) as u8;
                }
            }
        }
    }
}

// ---- IndexedDB -----------------------------------------------------------------------------------

/// The IndexedDB database's version. 2 split the outbox and failed list into per-command records
/// and added the damaged area. 3 has the same stores but retires older senders before a tab can
/// queue a guarded move: older builds deserialize `MoveTask` without `from_list` and drop the guard.
/// 4 retires senders that cannot decode bulk task commands; 5 adds bulk reorder.
/// 6 adds workflow commands and resets the confirmed cache to replay workflow fields.
const DB_VERSION: u32 = 6;
const KV: &str = "kv";
const EVENTS: &str = "events";
const OUTBOX: &str = Area::Outbox.name();
const FAILED: &str = Area::Failed.name();
const DAMAGED: &str = "damaged";
const STORES: [&str; 5] = [KV, EVENTS, OUTBOX, FAILED, DAMAGED];

fn store_names() -> Array {
    STORES.iter().map(|s| JsValue::from_str(s)).collect()
}

/// A storage problem in the browser.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("IndexedDB: {0}")]
pub struct IdbError(pub String);

fn idb_err(e: &JsValue) -> IdbError {
    IdbError(js_err(e))
}

/// Resolves when an IndexedDB request succeeds, with its result.
async fn request_done(req: &IdbRequest) -> Result<JsValue, IdbError> {
    let promise = Promise::new(&mut |resolve, reject| {
        let r = req.clone();
        let ok = Closure::once_into_js(move || {
            let v = r.result().unwrap_or(JsValue::UNDEFINED);
            let _ = resolve.call1(&JsValue::NULL, &v);
        });
        let r = req.clone();
        let fail = Closure::once_into_js(move || {
            let e = r
                .error()
                .ok()
                .flatten()
                .map_or(JsValue::from_str("request failed"), JsValue::from);
            let _ = reject.call1(&JsValue::NULL, &e);
        });
        req.set_onsuccess(Some(ok.unchecked_ref()));
        req.set_onerror(Some(fail.unchecked_ref()));
    });
    JsFuture::from(promise).await.map_err(|e| idb_err(&e))
}

/// Resolves when a transaction commits.
async fn transaction_done(tx: &IdbTransaction) -> Result<(), IdbError> {
    let promise = Promise::new(&mut |resolve, reject| {
        let ok = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let reject2 = reject.clone();
        let fail = Closure::once_into_js(move || {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("transaction failed"));
        });
        let abort = Closure::once_into_js(move || {
            let _ = reject2.call1(&JsValue::NULL, &JsValue::from_str("transaction aborted"));
        });
        tx.set_oncomplete(Some(ok.unchecked_ref()));
        tx.set_onerror(Some(fail.unchecked_ref()));
        tx.set_onabort(Some(abort.unchecked_ref()));
    });
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|e| idb_err(&e))
}

/// What [`open_storage`] reports while it opens the database, and later.
#[derive(Clone)]
pub struct OpenHooks {
    /// Called with `true` while the upgrade waits for other tabs to close their connections, and
    /// with `false` once it went ahead.
    pub blocked: Rc<dyn Fn(bool)>,
    /// Called when another tab opens a newer version: this tab should stop making changes (stop the
    /// sync engine). The connection closes once everything written is stored.
    pub version_change: Rc<dyn Fn()>,
    /// Called with `true` while the connection stays open for another tab's upgrade because writes
    /// are still failing (the other tab waits), and with `false` once they landed.
    pub holding: Rc<dyn Fn(bool)>,
    /// Called after the connection closed for another tab's upgrade (the app should give up the
    /// sender lock and reload, to run the newer build).
    pub closed: Rc<dyn Fn()>,
}

impl fmt::Debug for OpenHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenHooks").finish_non_exhaustive()
    }
}

impl Default for OpenHooks {
    fn default() -> Self {
        Self {
            blocked: Rc::new(|_| {}),
            version_change: Rc::new(|| {}),
            holding: Rc::new(|_| {}),
            closed: Rc::new(|| {}),
        }
    }
}

async fn open_db(name: &str, version: u32, hooks: &OpenHooks) -> Result<IdbDatabase, IdbError> {
    let factory = web_sys::window()
        .and_then(|w| w.indexed_db().ok().flatten())
        .ok_or_else(|| IdbError("IndexedDB is not available".into()))?;
    let open: IdbOpenDbRequest = factory
        .open_with_u32(name, version)
        .map_err(|e| idb_err(&e))?;
    let o = open.clone();
    let upgrade = Closure::once_into_js(move || {
        if let Ok(db) = o.result().and_then(|r| r.dyn_into::<IdbDatabase>())
            && let Some(tx) = o.transaction()
        {
            let migrate = || -> Result<(), JsValue> {
                let names = db.object_store_names();
                for store in STORES {
                    if !names.contains(store) {
                        db.create_object_store(store)?;
                    }
                }
                // v6 clients read full workflow events. Discard only the old projection, not the
                // outbox, receipts or log epoch. The upgrade transaction excludes older tabs.
                tx.object_store(KV)?
                    .delete(&JsValue::from_str(Key::Snapshot.name()))?;
                tx.object_store(EVENTS)?.clear()?;
                Ok(())
            };
            if migrate().is_err() {
                let _ = tx.abort();
            }
        }
    });
    open.set_onupgradeneeded(Some(upgrade.unchecked_ref()));
    let was_blocked = Rc::new(Cell::new(false));
    let on_blocked = {
        let hooks = hooks.clone();
        let was_blocked = was_blocked.clone();
        Closure::<dyn FnMut()>::new(move || {
            tracing::info!("the database upgrade waits for other tabs to close");
            was_blocked.set(true);
            (hooks.blocked)(true);
        })
    };
    open.set_onblocked(Some(on_blocked.as_ref().unchecked_ref()));
    let db = request_done(&open).await;
    open.set_onblocked(None);
    drop(on_blocked);
    if was_blocked.get() {
        (hooks.blocked)(false);
    }
    db?.dyn_into::<IdbDatabase>()
        .map_err(|_| IdbError("open did not return a database".into()))
}

fn store(tx: &IdbTransaction, name: &str) -> Result<IdbObjectStore, IdbError> {
    tx.object_store(name).map_err(|e| idb_err(&e))
}

/// Every `(key, value)` of a store, in key order.
async fn entries(tx: &IdbTransaction, name: &str) -> Result<Vec<(JsValue, JsValue)>, IdbError> {
    let s = store(tx, name)?;
    let keys = request_done(&s.get_all_keys().map_err(|e| idb_err(&e))?).await?;
    let values = request_done(&s.get_all().map_err(|e| idb_err(&e))?).await?;
    Ok(Array::from(&keys)
        .iter()
        .zip(Array::from(&values).iter())
        .collect())
}

/// A record's value as the string it was stored as (anything else, as JSON, so it can be set aside).
fn value_text(v: &JsValue) -> String {
    v.as_string().unwrap_or_else(|| {
        js_sys::JSON::stringify(v)
            .ok()
            .and_then(|s| s.as_string())
            .unwrap_or_default()
    })
}

async fn read_all(db: &IdbDatabase) -> Result<Persisted, IdbError> {
    let tx = db
        .transaction_with_str_sequence(&store_names())
        .map_err(|e| idb_err(&e))?;
    let mut p = Persisted::default();
    for (k, v) in entries(&tx, KV).await? {
        if let Some(k) = k.as_string().and_then(|k| Key::from_name(&k)) {
            p.kv.insert(k, value_text(&v));
        }
    }
    for (k, v) in entries(&tx, EVENTS).await? {
        if let Some(k) = k.as_f64() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            p.events.insert(k as u64, value_text(&v));
        }
    }
    for (name, into) in [
        (OUTBOX, &mut p.outbox),
        (FAILED, &mut p.failed),
        (DAMAGED, &mut p.damaged),
    ] {
        for (k, v) in entries(&tx, name).await? {
            // Keys are strings; anything else is kept under its text, and set aside by recovery.
            let key = k.as_string().unwrap_or_else(|| value_text(&k));
            into.insert(key, value_text(&v));
        }
    }
    Ok(p)
}

async fn write_batch(db: &IdbDatabase, batch: &Batch) -> Result<(), IdbError> {
    let tx = db
        .transaction_with_str_sequence_and_mode(&store_names(), IdbTransactionMode::Readwrite)
        .map_err(|e| idb_err(&e))?;
    let kv = store(&tx, KV)?;
    let events = store(&tx, EVENTS)?;
    let outbox = store(&tx, OUTBOX)?;
    let failed = store(&tx, FAILED)?;
    let damaged = store(&tx, DAMAGED)?;
    let area = |a: Area| match a {
        Area::Outbox => &outbox,
        Area::Failed => &failed,
    };
    for change in batch {
        let r = match change {
            Change::Put(k, v) => kv
                .put_with_key(&JsValue::from_str(v), &JsValue::from_str(k.name()))
                .map(drop),
            Change::Delete(k) => kv.delete(&JsValue::from_str(k.name())).map(drop),
            #[allow(clippy::cast_precision_loss)]
            Change::PutEvent(seq, v) => events
                .put_with_key(&JsValue::from_str(v), &JsValue::from_f64(*seq as f64))
                .map(drop),
            Change::ClearEvents => events.clear().map(drop),
            Change::PutEntry(a, k, v) => area(*a)
                .put_with_key(&JsValue::from_str(v), &JsValue::from_str(k))
                .map(drop),
            Change::DeleteEntry(a, k) => area(*a).delete(&JsValue::from_str(k)).map(drop),
            Change::PutDamaged(k, v) => damaged
                .put_with_key(&JsValue::from_str(v), &JsValue::from_str(k))
                .map(drop),
        };
        if let Err(e) = r {
            let _ = tx.abort();
            return Err(idb_err(&e));
        }
    }
    transaction_done(&tx).await
}

/// The Web Lock held by the tab that sends for all tabs.
pub const SENDER_LOCK: &str = "todo-sender";

/// The `BroadcastChannel` tabs announce new outbox records on.
pub const CHANNEL: &str = "todo-outbox";

/// How often a tab asked to close its connection checks whether its writes have landed.
const CLOSE_POLL: Duration = Duration::from_millis(250);

/// Persists a [`MirrorStorage`]'s journal to IndexedDB. Clones share the connection.
#[derive(Clone)]
pub struct IdbFlusher {
    db: IdbDatabase,
    storage: MirrorStorage,
    /// A flush is running (it drains the journal; another one would only write the same batches).
    busy: Rc<Cell<bool>>,
    /// The connection was closed for another tab's upgrade.
    closed: Rc<Cell<bool>>,
    channel: Rc<RefCell<Option<BroadcastChannel>>>,
}

impl fmt::Debug for IdbFlusher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdbFlusher")
            .field("storage", &self.storage)
            .finish_non_exhaustive()
    }
}

/// Clears a flag when dropped.
struct Busy(Rc<Cell<bool>>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn touches_entries(batch: &Batch) -> bool {
    batch
        .iter()
        .any(|c| matches!(c, Change::PutEntry(..) | Change::DeleteEntry(..)))
}

impl IdbFlusher {
    fn new(db: IdbDatabase, storage: MirrorStorage) -> Self {
        Self {
            db,
            storage,
            busy: Rc::new(Cell::new(false)),
            closed: Rc::new(Cell::new(false)),
            channel: Rc::new(RefCell::new(None)),
        }
    }

    /// Announces committed outbox and failed-list changes on `channel` (see the module docs).
    pub fn announce_on(&self, channel: BroadcastChannel) {
        *self.channel.borrow_mut() = Some(channel);
    }

    /// Writes everything waiting now, oldest first, each batch in its own transaction; a batch
    /// leaves the journal only once its transaction committed. On failure the batch stays queued and
    /// the error is recorded. If a flush is already running, returns at once (that one drains the
    /// journal).
    pub async fn flush(&self) -> Result<(), IdbError> {
        if self.busy.get() || self.closed.get() {
            return Ok(());
        }
        self.busy.set(true);
        let _busy = Busy(self.busy.clone());
        let mut announce = false;
        let result = loop {
            let Some((seq, batch)) = self.storage.front() else {
                break Ok(());
            };
            match write_batch(&self.db, &batch).await {
                Ok(()) => {
                    self.storage.commit_front(seq);
                    announce |= touches_entries(&batch);
                }
                Err(e) => {
                    self.storage.set_flush_error(e.0.clone());
                    break Err(e);
                }
            }
        };
        if announce && let Some(c) = &*self.channel.borrow() {
            let _ = c.post_message(&JsValue::from_str("outbox"));
        }
        result
    }

    /// Flushes after every write, until the connection is closed (spawn it with `spawn_local`).
    /// Failures are retried after a second.
    pub async fn run(self) {
        let written = self.storage.written();
        let mut seen = written.generation();
        loop {
            if self.closed.get() {
                return;
            }
            if self.storage.has_pending()
                && let Err(e) = self.flush().await
            {
                tracing::warn!(error = %e, "could not persist; retrying");
                sleep(Duration::from_secs(1)).await;
                continue;
            }
            seen = written.changed(seen).await;
        }
    }

    /// The outbox and failed-list records in the database now, every tab's (unreadable ones are
    /// skipped: the replica that reads them on start sets them aside). The outbox entries are
    /// recorded as durable in this tab's storage.
    pub async fn queued(&self) -> Result<(Vec<OutboxEntry>, Vec<FailedEntry>), IdbError> {
        let names: Array = [OUTBOX, FAILED]
            .iter()
            .map(|s| JsValue::from_str(s))
            .collect();
        let tx = self
            .db
            .transaction_with_str_sequence(&names)
            .map_err(|e| idb_err(&e))?;
        let outbox: Vec<OutboxEntry> = entries(&tx, OUTBOX)
            .await?
            .iter()
            .filter_map(|(_, v)| serde_json::from_str(&value_text(v)).ok())
            .collect();
        let failed: Vec<FailedEntry> = entries(&tx, FAILED)
            .await?
            .iter()
            .filter_map(|(_, v)| serde_json::from_str(&value_text(v)).ok())
            .collect();
        self.storage
            .mark_durable(outbox.iter().map(|e| e.command_id));
        Ok((outbox, failed))
    }

    /// Writes what is left, then closes the connection. Never closes while anything written is not
    /// stored: until then `holding(true)` is reported (and the flusher keeps retrying).
    async fn close_when_saved(&self, holding: &dyn Fn(bool)) {
        let mut held = false;
        loop {
            if !self.storage.has_pending() {
                break;
            }
            if !self.busy.get() {
                let _ = self.flush().await;
            }
            if self.storage.has_pending() {
                if !held && self.storage.flush_error().is_some() {
                    tracing::info!(
                        "unsaved changes keep the database open for another tab's upgrade"
                    );
                    held = true;
                    holding(true);
                }
                sleep(CLOSE_POLL).await;
            }
        }
        if held {
            holding(false);
        }
        self.closed.set(true);
        self.db.close();
    }
}

/// Opens the `BroadcastChannel` [`CHANNEL`] and calls `on_message` for every message from another
/// tab. `None` if the browser has none.
#[must_use]
pub fn open_channel(on_message: impl Fn() + 'static) -> Option<BroadcastChannel> {
    let channel = BroadcastChannel::new(CHANNEL).ok()?;
    let cb = Closure::<dyn Fn()>::new(on_message);
    channel.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
    Some(channel)
}

/// The sender lock, held or asked for (see [`elect_sender`]). Clones share it.
#[derive(Clone)]
pub struct SenderLock {
    abort: AbortController,
    release: Rc<RefCell<Option<js_sys::Function>>>,
}

impl fmt::Debug for SenderLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SenderLock").finish_non_exhaustive()
    }
}

impl SenderLock {
    /// Gives the lock up (or stops waiting for it), so the next tab can send.
    pub fn release(&self) {
        self.abort.abort();
        if let Some(resolve) = self.release.borrow_mut().take() {
            let _ = resolve.call0(&JsValue::NULL);
        }
    }
}

/// Asks for the Web Lock [`SENDER_LOCK`] and calls `on_elected` once this tab holds it; it holds it
/// until the page goes away or [`SenderLock::release`]. Returns `None` (and never calls `on_elected`)
/// if the browser has no Web Locks, in which case every tab sends for itself.
pub fn elect_sender(on_elected: impl FnOnce() + 'static) -> Option<SenderLock> {
    let locks = web_sys::window()
        .and_then(|w| Reflect::get(&w, &JsValue::from_str("navigator")).ok())
        .and_then(|n| Reflect::get(&n, &JsValue::from_str("locks")).ok())
        .filter(|l| !l.is_undefined() && !l.is_null())?;
    let request = Reflect::get(&locks, &JsValue::from_str("request"))
        .and_then(|f| f.dyn_into::<js_sys::Function>())
        .ok()?;
    let abort = AbortController::new().ok()?;
    let options = js_sys::Object::new();
    Reflect::set(&options, &JsValue::from_str("signal"), &abort.signal()).ok()?;
    let lock = SenderLock {
        abort,
        release: Rc::new(RefCell::new(None)),
    };
    let release = lock.release.clone();
    let mut on_elected = Some(on_elected);
    let granted = Closure::<dyn FnMut() -> Promise>::new(move || {
        // Held until this promise resolves: at `release`, or when the page goes away.
        let held = Promise::new(&mut |resolve, _| {
            *release.borrow_mut() = Some(resolve);
        });
        if let Some(f) = on_elected.take() {
            tracing::debug!("this tab sends for all tabs");
            f();
        }
        held
    });
    let asked = request.call3(
        &locks,
        &JsValue::from_str(SENDER_LOCK),
        &options,
        granted.as_ref(),
    );
    granted.forget();
    // Giving up while still waiting rejects the request: that is expected, not an error.
    if let Ok(p) = asked.and_then(JsCast::dyn_into::<Promise>) {
        let ignore = Closure::<dyn FnMut(JsValue)>::new(|_| {});
        let _ = p.catch(&ignore);
        ignore.forget();
        Some(lock)
    } else {
        None
    }
}

/// Why the browser replica could not start.
#[derive(Debug, thiserror::Error)]
pub enum OpenStorageError {
    #[error(transparent)]
    Storage(#[from] IdbError),
    #[error("another tab requested a database upgrade during startup")]
    Upgrading,
}

/// Opens (or creates) the IndexedDB database `name` and loads it into a [`MirrorStorage`], with what
/// had to be repaired ([`MirrorStorage::recover`]): an unreadable confirmed state is fetched from the
/// server again, and unreadable outbox entries are set aside in the `damaged` store. Unsent work is
/// never deleted. The repairs are written first, as soon as the flusher runs.
///
/// `hooks` hear about an upgrade blocked by other tabs, and about this connection being closed for
/// another tab's upgrade (see the module docs).
///
/// Stops startup if another tab requests an upgrade during the replica read.
pub async fn open_storage(
    name: &str,
    hooks: OpenHooks,
) -> Result<(MirrorStorage, IdbFlusher, Recovery), OpenStorageError> {
    let db = open_db(name, DB_VERSION, &hooks).await?;
    // An upgrade can request this connection to close as soon as it opens. Do not miss that
    // event while reading the replica. No edits or repairs need flushing until recovery starts.
    let upgrading = Rc::new(Cell::new(false));
    let during_read = {
        let db = db.clone();
        let upgrading = upgrading.clone();
        let notify = hooks.version_change.clone();
        Closure::<dyn Fn()>::new(move || {
            if !upgrading.replace(true) {
                notify();
                db.close();
            }
        })
    };
    db.set_onversionchange(Some(during_read.as_ref().unchecked_ref()));
    let persisted = read_all(&db).await;
    db.set_onversionchange(None);
    if upgrading.get() {
        (hooks.closed)();
        return Err(OpenStorageError::Upgrading);
    }
    let persisted = persisted.inspect_err(|_| db.close())?;
    let (storage, recovery) = MirrorStorage::recover(persisted);
    if recovery != Recovery::default() {
        tracing::warn!(
            dropped_confirmed = recovery.dropped_confirmed,
            dropped_events = recovery.dropped_events,
            damaged = recovery.damaged.len(),
            "repaired the stored replica"
        );
    }
    let flusher = IdbFlusher::new(db.clone(), storage.clone());
    let closing = Rc::new(Cell::new(false));
    let on_version_change = {
        let flusher = flusher.clone();
        Closure::<dyn Fn()>::new(move || {
            if closing.replace(true) {
                return;
            }
            tracing::info!("another tab upgrades the database; closing once everything is stored");
            (hooks.version_change)();
            let flusher = flusher.clone();
            let hooks = hooks.clone();
            wasm_bindgen_futures::spawn_local(async move {
                flusher.close_when_saved(&*hooks.holding).await;
                (hooks.closed)();
            });
        })
    };
    db.set_onversionchange(Some(on_version_change.as_ref().unchecked_ref()));
    on_version_change.forget();
    Ok((storage, flusher, recovery))
}

/// Deletes the database, unsent changes included: only for the user's explicit "reset local data".
pub async fn delete_storage(name: &str) -> Result<(), IdbError> {
    let factory = web_sys::window()
        .and_then(|w| w.indexed_db().ok().flatten())
        .ok_or_else(|| IdbError("IndexedDB is not available".into()))?;
    let req = factory.delete_database(name).map_err(|e| idb_err(&e))?;
    request_done(&req).await.map(drop)
}
