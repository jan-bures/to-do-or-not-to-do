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
use todo_client::intent::{self, TaskRoom};
use todo_client::native::{Cleartext, HttpTransport, OsEntropy, TokioClock};
use todo_client::outbox::OutboxEntry;
use todo_client::sqlite::SqliteStorage;
use todo_client::storage::{Storage, StorageError, StorageOp};
use todo_client::{ClientConfig, Connection, ErrorKind, MemoryStorage, SyncClient};
use todo_core::ids::{EntityId, ListId, StepId, TaskId};
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
    /// Built by [`TodoEngine::fixture`]: only such an engine takes the test hooks
    /// ([`TodoEngine::fixture_delete_task`], [`TodoEngine::fixture_restore_task`]).
    fixture: bool,
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
    fn new(client: Client, recovered: Option<CacheRecovery>, fixture: bool) -> Arc<Self> {
        Arc::new(Self {
            client,
            recovered,
            fixture,
            run: Mutex::new(None),
            watch: Mutex::new(None),
        })
    }

    /// Refuses a test hook on an engine that isn't a fixture.
    fn fixture_only(&self) -> Result<(), FfiError> {
        if self.fixture {
            Ok(())
        } else {
            Err(invalid("only a fixture engine takes this"))
        }
    }

    /// The optimistic view (for tests).
    pub fn state(&self) -> Arc<State> {
        self.client.view()
    }

    fn is_running(&self) -> bool {
        locked(&self.run).as_ref().is_some_and(|h| !h.is_finished())
    }
}

