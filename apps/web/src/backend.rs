//! Where the state comes from and where commands go.
//!
//! - **Live** (the real app): a `todo_client::SyncClient` over `fetch`, with the replica mirrored to
//!   IndexedDB so pending edits survive a reload or a closed tab, and the event stream read with a
//!   streaming `fetch` (see `docs/web.md`). Requests carry the session cookie and the CSRF token from the
//!   page (refreshed by the transport if the session changes); there is no token in the browser.
//!   Local data is never deleted on its own: what can't be read is repaired or set aside
//!   (`todo_client::mirror`), and a storage failure shows a "Reset local data" action instead.
//!   A command is sent only once IndexedDB has it; a failed write shows until one succeeds, and the
//!   page asks before closing while anything is unsaved. One tab sends for all of them (a Web Lock);
//!   the others tell it about their commands over a `BroadcastChannel` (`todo_client::web`).
//! - **Fixtures** (the screenshot gallery, `--features fixtures`): invented data with a fixed clock,
//!   changed through `decide` and `apply` like the server would, and never sent anywhere.
//!
//! The backend lives in a thread-local, so the UI's reactive closures (which must be `Send`) only hold
//! signals and call the free functions here.
// The fixtures build never opens the live backend.
#![cfg_attr(feature = "fixtures", allow(dead_code))]

use crate::changes::{self, Failure};
#[cfg(feature = "fixtures")]
use crate::model::Now;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use todo_client::activity::{ActivityError, ActivityPage, ActivityQuery};
use todo_client::mirror::{MirrorStorage, Recovery};
use todo_client::notify::Notify;
use todo_client::web::{
    BrowserClock, BrowserEntropy, CsrfToken, FetchTransport, IdbFlusher, OpenHooks,
    OpenStorageError, SenderLock, elect_sender, open_channel, open_storage, sleep,
};
use todo_client::{
    ClientConfig, CommandId, Connection, SubmitError, SyncClient, TrackedCommand, TrackedUndo,
    failure, protocol,
};
use todo_core::ids::{AttachmentId, EventId, TaskId};
use todo_core::{Actor, Command, State};
use uuid::Uuid;

/// The IndexedDB database of the replica.
const DB_NAME: &str = "todo-replica";

/// The owner's device as the server names a web session (`todo_server::auth::WEB_DEVICE`).
pub const DEVICE: &str = "web";

type Client = SyncClient<FetchTransport, BrowserClock>;

enum Backend {
    Live(Client),
    #[cfg(feature = "fixtures")]
    Local(Local),
}

#[cfg(feature = "fixtures")]
struct Local {
    state: Arc<State>,
    now: Now,
    /// Pretend storing fails (`?fixture=storage-error`, for the gallery).
    storage_error: bool,
    /// Pretend a newer version took the database over (`?fixture=read-only`, for the gallery).
    read_only: bool,
    started_ms: f64,
    counter: u64,
    failures: Vec<Failure>,
    activity: Vec<todo_client::activity::ActivityItem>,
}

