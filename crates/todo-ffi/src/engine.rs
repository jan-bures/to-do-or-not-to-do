//! The engine the app holds: a `todo_client::SyncClient` over the SQLite cache, the sync loop it runs
//! while the app is in the foreground, and the commands and views the screens use.
//!
//! Every method is synchronous and cheap except where noted, so Kotlin calls them from a background
//! dispatcher and never blocks the main thread on the network: commands go to the outbox and show in
//! the view at once, and the sync loop sends them when it can.

use crate::FfiError;
use crate::fixtures::{self, Fixture};
use crate::labels::{LocalNow, Now};
use crate::recovery::{self, CacheRecovery};
use crate::runtime::runtime;
use crate::transport::AppTransport;
use crate::view::{self, Home, RoomKey, RoomView, RowPlacement, SmartList, TaskDetail};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use todo_client::native::{Cleartext, HttpTransport, OsEntropy, TokioClock};
use todo_client::outbox::OutboxEntry;
use todo_client::sqlite::SqliteStorage;
use todo_client::storage::{Storage, StorageError, StorageOp};
use todo_client::{ClientConfig, Connection, ErrorKind, MemoryStorage, SyncClient};
use todo_core::command::{NewTask, Placement, TaskPatch};
use todo_core::ids::{CollectionId, ListId, StepId, TaskId};
use todo_core::model::Priority;
use todo_core::{Actor, Command, State};
use tokio::task::JoinHandle;

type Client = SyncClient<AppTransport, TokioClock>;

/// How the app reaches its server, from pairing.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct EngineConfig {
    /// The server's origin, as [`crate::normalize_server_address`] returned it.
    pub server_url: String,
    /// The device token (`tdd_…`). Kept in memory only.
    pub token: String,
    /// The device ID the server issued with the token.
    pub device_id: String,
    /// The SQLite cache file (the app's private storage).
    pub database_path: String,
    /// Allow plain `http` to a loopback server. Only a debug build of the app sets it (the emulator
    /// workflow); otherwise the transport refuses anything but `https`.
    pub allow_loopback_http: bool,
}

impl std::fmt::Debug for EngineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineConfig")
            .field("server_url", &self.server_url)
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// Called (on a background thread) whenever the view or the sync status may have changed.
#[uniffi::export(with_foreign)]
pub trait ChangeListener: Send + Sync {
    /// Something changed: re-read what the screen shows.
    fn on_change(&self);
}

/// Connectivity as the status line shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SyncState {
    /// The app is in the background, or has never started syncing.
    Paused,
    /// The server can't be reached.
    Offline,
    /// Connected, catching up or sending.
    Syncing,
    /// Following the live stream with nothing to send.
    Online,
}

/// What kind of problem the last error was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ProblemKind {
    /// No connection or no answer.
    Network,
    /// The token was refused: pair again.
    Unauthorized,
    /// The server answered with an error.
    Server,
    /// The answer wasn't understood.
    Protocol,
    /// The local cache failed.
    Storage,
}

/// The sync status.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncStatus {
    /// Connectivity.
    pub state: SyncState,
    /// Changes not yet confirmed by the server.
    pub pending: u32,
    /// Changes the server refused.
    pub failed: u32,
    /// The most recent problem, cleared once online.
    pub problem: Option<ProblemKind>,
    /// Its description (never task content).
    pub problem_detail: Option<String>,
}

/// A change the server refused.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FailedChange {
    /// The command ID, to dismiss it.
    pub id: String,
    /// What it was ("Complete a task").
    pub what: String,
    /// Why the server refused it.
    pub reason: String,
}

/// The sync engine and view model.
#[derive(uniffi::Object)]
pub struct TodoEngine {
    client: Client,
    recovered: Option<CacheRecovery>,
    run: Mutex<Option<JoinHandle<()>>>,
    watch: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for TodoEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TodoEngine").finish_non_exhaustive()
    }
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn invalid(detail: impl Into<String>) -> FfiError {
    FfiError::Invalid {
        detail: detail.into(),
    }
}