fn intent_room(room: RoomKey) -> TaskRoom {
    match room {
        RoomKey::List { id } => id.parse().map_or(TaskRoom::Other, TaskRoom::List),
        RoomKey::Collection { id } => id.parse().map_or(TaskRoom::Other, TaskRoom::Collection),
        RoomKey::Smart { list } => match list {
            SmartList::MyDay => TaskRoom::MyDay,
            SmartList::Important => TaskRoom::Important,
            SmartList::Planned => TaskRoom::Planned,
            SmartList::All | SmartList::Completed | SmartList::AgentActivity => TaskRoom::Other,
        },
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
        Ok(Self::new(client, recovered, false))
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
        Ok(Self::new(client, None, true))
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

    /// The navigation, counting the tasks held or being reopened in `placement` as they were (see
    /// [`view::placed`]).
    pub fn home_placed(&self, placement: RowPlacement, now: LocalNow) -> Home {
        let state = self.client.view();
        if placement.is_empty() {
            return view::home(&state, &Now::new(now));
        }
        view::home(&view::placed(&state, &placement), &Now::new(now))
    }

    /// A room, filtered as [`TodoEngine::room_matching`] (an empty filter shows everything), with the tasks
    /// in `placement` kept where they were while their choreography plays (see [`view::room_placed`]).
    pub fn room_placed(
        &self,
        key: RoomKey,
        filter: String,
        placement: RowPlacement,
        now: LocalNow,
    ) -> RoomView {
        view::room_placed(
            &self.client.view(),
            &key,
            &filter,
            &placement,
            &Now::new(now),
        )
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

    /// Adds a task at the top of its list and returns its ID. The text may have a trailing date and
    /// priority. The current room picks the list and supplies defaults for My Day, Important and Planned.
    pub fn add_task(
        &self,
        room: RoomKey,
        title: String,
        now: LocalNow,
    ) -> Result<String, FfiError> {
        let (id, commands) = intent::quick_add_task(
            &self.client.view(),
            intent_room(room),
            &title,
            &Now::new(now).eval(),
            || self.client.new_uuid(),
        );
        let id = id.ok_or_else(|| invalid("A task needs a title."))?;
        for command in commands {
            self.client.submit(command)?;
        }
        Ok(id.to_string())
    }

    /// Completes or reopens a task.
    pub fn set_done(&self, id: String, done: bool) -> Result<(), FfiError> {
        let id = task_id(&id)?;
        if let Some(command) = intent::set_done(&self.client.view(), id, done) {
            self.client.submit(command)?;
        }
        Ok(())
    }

    /// A test hook, refused unless this is a [`fixture`](Self::fixture) engine (the app has no way to
    /// delete a task yet): deletes a task, as another device would. A tombstone:
    /// [`fixture_restore_task`](Self::fixture_restore_task) brings it back.
    pub fn fixture_delete_task(&self, id: String) -> Result<(), FfiError> {
        self.fixture_only()?;
        let id = task_id(&id)?;
        self.client.submit(intent::delete(id))?;
        Ok(())
    }

    /// A test hook, refused unless this is a [`fixture`](Self::fixture) engine: restores a deleted task,
    /// as another device would. Only what deleting it changed comes back: a task merged into another
    /// keeps its `merged_into`, so it stays gone.
    pub fn fixture_restore_task(&self, id: String) -> Result<(), FfiError> {
        self.fixture_only()?;
        let id = task_id(&id)?;
        self.client.submit(Command::Restore {
            id: EntityId::Task(id),
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
        let state = self.client.view();
        if !state.tasks.contains_key(&id) {
            return Err(invalid("No such task."));
        }
        if let Some(command) = intent::retitle(&state, id, title) {
            self.client.submit(command)?;
        }
        Ok(())
    }

    /// Puts a task in today's My Day or takes it out.
    pub fn set_my_day(&self, id: String, on: bool, now: LocalNow) -> Result<(), FfiError> {
        let id = task_id(&id)?;
        let today = Now::new(now).today();
        self.client
            .submit(intent::set_my_day(id, on.then_some(today)))?;
        Ok(())
    }

    /// Marks a task important (high priority), or takes its priority away, as the desktop's star does.
    pub fn set_important(&self, id: String, on: bool) -> Result<(), FfiError> {
        let id = task_id(&id)?;
        if let Some(command) = intent::set_important(&self.client.view(), id, on) {
            self.client.submit(command)?;
        }
        Ok(())
    }

    /// Adds a step at the end of a task's steps and returns its ID.
    pub fn add_step(&self, task: String, title: String) -> Result<String, FfiError> {
        let task = task_id(&task)?;
        let Some(command @ Command::CreateStep { id, .. }) =
            intent::add_step(task, &title, || self.client.new_uuid())
        else {
            return Err(invalid("A step needs a title."));
        };
        self.client.submit(command)?;
        Ok(id.to_string())
    }

    /// Completes or reopens a step.
    pub fn set_step_done(&self, id: String, done: bool) -> Result<(), FfiError> {
        let id: StepId = id.parse().map_err(|_| invalid("no such step"))?;
        if let Some(command) = intent::set_step_done(&self.client.view(), id, done) {
            self.client.submit(command)?;
        }
        Ok(())
    }

    /// Adds a list at the end of the room's current collection (or at the top level), and returns
    /// its room.
    pub fn create_list(&self, name: String, room: RoomKey) -> Result<RoomKey, FfiError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(invalid("A list needs a name."));
        }
        let collection = intent::list_collection(&self.client.view(), intent_room(room));
        let id = ListId::from_uuid(self.client.new_uuid());
        self.client
            .submit(intent::create_list(id, collection, name))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::ids::CollectionId;
    use todo_core::time::Timestamp;
    use uuid::Uuid;

    #[test]
    fn ffi_rooms_map_to_shared_task_rooms() {
        let collection = CollectionId::from_uuid(Uuid::from_u128(1));
        let list = ListId::from_uuid(Uuid::from_u128(2));
        let smart = |list| RoomKey::Smart { list };
        for (room, expected) in [
            (smart(SmartList::MyDay), TaskRoom::MyDay),
            (smart(SmartList::Important), TaskRoom::Important),
            (smart(SmartList::Planned), TaskRoom::Planned),
            (smart(SmartList::All), TaskRoom::Other),
            (smart(SmartList::Completed), TaskRoom::Other),
            (smart(SmartList::AgentActivity), TaskRoom::Other),
            (
                RoomKey::Collection {
                    id: collection.to_string(),
                },
                TaskRoom::Collection(collection),
            ),
            (
                RoomKey::List {
                    id: list.to_string(),
                },
                TaskRoom::List(list),
            ),
            (
                RoomKey::Collection {
                    id: "bad id".into(),
                },
                TaskRoom::Other,
            ),
            (
                RoomKey::List {
                    id: "bad id".into(),
                },
                TaskRoom::Other,
            ),
        ] {
            assert_eq!(intent_room(room.clone()), expected, "{room:?}");
        }
    }

    #[test]
    fn ffi_submits_shared_intents() {
        let at: Timestamp = "2026-11-21T10:35:00Z".parse().expect("timestamp");
        let now = LocalNow {
            epoch_ms: at.as_millisecond(),
            utc_offset_secs: 0,
        };
        let engine = TodoEngine::fixture("blank".into(), now).expect("fixture");
        let before = engine.state();
        let text = "Pay the invoice by fri !!";
        let id: TaskId = engine
            .add_task(RoomKey::default(), text.into(), now)
            .expect("task added")
            .parse()
            .expect("task ID");
        let list = engine.state().tasks[&id].list;
        let mut ids = [list.as_uuid(), id.as_uuid()].into_iter();
        let expected = intent::quick_add_task(
            &before,
            TaskRoom::MyDay,
            text,
            &Now::new(now).eval(),
            || ids.next().expect("fresh ID"),
        );
        let pending = engine.client.pending();
        assert_eq!(Some(id), expected.0);
        assert_eq!(
            pending
                .iter()
                .filter_map(OutboxEntry::command)
                .collect::<Vec<_>>(),
            expected.1.iter().collect::<Vec<_>>()
        );

        let engine = TodoEngine::fixture("world".into(), now).expect("fixture");
        let state = engine.state();
        let &collection = state.collections.keys().next().expect("collection");
        let RoomKey::List { id } = engine
            .create_list(
                " Garden ".into(),
                RoomKey::Collection {
                    id: collection.to_string(),
                },
            )
            .expect("list added")
        else {
            panic!("expected a list room");
        };
        let id: ListId = id.parse().expect("list ID");
        let pending = engine.client.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].command(),
            Some(&intent::create_list(id, Some(collection), "Garden"))
        );
        assert_eq!(engine.state().lists[&id].collection, Some(collection));

        // Pass only a room key captured before another command moves its list. The new list must
        // follow the current collection, not the one in the earlier sidebar snapshot.
        let room = RoomKey::List { id: id.to_string() };
        let other = *state
            .collections
            .keys()
            .find(|&&c| c != collection)
            .expect("second collection");
        engine
            .client
            .submit(Command::MoveList {
                id,
                collection: Some(other),
                placement: todo_core::command::Placement::Last,
            })
            .expect("move list");
        let RoomKey::List { id: sibling } = engine
            .create_list("Sibling".into(), room)
            .expect("list added")
        else {
            panic!("expected a list room");
        };
        assert_eq!(
            engine.state().lists[&sibling.parse().expect("list ID")].collection,
            Some(other)
        );
    }
}
