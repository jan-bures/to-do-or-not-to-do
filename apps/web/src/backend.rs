//! Where the state comes from and where commands go.
//!
//! - **Live** (the real app): a `todo_client::SyncClient` over `fetch`, with the replica mirrored to
//!   IndexedDB so pending edits survive a reload or a closed tab, and the event stream read with a
//!   streaming `fetch` (see `docs/web.md`). Requests carry the session cookie and the CSRF token from the
//!   page; there is no token in the browser.
//! - **Fixtures** (the screenshot gallery, `--features fixtures`): invented data with a fixed clock,
//!   changed through `decide` and `apply` like the server would, and never sent anywhere.
//!
//! The backend lives in a thread-local, so the UI's reactive closures (which must be `Send`) only hold
//! signals and call the free functions here.
// The fixtures build never opens the live backend.
#![cfg_attr(feature = "fixtures", allow(dead_code))]

#[cfg(feature = "fixtures")]
use crate::model::Now;
use std::cell::RefCell;
use std::sync::Arc;
use todo_client::web::{BrowserClock, BrowserEntropy, FetchTransport, IdbFlusher, open_storage};
use todo_client::{ClientConfig, Connection, ErrorKind, SyncClient};
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
    started_ms: f64,
    counter: u64,
}

thread_local! {
    static BACKEND: RefCell<Option<Backend>> = const { RefCell::new(None) };
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
}

impl Default for Sync {
    fn default() -> Self {
        Self {
            connection: Connection::Syncing,
            pending: 0,
            failed: 0,
            signed_out: false,
            offline_demo: false,
        }
    }
}

/// Why the app could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The page has no CSRF token: it was not served by `todo-server`.
    #[error("this page was not served by the to-do server")]
    NoCsrf,
    /// IndexedDB is not available (private mode in some browsers) or failed.
    #[error("local storage is unavailable: {0}")]
    Storage(String),
}

fn owner() -> Actor {
    Actor::owner(DEVICE).unwrap_or(Actor::System)
}

/// Opens the live backend: the replica from IndexedDB (or empty) and the sync engine. Returns the
/// flusher, which the caller runs alongside [`run`].
pub async fn open_live(csrf: String) -> Result<IdbFlusher, StartError> {
    let open = || async {
        let (storage, flusher) = open_storage(DB_NAME)
            .await
            .map_err(|e| StartError::Storage(e.0))?;
        let client = SyncClient::new(
            FetchTransport::new(csrf.clone()),
            BrowserClock,
            BrowserEntropy,
            storage,
            ClientConfig::new(owner()),
        )
        .map_err(|e| StartError::Storage(e.to_string()))?;
        Ok::<_, StartError>((client, flusher))
    };
    let (client, flusher) = match open().await {
        Ok(v) => v,
        Err(e) => {
            // An unreadable replica is a cache: drop it and start over from the server.
            tracing::warn!("replica not opened, starting fresh: {e}");
            todo_client::web::delete_storage(DB_NAME)
                .await
                .map_err(|e| StartError::Storage(e.0))?;
            open().await?
        }
    };
    BACKEND.with(|b| *b.borrow_mut() = Some(Backend::Live(client)));
    Ok(flusher)
}

/// The fixtures backend with this state.
#[cfg(feature = "fixtures")]
pub fn open_fixtures(state: State, now: Now) {
    BACKEND.with(|b| {
        *b.borrow_mut() = Some(Backend::Local(Local {
            state: Arc::new(state),
            now,
            started_ms: js_sys::Date::now(),
            counter: 0,
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
    match live() {
        Some(c) => c.changes().next().await,
        None => std::future::pending::<()>().await,
    }
}

/// Asks the engine to retry now (the tab became visible, or the network came back).
pub fn nudge() {
    if let Some(c) = live() {
        c.nudge();
    }
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
                signed_out: s
                    .last_error
                    .is_some_and(|e| e.kind == ErrorKind::Unauthorized),
                offline_demo: false,
            }
        }
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(_)) => Sync {
            connection: Connection::Online,
            offline_demo: true,
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

/// Queues a command; the view reflects it at once. Fails if the command is invalid against the view.
pub fn submit(command: Command) -> Result<(), String> {
    BACKEND.with(|b| match &mut *b.borrow_mut() {
        Some(Backend::Live(c)) => c.submit(command).map(|_| ()).map_err(|e| e.to_string()),
        #[cfg(feature = "fixtures")]
        Some(Backend::Local(l)) => {
            // Event times move on with the real clock so repeated commands get distinct IDs.
            #[allow(clippy::cast_possible_truncation)]
            let elapsed = (js_sys::Date::now() - l.started_ms).max(0.0) as i64;
            l.counter += 1;
            let at = l
                .now
                .at
                .checked_add(jiff::SignedDuration::from_millis(
                    elapsed + i64::try_from(l.counter).unwrap_or(0),
                ))
                .unwrap_or(l.now.at);
            let events =
                todo_core::decide(&l.state, &owner(), &command, at).map_err(|e| e.to_string())?;
            let mut next = (*l.state).clone();
            for e in &events {
                todo_core::apply(&mut next, e).map_err(|e| e.to_string())?;
            }
            l.state = Arc::new(next);
            Ok(())
        }
        None => Err("not started".to_owned()),
    })
}