thread_local! {
    static BACKEND: RefCell<Option<Backend>> = const { RefCell::new(None) };
    static CSRF: RefCell<Option<CsrfToken>> = const { RefCell::new(None) };
    static STORE: RefCell<Option<(MirrorStorage, IdbFlusher)>> = const { RefCell::new(None) };
    /// The sender lock, while this tab holds or waits for it.
    static SENDER: RefCell<Option<SenderLock>> = const { RefCell::new(None) };
    /// Undo targets in the shared outbox, for tabs that aren't adopting its commands.
    static SHARED_UNDOS: RefCell<BTreeSet<EventId>> = const { RefCell::new(BTreeSet::new()) };
    /// Bumped when something only this module knows changed (an outbox scan, an upgrade waiting).
    static LOCAL: Notify = Notify::default();
    /// Bumped to rescan the outbox store now (the tab became visible, or the network came back).
    static RESCAN: Notify = Notify::default();
    /// Why reading the other tabs' changes keeps failing.
    static SCAN_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    static SCAN_FAILURES: Cell<u32> = const { Cell::new(0) };
    /// Another tab upgrades the database: this one stopped and closes once its writes landed.
    static UPGRADING: Cell<bool> = const { Cell::new(false) };
    /// ... and its writes keep failing, so the other tab waits.
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

/// How often the sending tab reads the outbox store, in case a tab's announcement was missed.
const RESCAN_EVERY: Duration = Duration::from_secs(5);
/// How soon a failed read is tried again.
const SCAN_RETRY: Duration = Duration::from_secs(1);
/// Consecutive failed reads before the alert shows.
const SCAN_FAILURES_SHOWN: u32 = 2;

fn store() -> Option<(MirrorStorage, IdbFlusher)> {
    STORE.with(|s| s.borrow().clone())
}

/// The session's CSRF token: the transport's (kept current when the session changes), or the page's
/// before the backend opens.
#[must_use]
pub fn csrf() -> Option<String> {
    CSRF.with(|c| c.borrow().as_ref().map(CsrfToken::get))
        .filter(|t| !t.is_empty())
        .or_else(crate::platform::csrf)
}

/// Sync status as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sync {
    /// Online, syncing or offline.
    pub connection: Connection,
    /// Changes not yet confirmed by the server.
    pub pending: usize,
    /// Changes the server refused.
    pub failed: usize,
    /// The session is gone: sign in again.
    pub signed_out: bool,
    /// The fixtures backend (nothing is sent anywhere).
    pub offline_demo: bool,
    /// Why this browser couldn't store the last change (retried until it works).
    pub local_error: Option<String>,
    /// Something is not in this browser's storage yet: closing the page now could lose it.
    pub unsaved: bool,
    /// Another tab waits to upgrade the database until this one's unsaved changes are stored.
    pub holding_upgrade: bool,
    /// A newer version took the database over: this tab takes no more edits (reload to keep editing).
    pub read_only: bool,
}

impl Default for Sync {
    fn default() -> Self {
        Self {
            connection: Connection::Syncing,
            pending: 0,
            failed: 0,
            signed_out: false,
            offline_demo: false,
            local_error: None,
            unsaved: false,
            holding_upgrade: false,
            read_only: false,
        }
    }
}

/// Why the app could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The page has no CSRF token: it was not served by `todo-server`.
    #[error("this page was not served by the to-do server")]
    NoCsrf,
    /// IndexedDB is not available (private mode in some browsers) or failed, or the stored replica
    /// could not be opened. Nothing is deleted: the page offers "Reset local data".
    #[error("local storage is unavailable: {0}")]
    Storage(String),
    /// Another tab asked this one to retire before it finished opening.
    #[error("another tab is updating the local database; reload to keep using To-do")]
    Updating,
}

/// The live backend, opened.
pub struct Opened {
    /// Persists the replica; run it alongside [`run`].
    pub flusher: IdbFlusher,
    /// What had to be repaired in the stored replica.
    pub recovery: Recovery,
}

fn owner() -> Actor {
    Actor::owner(DEVICE).unwrap_or(Actor::System)
}