fn task_id(id: &str) -> Result<TaskId, FfiError> {
    id.parse().map_err(|_| invalid("no such task"))
}

fn describe(entry: &OutboxEntry) -> &'static str {
    match entry.command() {
        Some(Command::CreateTask(_)) => "Add a task",
        Some(Command::UpdateTask { .. }) => "Edit a task",
        Some(Command::CompleteTask { .. }) => "Complete a task",
        Some(Command::UncompleteTask { .. }) => "Reopen a task",
        Some(Command::SetMyDay { .. }) => "Change My Day",
        Some(Command::CreateList { .. }) => "Add a list",
        Some(Command::CreateStep { .. }) => "Add a step",
        Some(Command::CompleteStep { .. }) => "Complete a step",
        Some(Command::UncompleteStep { .. }) => "Reopen a step",
        Some(_) => "A change",
        None => "Undo",
    }
}

impl TodoEngine {
    fn new(client: Client, recovered: Option<CacheRecovery>) -> Arc<Self> {
        Arc::new(Self {
            client,
            recovered,
            run: Mutex::new(None),
            watch: Mutex::new(None),
        })
    }

    /// The optimistic view (for tests).
    pub fn state(&self) -> Arc<State> {
        self.client.view()
    }

    fn is_running(&self) -> bool {
        locked(&self.run).as_ref().is_some_and(|h| !h.is_finished())
    }

    /// Where a new task in `room` goes, and what it starts with: (list, in My Day, priority, due today).
    fn target(&self, state: &State, room: &RoomKey) -> (Option<ListId>, bool, Priority, bool) {
        let live = |id: ListId| view::list_live(state, id);
        let ordered = view::ordered_lists(state);
        let first_live = ordered.iter().map(|l| l.id).find(|id| live(*id));
        match room {
            RoomKey::List { id } => {
                let list = id.parse().ok().filter(|l| live(*l));
                (list.or(first_live), false, Priority::None, false)
            }
            RoomKey::Collection { id } => {
                let col: Option<CollectionId> = id.parse().ok();
                let list = ordered
                    .iter()
                    .find(|l| l.collection.is_some() && l.collection == col)
                    .map(|l| l.id);
                (list.or(first_live), false, Priority::None, false)
            }
            RoomKey::Smart { list } => match list {
                SmartList::MyDay => (first_live, true, Priority::None, false),
                SmartList::Important => (first_live, false, Priority::High, false),
                SmartList::Planned => (first_live, false, Priority::None, true),
                _ => (first_live, false, Priority::None, false),
            },
        }
    }
}

#[uniffi::export]
impl TodoEngine {
    /// Opens the engine on the cache at `config.database_path` for the paired server. Doesn't touch the
    /// network: call [`start`](Self::start) when the app comes to the foreground.
    #[uniffi::constructor]
    pub fn open(config: EngineConfig) -> Result<Arc<Self>, FfiError> {
        let server = crate::pairing::normalize_server_address(
            config.server_url.clone(),
            config.allow_loopback_http,
        )?;
        let actor = Actor::owner(&config.device_id).map_err(|e| invalid(e.to_string()))?;
        let cleartext = if config.allow_loopback_http {
            Cleartext::Loopback
        } else {
            Cleartext::Refuse
        };
        let transport = HttpTransport::with_cleartext(&server, Some(&config.token), cleartext)
            .map_err(|e| FfiError::BadAddress {
                reason: e.to_string(),
            })?;
        let files = recovery::Files::new(Path::new(&config.database_path));
        let storage_error = |e: recovery::RecoveryError| FfiError::Storage {
            detail: e.to_string(),
        };
        // Finish a recovery an earlier open started before anything opens the cache, so an empty cache
        // is never opened while unsent changes sit in the quarantined copy.
        let mut recovered = recovery::resume(&files).map_err(storage_error)?;
        let open = || -> Result<Client, StorageError> {
            let storage = SqliteStorage::open(&files.cache)?;
            SyncClient::new(
                AppTransport::Http(transport.clone()),
                TokioClock,
                OsEntropy,
                storage,
                ClientConfig::new(actor.clone()),
            )
            .map_err(|todo_client::replica::OpenError::Storage(e)| e)
        };
        let client = match open() {
            Err(StorageError::Corrupt(_)) if recovered.is_none() => {
                tracing::warn!(
                    "the local cache is damaged; setting it aside and keeping its unsent changes"
                );
                recovered = Some(recovery::recover(&files).map_err(storage_error)?);
                open()?
            }
            other => other?,
        };
        Ok(Self::new(client, recovered))
    }

