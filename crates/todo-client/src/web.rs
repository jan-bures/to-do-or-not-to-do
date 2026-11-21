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
//! ([`CSRF_PATH`]) and, if it is a new one, sends the same request again (same body, so the same
//! command ID: the server applies it once). If the token is unchanged, the refusal was about
//! something else (the `Origin`), and the `403` stands. If the refresh itself fails, the request
//! counts as a network failure and is retried later; a signed-out session is a `401`.
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
//! The database has one object store per part of [`Persisted`]: `kv` (snapshot and epoch), `events`,
//! `outbox` and `failed` (one record per command, so tabs never overwrite each other's), and
//! `damaged` (records that could not be read, set aside). Nothing here ever deletes the database on
//! its own: [`delete_storage`] is for an explicit "reset local data" by the user.

use crate::mirror::{Area, Batch, Change, Key, MirrorStorage, Persisted, Recovery};
use crate::platform::{
    Clock, Entropy, EventStream, Method, Request, Response, SseFrame, StreamOpen, Transport,
    TransportError,
};
use crate::sse::{DEFAULT_MAX_FRAME_BYTES, SseParser};
use js_sys::{Array, Promise, Reflect, Uint8Array};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use todo_core::time::Timestamp;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, Headers, IdbDatabase, IdbObjectStore, IdbOpenDbRequest, IdbRequest,
    IdbTransaction, IdbTransactionMode, ReadableStreamDefaultReader, RequestCredentials,
    RequestInit,
};

/// The header carrying the session's CSRF token.
pub const CSRF_HEADER: &str = "X-CSRF-Token";

/// Where the session's current CSRF token is read (`GET`, the cookie only: `{"csrf": "..."}`).
pub const CSRF_PATH: &str = "/auth/session";

/// The session's CSRF token, shared by the transport and the page (which needs it for its own
/// requests, such as the sign-out form). Clones share the value.
#[derive(Clone, Default)]
pub struct CsrfToken(Rc<RefCell<String>>);

impl fmt::Debug for CsrfToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the token.
        f.debug_struct("CsrfToken").finish_non_exhaustive()
    }
}

impl CsrfToken {
    /// A token with this value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Rc::new(RefCell::new(value.into())))
    }

    /// The current value.
    #[must_use]
    pub fn get(&self) -> String {
        self.0.borrow().clone()
    }

    fn set(&self, value: String) {
        *self.0.borrow_mut() = value;
    }
}

/// What asking for the session's current token found.
enum Refresh {
    /// A new token, now in use: send the request again.
    New,
    /// The same token: the refusal was not about it.
    Same,
    /// The session is gone (this `401` is the answer).
    SignedOut(Response),
}