/// Opens the live backend: the replica from IndexedDB (or empty) and the sync engine.
///
/// The stored replica is never deleted here, even if it fails to open: it may hold changes not yet
/// sent. Unreadable parts are repaired on the way in (see [`Recovery`]); anything worse is a
/// [`StartError::Storage`], and the page offers [`reset_local_data`].
///
/// `blocked` hears about a database upgrade that waits for other tabs. When another tab upgrades,
/// this one stops syncing and takes no more edits ([`submit`] refuses them, so nothing is shown as
/// saved that no database will hold), closes the database once everything it wrote is stored (saying
/// so while writes keep failing), gives up the sender lock, and reloads. If the reload doesn't happen,
/// the tab stays read-only and says so.
pub async fn open_live(csrf: String, blocked: Rc<dyn Fn(bool)>) -> Result<Opened, StartError> {
    let hooks = OpenHooks {
        blocked,
        version_change: Rc::new(|| {
            UPGRADING.with(|u| u.set(true));
            if let Some(c) = live() {
                c.stop();
            }
            LOCAL.with(Notify::notify);
        }),
        holding: Rc::new(|holding| {
            HOLDING.with(|h| h.set(holding));
            LOCAL.with(Notify::notify);
        }),
        closed: Rc::new(|| {
            if let Some(lock) = SENDER.with(|s| s.borrow_mut().take()) {
                lock.release();
            }
            crate::platform::reload();
        }),
    };
    let (storage, flusher, recovery) = open_storage(DB_NAME, hooks).await.map_err(|e| match e {
        OpenStorageError::Storage(e) => StartError::Storage(e.0),
        OpenStorageError::Upgrading => StartError::Updating,
    })?;
    let token = CsrfToken::new(csrf);
    let client = SyncClient::new(
        FetchTransport::new(token.clone()),
        BrowserClock,
        BrowserEntropy,
        storage.clone(),
        ClientConfig::new(owner()),
    )
    .map_err(|e| StartError::Storage(e.to_string()))?;
    CSRF.with(|c| *c.borrow_mut() = Some(token));
    STORE.with(|s| *s.borrow_mut() = Some((storage, flusher.clone())));
    // One tab sends for all: this one waits for the lock (if the browser has Web Locks), and sends
    // nothing, its own commands included, until it has read what the other tabs queued.
    client.set_sending(false);
    match elect_sender(|| wasm_bindgen_futures::spawn_local(become_sender())) {
        Some(lock) => SENDER.with(|s| *s.borrow_mut() = Some(lock)),
        None => {
            tracing::info!("no Web Locks: this tab sends its own commands");
            client.set_sending(true);
        }
    }
    if let Some(channel) = open_channel(|| {
        wasm_bindgen_futures::spawn_local(async {
            let sending = live().is_some_and(|c| c.is_sending());
            let _ = scan(sending).await;
        });
    }) {
        flusher.announce_on(channel);
    }
    BACKEND.with(|b| *b.borrow_mut() = Some(Backend::Live(client)));
    Ok(Opened { flusher, recovery })
}

/// This tab holds the sender lock: take in every tab's queued commands, and only then send (so they
/// go out in submission order, whoever queued them). Then keep reading the outbox store every few
/// seconds and on [`nudge`], in case an announcement was missed.
async fn become_sender() {
    while scan(true).await.is_err() {
        if UPGRADING.with(Cell::get) {
            return;
        }
        sleep(SCAN_RETRY).await;
    }
    if let Some(c) = live() {
        c.set_sending(true);
    }
    let mut delay = RESCAN_EVERY;
    loop {
        let rescan = RESCAN.with(Notify::clone);
        let seen = rescan.generation();
        futures_util::future::select(
            std::pin::pin!(sleep(delay)),
            std::pin::pin!(rescan.changed(seen)),
        )
        .await;
        if UPGRADING.with(Cell::get) {
            return;
        }
        delay = if scan(true).await.is_ok() {
            RESCAN_EVERY
        } else {
            SCAN_RETRY
        };
    }
}

/// Reads every tab's queued commands from IndexedDB: the sending tab (`adopt`) takes them in, while
/// the others only note pending undo targets. Every tab takes in what another refused.
async fn scan(adopt: bool) -> Result<(), ()> {
    let (Some(c), Some((_, flusher))) = (live(), store()) else {
        return Err(());
    };
    let (outbox, failed) = match flusher.queued().await {
        Ok(q) => {
            SCAN_FAILURES.with(|f| f.set(0));
            if SCAN_ERROR.with(|e| e.borrow_mut().take()).is_some() {
                LOCAL.with(Notify::notify);
            }
            q
        }
        Err(e) => {
            tracing::warn!("could not read other tabs' changes: {}", e.0);
            let n = SCAN_FAILURES.with(|f| {
                f.set(f.get() + 1);
                f.get()
            });
            if n >= SCAN_FAILURES_SHOWN {
                SCAN_ERROR.with(|s| *s.borrow_mut() = Some(e.0));
                LOCAL.with(Notify::notify);
            }
            return Err(());
        }
    };
    if adopt {
        if let Err(e) = c.adopt(outbox) {
            tracing::warn!("could not take in other tabs' changes: {e}");
        }
    } else {
        let targets = outbox
            .iter()
            .filter_map(|entry| entry.undo_target())
            .collect();
        SHARED_UNDOS.with(|shared| {
            let mut shared = shared.borrow_mut();
            if *shared != targets {
                *shared = targets;
                LOCAL.with(Notify::notify);
            }
        });
    }
    if let Err(e) = c.adopt_failed(failed) {
        tracing::warn!("could not take in other tabs' refusals: {e}");
    }
    Ok(())
}

/// Writes what is not stored yet (the page is being hidden).
pub fn flush_now() {
    if let Some((_, flusher)) = store() {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = flusher.flush().await;
        });
    }
}