    /// What opening carried over from a damaged cache, if it had to set one aside: the app tells the
    /// owner. `None` when the cache opened as it was.
    pub fn recovered(&self) -> Option<CacheRecovery> {
        self.recovered
    }

    /// An offline engine on invented data (`world`, `finished` or `blank`) dated relative to `now`, for
    /// the screenshot gallery and previews. It never touches the network.
    #[uniffi::constructor]
    pub fn fixture(name: String, now: LocalNow) -> Result<Arc<Self>, FfiError> {
        let fixture = Fixture::by_name(&name).ok_or_else(|| invalid("no such fixture"))?;
        let state = fixtures::build(fixture, &Now::new(now));
        let mut storage = MemoryStorage::new();
        storage.write(&[
            StorageOp::SetEpoch(uuid::Uuid::nil()),
            StorageOp::Snapshot {
                state: &state,
                cursor: 0,
            },
        ])?;
        let actor = Actor::owner("fixture-phone").map_err(|e| invalid(e.to_string()))?;
        let client = SyncClient::new(
            AppTransport::Offline,
            TokioClock,
            OsEntropy,
            storage,
            ClientConfig::new(actor),
        )
        .map_err(|e| FfiError::Storage {
            detail: e.to_string(),
        })?;
        Ok(Self::new(client, None))
    }

    /// Starts syncing (the app came to the foreground): sends the outbox and follows the live event
    /// stream until [`stop`](Self::stop). Does nothing if it is already running.
    pub fn start(&self) -> Result<(), FfiError> {
        let mut run = locked(&self.run);
        if run.as_ref().is_some_and(|h| !h.is_finished()) {
            return Ok(());
        }
        let client = self.client.clone();
        *run = Some(runtime()?.spawn(async move { client.run().await }));
        drop(run);
        self.client.nudge();
        Ok(())
    }

    /// Stops syncing (the app went to the background). In-flight requests are abandoned; the server is
    /// idempotent by command ID, so a resend after [`start`](Self::start) is safe. The outbox stays.
    /// [`status`](Self::status) reports `Paused` from now on; the listener isn't called for that.
    pub fn stop(&self) {
        if let Some(h) = locked(&self.run).take() {
            h.abort();
        }
    }

    /// Retries now instead of waiting out the backoff (the network came back).
    pub fn nudge(&self) {
        self.client.nudge();
    }

    /// Calls `listener` whenever the view or the status may have changed, replacing any earlier one.
    pub fn set_listener(&self, listener: Arc<dyn ChangeListener>) -> Result<(), FfiError> {
        let mut changes = self.client.changes();
        let task = runtime()?.spawn(async move {
            loop {
                changes.next().await;
                listener.on_change();
            }
        });
        if let Some(old) = locked(&self.watch).replace(task) {
            old.abort();
        }
        Ok(())
    }

    /// The sync status.
    pub fn status(&self) -> SyncStatus {
        let s = self.client.status();
        let state = if !self.is_running() {
            SyncState::Paused
        } else {
            match s.connection {
                Connection::Offline => SyncState::Offline,
                Connection::Syncing => SyncState::Syncing,
                Connection::Online => SyncState::Online,
            }
        };
        SyncStatus {
            state,
            pending: u32::try_from(s.pending).unwrap_or(u32::MAX),
            failed: u32::try_from(s.failed).unwrap_or(u32::MAX),
            problem: s.last_error.as_ref().map(|e| match e.kind {
                ErrorKind::Network => ProblemKind::Network,
                ErrorKind::Unauthorized => ProblemKind::Unauthorized,
                ErrorKind::Server => ProblemKind::Server,
                ErrorKind::Protocol => ProblemKind::Protocol,
                ErrorKind::Storage => ProblemKind::Storage,
            }),
            problem_detail: s.last_error.map(|e| e.message),
        }
    }