/// True if a response is the server's `403 csrf`.
fn is_csrf_refusal(res: &Response) -> bool {
    res.status == 403
        && serde_json::from_slice::<serde_json::Value>(&res.body)
            .ok()
            .and_then(|v| v.get("error")?.get("code")?.as_str().map(str::to_owned))
            .is_some_and(|c| c == "csrf")
}

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
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }

    /// Asks the server for the session's current token (see the module docs).
    async fn refresh(&self) -> Result<Refresh, TransportError> {
        let abort = new_abort()?;
        let req = Request {
            method: Method::Get,
            path: CSRF_PATH.to_owned(),
            body: None,
        };
        let res = read(&self.fetch(&req, "application/json", &abort.0).await?).await?;
        match res.status {
            200 => {
                let token = serde_json::from_slice::<serde_json::Value>(&res.body)
                    .ok()
                    .and_then(|v| v.get("csrf")?.as_str().map(str::to_owned))
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| TransportError::Protocol("unreadable session token".into()))?;
                if token == self.csrf.get() {
                    Ok(Refresh::Same)
                } else {
                    tracing::debug!("the session's CSRF token changed; retrying with the new one");
                    self.csrf.set(token);
                    Ok(Refresh::New)
                }
            }
            401 => Ok(Refresh::SignedOut(res)),
            s => Err(TransportError::Network(format!(
                "could not refresh the session token ({s})"
            ))),
        }
    }

    async fn fetch(
        &self,
        request: &Request,
        accept: &str,
        abort: &AbortController,
    ) -> Result<web_sys::Response, TransportError> {
        let headers = Headers::new().map_err(|e| TransportError::Network(js_err(&e)))?;
        let set = |k: &str, v: &str| {
            headers
                .set(k, v)
                .map_err(|e| TransportError::Network(js_err(&e)))
        };
        set(CSRF_HEADER, &self.csrf.get())?;
        set("Accept", accept)?;
        let init = RequestInit::new();
        init.set_credentials(RequestCredentials::SameOrigin);
        init.set_signal(Some(&abort.signal()));
        match request.method {
            Method::Get => init.set_method("GET"),
            Method::Post => init.set_method("POST"),
        }
        if let Some(body) = &request.body {
            set("Content-Type", "application/json")?;
            let bytes = Uint8Array::from(body.as_slice());
            init.set_body(&bytes);
        }
        init.set_headers(&headers);
        let req = web_sys::Request::new_with_str_and_init(&request.path, &init)
            .map_err(|e| TransportError::Network(js_err(&e)))?;
        let res = JsFuture::from(window()?.fetch_with_request(&req))
            .await
            .map_err(|e| TransportError::Network(js_err(&e)))?;
        res.dyn_into::<web_sys::Response>()
            .map_err(|_| TransportError::Protocol("fetch did not return a Response".into()))
    }
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
    let buf = JsFuture::from(
        res.array_buffer()
            .map_err(|e| TransportError::Network(js_err(&e)))?,
    )
    .await
    .map_err(|e| TransportError::Network(js_err(&e)))?;
    Ok(Response {
        status: res.status(),
        retry_after,
        body: Uint8Array::new(&buf).to_vec(),
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
        let abort = new_abort()?;
        let res = read(&self.fetch(&request, "application/json", &abort.0).await?).await?;
        if !is_csrf_refusal(&res) {
            return Ok(res);
        }
        match self.refresh().await? {
            Refresh::Same => Ok(res),
            Refresh::SignedOut(r) => Ok(r),
            Refresh::New => {
                let abort = new_abort()?;
                read(&self.fetch(&request, "application/json", &abort.0).await?).await
            }
        }
    }

    async fn open_stream(
        &self,
        request: Request,
    ) -> Result<StreamOpen<FetchEventStream>, TransportError> {
        let mut abort = new_abort()?;
        let mut res = self.fetch(&request, "text/event-stream", &abort.0).await?;
        if res.status() == 403 {
            let refused = read(&res).await?;
            if !is_csrf_refusal(&refused) {
                return Ok(StreamOpen::Refused(refused));
            }
            match self.refresh().await? {
                Refresh::Same => return Ok(StreamOpen::Refused(refused)),
                Refresh::SignedOut(r) => return Ok(StreamOpen::Refused(r)),
                Refresh::New => {
                    abort = new_abort()?;
                    res = self.fetch(&request, "text/event-stream", &abort.0).await?;
                }
            }
        }
        if res.status() != 200 {
            return Ok(StreamOpen::Refused(read(&res).await?));
        }
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

/// The IndexedDB database's version. 2: the outbox and failed list one record per command, and the
/// damaged area (version 1 kept each list as one array in `kv`; it is split on the first start).
const DB_VERSION: u32 = 2;
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

async fn open_db(name: &str) -> Result<IdbDatabase, IdbError> {
    let factory = web_sys::window()
        .and_then(|w| w.indexed_db().ok().flatten())
        .ok_or_else(|| IdbError("IndexedDB is not available".into()))?;
    let open: IdbOpenDbRequest = factory
        .open_with_u32(name, DB_VERSION)
        .map_err(|e| idb_err(&e))?;
    let o = open.clone();
    let upgrade = Closure::once_into_js(move || {
        if let Ok(db) = o.result().and_then(|r| r.dyn_into::<IdbDatabase>()) {
            let names = db.object_store_names();
            for store in STORES {
                if !names.contains(store) {
                    let _ = db.create_object_store(store);
                }
            }
        }
    });
    open.set_onupgradeneeded(Some(upgrade.unchecked_ref()));
    let db = request_done(&open).await?;
    db.dyn_into::<IdbDatabase>()
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

/// Persists a [`MirrorStorage`]'s journal to IndexedDB.
pub struct IdbFlusher {
    db: IdbDatabase,
    storage: MirrorStorage,
}

impl fmt::Debug for IdbFlusher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdbFlusher")
            .field("storage", &self.storage)
            .finish_non_exhaustive()
    }
}

impl IdbFlusher {
    /// Writes everything waiting now. On failure the batches stay queued.
    pub async fn flush(&self) -> Result<(), IdbError> {
        let mut batches: VecDeque<Batch> = self.storage.take_batches().into();
        while let Some(b) = batches.pop_front() {
            if let Err(e) = write_batch(&self.db, &b).await {
                batches.push_front(b);
                self.storage.requeue(batches.into());
                return Err(e);
            }
        }
        Ok(())
    }

    /// Flushes after every write, forever (spawn it with `spawn_local`). Failures are retried after
    /// a second.
    pub async fn run(self) {
        let written = self.storage.written();
        let mut seen = written.generation();
        loop {
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
}

/// Opens (or creates) the IndexedDB database `name` and loads it into a [`MirrorStorage`], with what
/// had to be repaired ([`MirrorStorage::recover`]): an unreadable confirmed state is fetched from the
/// server again, and unreadable outbox entries are set aside in the `damaged` store. Unsent work is
/// never deleted. The repairs are written first, as soon as the flusher runs.
///
/// Fails only if IndexedDB itself fails (unavailable, blocked, or out of space).
pub async fn open_storage(name: &str) -> Result<(MirrorStorage, IdbFlusher, Recovery), IdbError> {
    let db = open_db(name).await?;
    let persisted = read_all(&db).await?;
    let (storage, recovery) = MirrorStorage::recover(persisted);
    if recovery != Recovery::default() {
        tracing::warn!(
            dropped_confirmed = recovery.dropped_confirmed,
            dropped_events = recovery.dropped_events,
            damaged = recovery.damaged.len(),
            "repaired the stored replica"
        );
    }
    Ok((storage.clone(), IdbFlusher { db, storage }, recovery))
}

/// Deletes the database, unsent changes included: only for the user's explicit "reset local data".
pub async fn delete_storage(name: &str) -> Result<(), IdbError> {
    let factory = web_sys::window()
        .and_then(|w| w.indexed_db().ok().flatten())
        .ok_or_else(|| IdbError("IndexedDB is not available".into()))?;
    let req = factory.delete_database(name).map_err(|e| idb_err(&e))?;
    request_done(&req).await.map(drop)
}