/// True if closing the page now could lose a change (not stored yet, or storing failed).
#[must_use]
pub fn unsaved() -> bool {
    store().is_some_and(|(s, _)| s.unsaved() || s.flush_error().is_some())
}

/// Deletes this browser's copy, unsent changes included. Only on the user's confirmed request.
pub async fn reset_local_data() -> Result<(), String> {
    todo_client::web::delete_storage(DB_NAME)
        .await
        .map_err(|e| e.0)
}

/// The fixtures backend with this state.
#[cfg(feature = "fixtures")]
pub fn open_fixtures(state: State, now: Now) {
    let query = crate::platform::query();
    let failures = if query.contains("fixture=not-synced") {
        vec![
            Failure {
                id: Uuid::from_u128(1),
                what: "Rename list to “Museum visits”".into(),
                why: "The item or where it belongs no longer exists.",
            },
            Failure {
                id: Uuid::from_u128(2),
                what: "Undo a change".into(),
                why: changes::undo_failure("undo_conflict"),
            },
        ]
    } else {
        Vec::new()
    };
    BACKEND.with(|b| {
        *b.borrow_mut() = Some(Backend::Local(Local {
            state: Arc::new(state),
            now,
            storage_error: query.contains("storage-error"),
            read_only: query.contains("read-only"),
            started_ms: js_sys::Date::now(),
            counter: 0,
            failures,
            activity: if query.contains("fixture=blank") {
                Vec::new()
            } else {
                crate::fixtures::activity()
            },
        }));
    });
}

fn live() -> Option<Client> {
    BACKEND.with(|b| match &*b.borrow() {
        Some(Backend::Live(c)) => Some(c.clone()),
        #[cfg(feature = "fixtures")]
        _ => None,
        #[cfg(not(feature = "fixtures"))]
        None => None,
    })
}

/// Runs the sync engine until it stops (live backend only).
pub async fn run() {
    if let Some(c) = live() {
        c.run().await;
    }
}

/// Waits for the next change of the view or the status (live backend only; the fixtures backend
/// never changes on its own).
pub async fn next_change() {
    use futures_util::future::select;
    use std::pin::pin;
    let Some(c) = live() else {
        return std::future::pending::<()>().await;
    };
    let mut changes = c.changes();
    let local = LOCAL.with(Notify::clone);
    let local_seen = local.generation();
    let saved = store().and_then(|(s, _)| todo_client::Storage::saved(&s));
    let saved_seen = saved.as_ref().map(Notify::generation).unwrap_or_default();
    let saved_changed = async {
        match &saved {
            Some(n) => {
                n.changed(saved_seen).await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    select(
        pin!(changes.next()),
        pin!(select(pin!(local.changed(local_seen)), pin!(saved_changed))),
    )
    .await;
}

/// Asks the engine to retry now, and reads the outbox store (the tab became visible, or the network
/// came back).
pub fn nudge() {
    if let Some(c) = live() {
        c.nudge();
        if !c.is_sending() && !UPGRADING.with(Cell::get) {
            wasm_bindgen_futures::spawn_local(async {
                let _ = scan(false).await;
            });
        }
    }
    RESCAN.with(Notify::notify);
}

/// The view: the confirmed state with pending changes applied.
#[must_use]
pub fn view() -> Arc<State> {
    BACKEND.with(|b| match &*b.borrow() {
        Some(Backend::Live(c)) => c.view(),
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(l)) => l.state.clone(),
        None => Arc::new(State::new()),
    })
}

/// The sync status.
#[must_use]
pub fn status() -> Sync {
    BACKEND.with(|b| match &*b.borrow() {
        Some(Backend::Live(c)) => {
            let s = c.status();
            Sync {
                connection: s.connection,
                pending: s.pending,
                failed: s.failed,
                signed_out: s.revoked,
                offline_demo: false,
                local_error: store()
                    .and_then(|(s, _)| s.flush_error())
                    .or_else(|| SCAN_ERROR.with(|e| e.borrow().clone())),
                unsaved: store().is_some_and(|(s, _)| s.unsaved()),
                holding_upgrade: HOLDING.with(Cell::get),
                read_only: UPGRADING.with(Cell::get),
            }
        }
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(l)) => Sync {
            connection: Connection::Online,
            offline_demo: true,
            failed: l.failures.len(),
            local_error: l
                .storage_error
                .then(|| "QuotaExceededError: the quota has been exceeded".to_owned()),
            unsaved: l.storage_error,
            read_only: l.read_only,
            ..Sync::default()
        },
        None => Sync::default(),
    })
}

/// A fresh UUIDv7 for an entity a command creates.
#[must_use]
pub fn new_uuid() -> Uuid {
    BACKEND.with(|b| match &mut *b.borrow_mut() {
        Some(Backend::Live(c)) => c.new_uuid(),
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(l)) => {
            l.counter += 1;
            let ms = u64::try_from(l.now.at.as_millisecond()).unwrap_or(0);
            let mut rand = [0u8; 10];
            rand[..8].copy_from_slice(&l.counter.to_le_bytes());
            todo_core::ids::TaskId::new_v7(ms, rand).as_uuid()
        }
        None => Uuid::nil(),
    })
}