    /// The navigation.
    pub fn home(&self, now: LocalNow) -> Home {
        view::home(&self.client.view(), &Now::new(now))
    }

    /// A room.
    pub fn room(&self, key: RoomKey, now: LocalNow) -> RoomView {
        view::room(&self.client.view(), &key, &Now::new(now))
    }

    /// A room showing only the tasks matching `filter`: the query language when it parses
    /// (`tag:docs`, `due<=friday`), else words in the title. The progress stays the whole room's.
    pub fn room_matching(&self, key: RoomKey, filter: String, now: LocalNow) -> RoomView {
        view::room_filtered(&self.client.view(), &key, &filter, &Now::new(now))
    }

    /// The navigation, counting the tasks in `placement` as they were (see [`view::placed`]).
    pub fn home_placed(&self, placement: RowPlacement, now: LocalNow) -> Home {
        let state = self.client.view();
        if placement.is_empty() {
            return view::home(&state, &Now::new(now));
        }
        view::home(&view::placed(&state, &placement), &Now::new(now))
    }

    /// A room, filtered as [`TodoEngine::room_matching`] (an empty filter shows everything), with the tasks
    /// in `placement` kept where they were while their choreography plays (see [`view::placed`]).
    pub fn room_placed(
        &self,
        key: RoomKey,
        filter: String,
        placement: RowPlacement,
        now: LocalNow,
    ) -> RoomView {
        let state = self.client.view();
        let now = Now::new(now);
        if placement.is_empty() {
            return view::room_filtered(&state, &key, &filter, &now);
        }
        view::room_filtered(&view::placed(&state, &placement), &key, &filter, &now)
    }

    /// Whether a task is done now, or `None` when it is gone (deleted, merged, in a deleted list, or unknown): what the app's
    /// motion follows when the data changes under a choreography.
    pub fn task_done(&self, id: String) -> Option<bool> {
        let id: TaskId = id.parse().ok()?;
        let state = self.client.view();
        state
            .tasks
            .get(&id)
            .filter(|t| view::task_live(&state, t))
            .map(|t| t.is_done())
    }

    /// A task's detail, or `None` if it is gone.
    pub fn task(&self, id: String, now: LocalNow) -> Option<TaskDetail> {
        view::detail(&self.client.view(), &id, &Now::new(now))
    }