/// Why [`submit`] refuses every command once a newer version took the database over.
pub const READ_ONLY: &str = "a newer version is open in another tab";

#[derive(Debug)]
pub enum UploadError {
    Failed(Cow<'static, str>),
    SignedOut,
}

impl UploadError {
    fn failed() -> Self {
        Self::Failed(failure::attachment_upload(None, None))
    }
}

pub fn task_confirmed(task: TaskId) -> bool {
    live().map_or_else(
        || view().is_live(task.into()),
        |c| c.with_confirmed(|s| s.is_live(task.into())),
    )
}

/// The file cannot go ahead of the task's own create command. The optimistic task stays visible
/// (even offline) while queued, but disappears if its create is refused or the task is deleted.
pub async fn wait_for_task(task: TaskId) -> Result<(), UploadError> {
    loop {
        let status = status();
        if status.signed_out {
            return Err(UploadError::SignedOut);
        }
        if status.read_only || !view().is_live(task.into()) {
            return Err(UploadError::failed());
        }
        if task_confirmed(task) {
            return Ok(());
        }
        next_change().await;
    }
}

/// Wait for the files the server already accepted to appear in the confirmed replica. A tombstone
/// counts as observed: another tab can remove an attachment while this tab is catching up.
pub async fn wait_for_attachments(ids: &[AttachmentId]) -> bool {
    use futures_util::future::{Either, select};
    use std::pin::pin;
    let Some(c) = live() else { return true };
    let observed = async {
        while !c.with_confirmed(|s| ids.iter().all(|id| s.attachments.contains_key(id))) {
            next_change().await;
        }
    };
    matches!(
        select(pin!(observed), pin!(sleep(Duration::from_secs(30)))).await,
        Either::Left(_)
    )
}

/// Uploads directly; the file is not an offline edit. The live replica sees the result through SSE.
pub async fn upload_attachment(
    task: TaskId,
    file: web_sys::File,
) -> Result<AttachmentId, UploadError> {
    if status().read_only {
        return Err(UploadError::failed());
    }
    let id = AttachmentId::from_uuid(new_uuid());
    let csrf = CSRF.with(|c| c.borrow().clone());
    if let Some(csrf) = csrf {
        let result = FetchTransport::new(csrf)
            .upload_attachment(task, id, &file)
            .await
            .map_err(|_| UploadError::failed())?;
        return match result.status {
            200 | 201 => {
                #[derive(serde::Deserialize)]
                struct Receipt {
                    attachment: todo_core::model::AttachmentRef,
                }
                // This endpoint uses the client-supplied ID, even for a replay. If the receipt
                // isn't readable, use that ID rather than suggesting the upload failed.
                Ok(serde_json::from_slice::<Receipt>(&result.body).map_or(id, |r| r.attachment.id))
            }
            401 => Err(UploadError::SignedOut),
            status => {
                let max = (status == 413)
                    .then(|| protocol::error_detail(&result))
                    .flatten()
                    .and_then(|error| error.details)
                    .and_then(|details| details.get("max_bytes")?.as_u64());
                Err(UploadError::Failed(failure::attachment_upload(
                    Some(status),
                    max,
                )))
            }
        };
    }

    #[cfg(feature = "fixtures")]
    {
        // Simulate network latency for the gallery; this is not a UI animation or timer.
        sleep(Duration::from_secs(2)).await;
        if crate::platform::query().contains("fixture=upload-fails") {
            return Err(UploadError::failed());
        }
        let media_type = file.type_();
        // File.size is a non-negative byte count (limited to JS precision).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let size_bytes = file.size() as u64;
        submit(Command::AddAttachment {
            id,
            task,
            file_name: file.name(),
            media_type: if media_type.is_empty() {
                "application/octet-stream".into()
            } else {
                media_type
            },
            size_bytes,
            storage_key: format!("fixture-upload/{id}"),
        })
        .map(|()| id)
        .map_err(|_| UploadError::failed())
    }
    #[cfg(not(feature = "fixtures"))]
    Err(UploadError::failed())
}

/// Queues a command; the view reflects it at once. Fails if the command is invalid against the view,
/// and refuses every command (with [`READ_ONLY`], applying nothing) once another tab upgrades the
/// database: this tab's connection is closing, so an edit made now would be shown but never stored.
pub fn submit(command: Command) -> Result<(), SubmitError> {
    submit_inner(command, false).map(drop)
}

pub enum UndoTarget {
    Live(TrackedCommand),
    #[cfg(feature = "fixtures")]
    Local(Vec<todo_core::Event>),
}

/// Retain the command's exact batch, rather than issuing a new command that approximates its inverse.
pub fn submit_tracked(command: Command) -> Result<UndoTarget, SubmitError> {
    submit_inner(command, true)?.ok_or_else(|| local_error("no undo target"))
}

fn local_error(text: impl Into<String>) -> SubmitError {
    todo_client::StorageError::Backend(text.into()).into()
}

fn submit_inner(command: Command, tracked: bool) -> Result<Option<UndoTarget>, SubmitError> {
    if UPGRADING.with(Cell::get) {
        return Err(local_error(READ_ONLY));
    }
    BACKEND.with(|b| match &mut *b.borrow_mut() {
        Some(Backend::Live(c)) => {
            if tracked {
                c.submit_tracked(command).map(|t| Some(UndoTarget::Live(t)))
            } else {
                c.submit(command).map(|_| None)
            }
        }
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(l)) if l.read_only => Err(local_error(READ_ONLY)),
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(l)) => {
            let at = l.event_time();
            let events = todo_core::decide(&l.state, &owner(), &command, at)?;
            l.state =
                Arc::new(changes::fixture::apply_batch(&l.state, &events).map_err(local_error)?);
            Ok(tracked.then_some(UndoTarget::Local(events)))
        }
        None => Err(local_error("not started")),
    })
}

#[cfg(feature = "fixtures")]
impl Local {
    fn event_time(&mut self) -> todo_core::time::Timestamp {
        // Event times move on with the real clock so repeated commands get distinct IDs.
        #[allow(clippy::cast_possible_truncation)]
        let elapsed = (js_sys::Date::now() - self.started_ms).max(0.0) as i64;
        self.counter += 1;
        self.now
            .at
            .checked_add(jiff::SignedDuration::from_millis(
                elapsed + i64::try_from(self.counter).unwrap_or(0),
            ))
            .unwrap_or(self.now.at)
    }
}

pub fn undo(target: &UndoTarget) -> Result<TrackedUndo, String> {
    if status().read_only {
        return Err(READ_ONLY.to_owned());
    }
    BACKEND.with(|b| match (&mut *b.borrow_mut(), target) {
        (Some(Backend::Live(c)), UndoTarget::Live(t)) => {
            // Even the sender can lose the lock on reload, and browsers without Web Locks run
            // several senders. Never infer from this replica alone that a command wasn't sent.
            c.compensate_tracked(t).map_err(|e| {
                tracing::debug!(error = %e, "could not queue tracked undo");
                changes::undo_failure("").into()
            })
        }
        #[cfg(feature = "fixtures")]
        (Some(Backend::Local(l)), UndoTarget::Local(events)) => {
            let at = l.event_time();
            l.state = Arc::new(changes::fixture::undo(&l.state, events, &owner(), at)?);
            Ok(TrackedUndo::Cancelled)
        }
        _ => Ok(TrackedUndo::Unavailable),
    })
}

/// The confirmed position, not the optimistic view; a change here may mean new feed entries.
pub fn activity_revision() -> Option<(Option<todo_client::Epoch>, u64)> {
    live().map(|c| (c.epoch(), c.cursor()))
}