    /// Adds a task to `room` and returns its ID. In My Day it lands in today's My Day, in Important it is
    /// high priority, in Planned it is due today; a smart list's task goes to the first list (a "Tasks"
    /// list is created when there is none).
    pub fn add_task(
        &self,
        room: RoomKey,
        title: String,
        now: LocalNow,
    ) -> Result<String, FfiError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(invalid("A task needs a title."));
        }
        let today = Now::new(now).today();
        let state = self.client.view();
        let (list, my_day, priority, due_today) = self.target(&state, &room);
        let list = match list {
            Some(l) => l,
            None => {
                let id = ListId::from_uuid(self.client.new_uuid());
                self.client.submit(Command::CreateList {
                    id,
                    collection: None,
                    name: "Tasks".into(),
                    color: None,
                    placement: Placement::Last,
                })?;
                id
            }
        };
        let id = TaskId::from_uuid(self.client.new_uuid());
        let mut task = NewTask::new(id, list, title);
        task.my_day = my_day.then_some(today);
        task.priority = priority;
        task.due = due_today.then_some(today);
        self.client.submit(Command::CreateTask(Box::new(task)))?;
        Ok(id.to_string())
    }

    /// Completes or reopens a task.
    pub fn set_done(&self, id: String, done: bool) -> Result<(), FfiError> {
        let id = task_id(&id)?;
        let already = self
            .client
            .view()
            .tasks
            .get(&id)
            .is_some_and(|t| t.is_done() == done);
        if already {
            return Ok(());
        }
        self.client.submit(if done {
            Command::CompleteTask { id }
        } else {
            Command::UncompleteTask { id }
        })?;
        Ok(())
    }

    /// Renames a task.
    pub fn rename(&self, id: String, title: String) -> Result<(), FfiError> {
        let id = task_id(&id)?;
        let title = title.trim();
        if title.is_empty() {
            return Err(invalid("A task needs a title."));
        }
        if self
            .client
            .view()
            .tasks
            .get(&id)
            .is_some_and(|t| t.title == title)
        {
            return Ok(());
        }
        self.client.submit(Command::UpdateTask {
            id,
            patch: TaskPatch {
                title: Some(title.to_owned()),
                ..TaskPatch::default()
            },
        })?;
        Ok(())
    }

    /// Puts a task in today's My Day or takes it out.
    pub fn set_my_day(&self, id: String, on: bool, now: LocalNow) -> Result<(), FfiError> {
        let id = task_id(&id)?;
        let today = Now::new(now).today();
        self.client.submit(Command::SetMyDay {
            id,
            date: on.then_some(today),
        })?;
        Ok(())
    }

    /// Marks a task important (high priority), or takes its priority away, as the desktop's star does.
    pub fn set_important(&self, id: String, on: bool) -> Result<(), FfiError> {
        let id = task_id(&id)?;
        let priority = if on { Priority::High } else { Priority::None };
        if self
            .client
            .view()
            .tasks
            .get(&id)
            .is_some_and(|t| (t.priority == Priority::High) == on)
        {
            return Ok(());
        }
        self.client.submit(Command::UpdateTask {
            id,
            patch: TaskPatch {
                priority: Some(priority),
                ..TaskPatch::default()
            },
        })?;
        Ok(())
    }

    /// Adds a step at the end of a task's steps and returns its ID.
    pub fn add_step(&self, task: String, title: String) -> Result<String, FfiError> {
        let task = task_id(&task)?;
        let title = title.trim();
        if title.is_empty() {
            return Err(invalid("A step needs a title."));
        }
        let id = StepId::from_uuid(self.client.new_uuid());
        self.client.submit(Command::CreateStep {
            id,
            task,
            title: title.to_owned(),
            placement: Placement::Last,
        })?;
        Ok(id.to_string())
    }

    /// Completes or reopens a step.
    pub fn set_step_done(&self, id: String, done: bool) -> Result<(), FfiError> {
        let id: StepId = id.parse().map_err(|_| invalid("no such step"))?;
        let already = self
            .client
            .view()
            .steps
            .get(&id)
            .is_some_and(|s| s.completed_at.is_some() == done);
        if already {
            return Ok(());
        }
        self.client.submit(if done {
            Command::CompleteStep { id }
        } else {
            Command::UncompleteStep { id }
        })?;
        Ok(())
    }

    /// Adds a top-level list at the end, with a swatch picked from its ID, and returns its room.
    pub fn create_list(&self, name: String) -> Result<RoomKey, FfiError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(invalid("A list needs a name."));
        }
        let id = ListId::from_uuid(self.client.new_uuid());
        let swatch = crate::lists::swatch(None, id.as_uuid().as_bytes());
        self.client.submit(Command::CreateList {
            id,
            collection: None,
            name: name.to_owned(),
            color: Some(swatch.to_owned()),
            placement: Placement::Last,
        })?;
        Ok(RoomKey::List { id: id.to_string() })
    }

    /// Changes the server refused, oldest first.
    pub fn failed(&self) -> Vec<FailedChange> {
        self.client
            .failed()
            .into_iter()
            .map(|f| FailedChange {
                id: f.entry.command_id.to_string(),
                what: describe(&f.entry).into(),
                reason: f.rejection.message,
            })
            .collect()
    }

    /// Removes a refused change from the list.
    pub fn dismiss_failed(&self, id: String) -> Result<bool, FfiError> {
        let id = id.parse().map_err(|_| invalid("no such change"))?;
        Ok(self.client.dismiss(id)?)
    }
}

impl Drop for TodoEngine {
    fn drop(&mut self) {
        for slot in [&self.run, &self.watch] {
            if let Some(h) = locked(slot).take() {
                h.abort();
            }
        }
    }
}