pub async fn activity(query: &ActivityQuery) -> Result<ActivityPage, ActivityError> {
    if let Some(c) = live() {
        return c.activity(query).await;
    }
    #[cfg(feature = "fixtures")]
    if let Some(page) = BACKEND.with(|b| {
        let b = b.borrow();
        let Some(Backend::Local(local)) = b.as_ref() else {
            return None;
        };
        let limit = query.limit.unwrap_or(50).clamp(1, 200);
        let mut matches = local.activity.iter().filter(|item| {
            let event = &item.event.event;
            query
                .before
                .is_none_or(|seq| event.seq.is_some_and(|n| n < seq))
                && query
                    .agent
                    .as_ref()
                    .is_none_or(|agent| event.actor.agent_id() == Some(agent))
        });
        let items: Vec<_> = matches.by_ref().take(limit).cloned().collect();
        let next_before = matches.next().and_then(|_| items.last()?.event.event.seq);
        Some(ActivityPage { items, next_before })
    }) {
        return Ok(page);
    }
    Err(ActivityError::Unavailable)
}

#[derive(Default)]
pub struct ActivityFlags {
    /// Undone and at the head of the undo chain, when the replica has seen the event.
    pub confirmed: Option<(bool, bool)>,
    pub pending: bool,
}

/// Reconcile the loaded row with the replica's confirmed journal and the shared outbox.
pub fn activity_flags(id: EventId, compensating: bool) -> ActivityFlags {
    let Some(c) = live() else {
        return ActivityFlags::default();
    };
    let pending = c.pending_undo(id).is_some()
        || (!c.is_sending() && SHARED_UNDOS.with(|shared| shared.borrow().contains(&id)));
    let confirmed = c.activity_confirmed(id, compensating);
    ActivityFlags { confirmed, pending }
}

/// Queue the checked, non-forced compensation. `None` is an immediate success in the fixture.
pub fn undo_activity(event: protocol::EventRef) -> Result<Option<CommandId>, &'static str> {
    if status().read_only {
        return Err(READ_ONLY);
    }
    if let Some(c) = live() {
        return c.undo_activity(event).map(Some).map_err(|error| {
            tracing::debug!(%error, "could not queue activity undo");
            changes::undo_failure("")
        });
    }
    #[cfg(feature = "fixtures")]
    return BACKEND.with(|b| {
        let mut b = b.borrow_mut();
        let Some(Backend::Local(local)) = b.as_mut() else {
            return Err(changes::undo_failure(""));
        };
        let item = local
            .activity
            .iter_mut()
            .find(|item| item.event_ref() == Some(event))
            .ok_or_else(|| changes::undo_failure("event_not_found"))?;
        if !item.can_undo {
            return Err(changes::undo_failure("already_undone"));
        }
        if event.seq == crate::fixtures::REFUSED_ACTIVITY_SEQ {
            return Err(changes::undo_failure("undo_conflict"));
        }
        item.can_undo = false;
        item.undone = true;
        Ok(None)
    });
    #[cfg(not(feature = "fixtures"))]
    Err(changes::undo_failure(""))
}

/// None while the compensation is still queued. Durable refusals also remain in Not synced.
pub fn undo_result(id: CommandId) -> Option<Result<(), &'static str>> {
    live()?.undo_result(id)
}

pub fn failures() -> Vec<Failure> {
    BACKEND.with(|b| match &*b.borrow() {
        Some(Backend::Live(c)) => {
            let failed = c.failed();
            let state = c.view();
            failed
                .iter()
                .map(|f| Failure::from_entry(f, &state))
                .collect()
        }
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(l)) => l.failures.clone(),
        None => Vec::new(),
    })
}

/// Dismiss only the entries the user saw, never re-read the failure list to choose what to remove.
pub fn dismiss_failures(ids: &[CommandId]) -> Result<(), String> {
    if status().read_only {
        return Err(READ_ONLY.to_owned());
    }
    BACKEND.with(|b| match &mut *b.borrow_mut() {
        Some(Backend::Live(c)) => c.dismiss_many(ids).map(drop).map_err(|e| e.to_string()),
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(l)) => {
            l.failures.retain(|f| !ids.contains(&f.id));
            Ok(())
        }
        None => Ok(()),
    })
}
