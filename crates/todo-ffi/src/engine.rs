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
use crate::view::{
    self, Home, ReminderRef, RoomKey, RoomView, RowPlacement, SmartList, TaskDetail,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use todo_client::intent::reminder::{self, REMINDER_CHANGED, REMINDER_PAST};
use todo_client::intent::{self, TaskRoom};
use todo_client::native::{Cleartext, HttpTransport, OsEntropy, TokioClock};
use todo_client::outbox::OutboxEntry;
use todo_client::sqlite::SqliteStorage;
use todo_client::storage::{Storage, StorageError, StorageOp};
use todo_client::{
    ClientConfig, CommandId, Connection, ErrorKind, MemoryStorage, Revocation, SyncClient,
    TrackedCommand, TrackedUndo,
};
use todo_core::ids::{CollectionId, EntityId, ListId, StepId, TaskId};
use todo_core::model::Entity;
use todo_core::time::{Date, Timestamp};
use todo_core::{Actor, Command, State};
use tokio::task::JoinHandle;

pub mod activity;
pub mod attachments;
pub mod bulk;
pub mod merge;
pub mod organize;
pub mod trash;

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
    /// The SQLite cache file (in the app's private storage, with a separate path for each pairing).
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
    /// The current connection's backlog is in. An open stream alone does not count.
    pub caught_up: bool,
    /// A sync request received a 401 on this engine, even if a later error replaced it.
    pub revoked: bool,
    /// The most recent problem, cleared once online.
    pub problem: Option<ProblemKind>,
    /// Its description (never task content).
    pub problem_detail: Option<String>,
}

/// The drawer dot's tone, derived from the same status as its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SyncTone {
    Ok,
    Busy,
    Problem,
    Idle,
}

/// The drawer's sync label and dot.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncText {
    pub short: String,
    pub full: String,
    pub tone: SyncTone,
}

#[uniffi::export]
pub fn sync_status_text(status: SyncStatus, local_time: Option<String>) -> SyncText {
    use todo_client::sync_label::SyncLabel;

    let connection = match status.state {
        SyncState::Paused => None,
        SyncState::Offline => Some(Connection::Offline),
        SyncState::Syncing => Some(Connection::Syncing),
        SyncState::Online => Some(Connection::Online),
    };
    let label = SyncLabel::for_client(
        connection,
        status.pending as usize,
        status.failed as usize,
        status.revoked,
        local_time.as_deref(),
    );
    let tone = match label {
        SyncLabel::Synced(_) => SyncTone::Ok,
        SyncLabel::Syncing => SyncTone::Busy,
        SyncLabel::Paused(_) => SyncTone::Idle,
        _ => SyncTone::Problem,
    };
    let text = label.text();
    SyncText {
        short: text.short,
        full: text.full,
        tone,
    }
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

/// An opaque reference to the exact completion (including its generated next occurrence).
/// Kept by the toast, never rebuilt from a task ID or timestamp.
#[derive(Debug, uniffi::Object)]
pub struct Completion(TrackedCommand);

/// An opaque reference to an exact deletion (a task, a step, or a list with its tasks), for its Undo. Kept by the
/// toast, like [`Completion`].
#[derive(Debug, uniffi::Object)]
pub struct Deletion {
    tracked: TrackedCommand,
    entity: EntityId,
}

/// The exact move or reorder the toast can undo.
#[derive(Debug, uniffi::Object)]
pub struct TaskMove(TrackedCommand);

/// The exact reminder change (set, moved or removed) the toast can undo.
#[derive(Debug, uniffi::Object)]
pub struct ReminderChange(TrackedCommand);

/// A reminder the phone alerts for (`intent::reminder::alarms`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ReminderAlarm {
    /// Reminder ID: one alarm per reminder.
    pub id: String,
    /// Its task, for the notification's actions.
    pub task: String,
    /// The task's title, on one line (task content: never log it).
    pub title: String,
    /// When it fires, in milliseconds since the Unix epoch.
    pub at_ms: i64,
    /// When it fires, in words: "Today, 15:30".
    pub label: String,
}

/// One destination in the Move to… picker.
#[derive(Debug, uniffi::Record)]
pub struct Destination {
    /// List ID, never its (possibly ambiguous) display name.
    pub id: String,
    /// One-line list name.
    pub name: String,
    /// The same list mark and swatch as the drawer.
    pub glyph: String,
    /// The list's swatch key.
    pub hue: String,
    /// The task is already here.
    pub current: bool,
}

/// A non-empty group of matching destinations, in sidebar order.
#[derive(Debug, uniffi::Record)]
pub struct DestinationGroup {
    /// Collection name, or “Other lists” when there are also collections.
    pub heading: Option<String>,
    /// Matching lists.
    pub lists: Vec<Destination>,
}

/// What confirming a list's deletion did.
#[derive(Debug, uniffi::Enum)]
pub enum ListDeletion {
    /// Deleted; the handle undoes exactly this deletion.
    Deleted {
        /// For the toast's Undo.
        undo: Arc<Deletion>,
    },
    /// Nothing was deleted: the question reads differently now (`None`: the list is gone).
    Changed {
        /// The question to ask again.
        question: Option<String>,
    },
}

/// What an undo in the outbox reverses, so a refusal can say so in plain words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Undone {
    Change(&'static str, &'static str),
    TaskDelete,
    TaskMove,
    Completion,
    Step,
    List,
    Reminder,
}

impl Undone {
    fn of(entity: EntityId) -> Self {
        match entity {
            EntityId::Task(_) => Self::TaskDelete,
            EntityId::Step(_) => Self::Step,
            EntityId::List(_) => Self::List,
            _ => Self::Completion,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Self::Change(what, _) => what,
            Self::TaskDelete => "Undo deleting a task",
            Self::TaskMove => "Undo moving a task",
            Self::Completion => "Undo a completion",
            Self::Step => "Undo deleting a step",
            Self::List => "Undo deleting a list",
            Self::Reminder => "Undo a reminder change",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Change(_, noun) => noun,
            Self::TaskDelete | Self::TaskMove | Self::Completion => "task",
            Self::Step => "step",
            Self::List => "list",
            Self::Reminder => "reminder",
        }
    }
}

/// The quick-add date label, if the shared submit parser found a trailing date.
#[uniffi::export]
pub fn quick_add_date(text: String, now: LocalNow) -> Option<String> {
    let now = Now::new(now);
    intent::quick_add::parse(&text, &now.eval())
        .due
        .map(|d| intent::quick_add::short_date(d, now.today()))
}

/// The sync engine and view model.
#[derive(uniffi::Object)]
pub struct TodoEngine {
    client: Client,
    recovered: Option<CacheRecovery>,
    /// Built by [`TodoEngine::fixture`]: only such an engine takes the test hooks
    /// ([`TodoEngine::fixture_delete_task`], [`TodoEngine::fixture_restore_task`]).
    fixture: bool,
    /// What each undo this engine queued reverses (forgotten on restart: a refusal is then worded
    /// generally).
    undos: Mutex<HashMap<CommandId, Undone>>,
    /// Pending batch protection survives replacing the Activity room's pager.
    activity_undos: Mutex<todo_client::activity::Undos>,
    /// Stop invalidates transfers, including ones still staging a picked document on Android.
    transfer_epoch: tokio::sync::watch::Sender<u64>,
    run: Mutex<Option<SyncRun>>,
    // On rapid stop/start, the aborted run may still be handling a frame. Let it finish before
    // resetting the connection status for the new run.
    run_gate: Arc<tokio::sync::Mutex<()>>,
    watch: Mutex<Option<JoinHandle<()>>>,
}

struct SyncRun {
    task: JoinHandle<()>,
    started: Arc<AtomicBool>,
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

fn step_id(id: &str) -> Result<StepId, FfiError> {
    id.parse().map_err(|_| invalid("no such step"))
}

fn list_id(id: &str) -> Result<ListId, FfiError> {
    id.parse().map_err(|_| invalid("no such list"))
}

fn destinations(groups: Vec<intent::organize::TaskMoveGroup<'_>>) -> Vec<DestinationGroup> {
    groups
        .into_iter()
        .map(|group| DestinationGroup {
            heading: group.heading,
            lists: group
                .lists
                .into_iter()
                .map(|dest| {
                    let list = dest.list;
                    Destination {
                        id: list.id.to_string(),
                        name: crate::labels::one_line(&list.name),
                        glyph: crate::lists::list_glyph(&list.name).into(),
                        hue: crate::lists::swatch(
                            list.color.as_deref(),
                            list.id.as_uuid().as_bytes(),
                        )
                        .into(),
                        current: dest.current,
                    }
                })
                .collect(),
        })
        .collect()
}

fn collection_id(id: &str) -> Result<CollectionId, FfiError> {
    id.parse().map_err(|_| invalid("no such collection"))
}

/// The reminder editor's validation note for a time that has already passed.
#[uniffi::export]
pub fn reminder_past_note() -> String {
    REMINDER_PAST.into()
}

/// Confirmation text for a reminder time already formatted in the phone's zone.
#[uniffi::export]
pub fn reminder_set_text(time: String) -> String {
    reminder::reminder_set_text(&time)
}

fn seen_reminder(seen: ReminderRef) -> Result<intent::reminder::Seen, FfiError> {
    Ok(intent::reminder::Seen {
        id: seen.id.parse().map_err(|_| invalid(REMINDER_CHANGED))?,
        at: Timestamp::from_millisecond(seen.at_ms).map_err(|_| invalid(REMINDER_CHANGED))?,
    })
}

fn stale(e: intent::reminder::Stale) -> FfiError {
    match e {
        intent::reminder::Stale::Task => invalid("No such task."),
        intent::reminder::Stale::Reminder => invalid(REMINDER_CHANGED),
    }
}

/// What a refused change was, in the "Not saved" banner's words: the shared wording
/// (`todo_client::failure::what`) for a command, and for an undo this engine queued, what it reverses
/// ([`Undone`]).
fn describe(entry: &OutboxEntry, undone: Option<Undone>) -> &'static str {
    match (entry.command(), undone) {
        (None, Some(undone)) => undone.what(),
        _ => todo_client::failure::what(entry),
    }
}

/// Why the server refused a change, in the app's own words: from the rejection's code, never the
/// server's message, which can be technical or quote task content.
fn reason(failed: &todo_client::FailedEntry, undone: Option<Undone>) -> String {
    if failed.entry.command().is_some() {
        todo_client::failure::why(failed).to_owned()
    } else {
        undo_failure_message(&failed.rejection.code, undone)
    }
}

/// Why a restored item still shows nothing: Undo brings back exactly what was deleted, never
/// what it belongs to, so if that (its task, list or collection) was deleted meanwhile, elsewhere, the
/// restored item stays hidden with it. `None` when it shows, or while the view doesn't predict the undo.
fn hidden_by_parent(state: &State, entity: EntityId) -> Option<String> {
    if state.tombstone(entity).is_some() || state.is_live(entity) {
        return None;
    }
    let mut at = state.entity(entity)?.parent();
    let gone = loop {
        let parent = state.entity(at?)?;
        if parent.deleted().is_some() {
            break parent;
        }
        at = parent.parent();
    };
    let what = match gone {
        Entity::Task(_) => "task",
        Entity::List(_) => "list",
        Entity::Collection(_) => "collection",
        _ => return None,
    };
    let subject = match entity {
        EntityId::Step(_) => "Step",
        EntityId::List(_) => "List",
        _ => "Task",
    };
    Some(format!(
        "{subject} restored, but its {what} was deleted elsewhere."
    ))
}

/// The question before deleting a list: which list, and how many tasks go with it, worded as the
/// desktop's. `None` if the list is gone.
pub fn delete_list_question(state: &State, id: ListId) -> Option<String> {
    let list = state
        .lists
        .get(&id)
        .filter(|_| view::list_live(state, id))?;
    let name = crate::labels::one_line(&list.name);
    let tasks = state
        .tasks
        .values()
        .filter(|t| t.list == id && t.deleted.is_none() && t.merged_into.is_none())
        .count();
    let what = match tasks {
        0 => format!("Delete the list \u{201c}{name}\u{201d}?"),
        1 => format!("Delete the list \u{201c}{name}\u{201d} and its task?"),
        n => format!("Delete the list \u{201c}{name}\u{201d} and its {n} tasks?"),
    };
    let back = if tasks == 0 { "it" } else { "them" };
    Some(format!("{what} Undo brings {back} back."))
}

impl TodoEngine {
    fn new(client: Client, recovered: Option<CacheRecovery>, fixture: bool) -> Arc<Self> {
        Arc::new(Self {
            client,
            recovered,
            fixture,
            undos: Mutex::new(HashMap::new()),
            activity_undos: Mutex::default(),
            transfer_epoch: tokio::sync::watch::channel(0).0,
            run: Mutex::new(None),
            run_gate: Arc::default(),
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

    /// Cancels an unsent tracked command or queues its checked inverse, remembering what it reverses.
    fn undo_tracked(&self, target: &TrackedCommand, undone: Undone) -> Result<(), FfiError> {
        match self.client.undo_tracked(target) {
            Ok(TrackedUndo::Cancelled) => Ok(()),
            Ok(TrackedUndo::Queued(id)) => {
                locked(&self.undos).insert(id, undone);
                Ok(())
            }
            Ok(TrackedUndo::Unavailable) => Err(invalid(
                "Couldn't undo: the original change couldn't be found.",
            )),
            Err(todo_client::SubmitError::Storage(e)) => Err(e.into()),
            Err(todo_client::SubmitError::Invalid(_)) => Err(invalid("Couldn't undo that.")),
        }
    }

    /// Submits `command` if there is one; `Ok(false)` when there was nothing to do.
    fn submit_some(&self, command: Option<Command>) -> Result<bool, FfiError> {
        match command {
            Some(command) => {
                self.client.submit(command)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn track_move(&self, command: Option<Command>) -> Result<Option<Arc<TaskMove>>, FfiError> {
        Ok(command
            .map(|cmd| {
                self.client
                    .submit_tracked(cmd)
                    .map(|t| Arc::new(TaskMove(t)))
            })
            .transpose()?)
    }

    fn track_reminder(&self, cmd: Command) -> Result<Arc<ReminderChange>, FfiError> {
        Ok(Arc::new(ReminderChange(self.client.submit_tracked(cmd)?)))
    }

    /// Deletes a live step or list, tracked for its Undo.
    fn delete_tracked(&self, entity: EntityId) -> Result<Arc<Deletion>, FfiError> {
        let tracked = self.client.submit_tracked(intent::delete(entity))?;
        Ok(Arc::new(Deletion { tracked, entity }))
    }

    /// A live task, or why not.
    fn live_task(&self, id: &str) -> Result<(TaskId, Arc<State>), FfiError> {
        let id = task_id(id)?;
        let state = self.client.view();
        if !state
            .tasks
            .get(&id)
            .is_some_and(|t| view::task_live(&state, t))
        {
            return Err(invalid("No such task."));
        }
        Ok((id, state))
    }

    /// A live step, or why not.
    fn live_step(&self, id: &str) -> Result<(StepId, Arc<State>), FfiError> {
        let id = step_id(id)?;
        let state = self.client.view();
        if !state.is_live(id.into()) {
            return Err(invalid("No such step."));
        }
        Ok((id, state))
    }

    /// A live list, or why not.
    fn live_list(&self, id: &str) -> Result<(ListId, Arc<State>), FfiError> {
        let id = list_id(id)?;
        let state = self.client.view();
        if !view::list_live(&state, id) {
            return Err(invalid("No such list."));
        }
        Ok((id, state))
    }
}

fn intent_room(room: RoomKey) -> TaskRoom {
    match room {
        RoomKey::List { id } => id.parse().map_or(TaskRoom::Other, TaskRoom::List),
        RoomKey::Collection { id } => id.parse().map_or(TaskRoom::Other, TaskRoom::Collection),
        RoomKey::Trash => TaskRoom::Other,
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
        let files = recovery::Files::new(Path::new(&config.database_path));
        let transport = HttpTransport::with_cleartext(&server, Some(&config.token), cleartext)
            .map_err(|e| FfiError::BadAddress {
                reason: e.to_string(),
            })?;
        let revocation = Revocation::for_cache(&files.cache).map_err(|e| FfiError::Storage {
            detail: format!("reading pairing revocation: {e}"),
        })?;
        let transport = transport.with_revocation(revocation.clone());
        let storage_error = |e: recovery::RecoveryError| FfiError::Storage {
            detail: e.to_string(),
        };
        // Finish a recovery an earlier open started before anything opens the cache, so an empty cache
        // is never opened while unsent changes sit in the quarantined copy.
        let mut recovered = recovery::resume(&files).map_err(storage_error)?;
        let open = || -> Result<Client, StorageError> {
            let storage = SqliteStorage::open(&files.cache)?;
            SyncClient::with_revocation(
                AppTransport::Http(transport.clone()),
                TokioClock,
                OsEntropy,
                storage,
                ClientConfig::new(actor.clone()),
                revocation.clone(),
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

    /// An offline engine on invented data dated relative to `now`, for
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
        if run.as_ref().is_some_and(|r| !r.task.is_finished()) {
            return Ok(());
        }
        let client = self.client.clone();
        let gate = self.run_gate.clone();
        let started = Arc::new(AtomicBool::new(false));
        let mark_started = started.clone();
        let task = runtime()?.spawn(async move {
            let _run = gate.lock().await;
            client
                .run_on_start(|| mark_started.store(true, Ordering::Release))
                .await;
        });
        *run = Some(SyncRun { task, started });
        drop(run);
        self.client.nudge();
        Ok(())
    }

    /// Stops syncing (the app went to the background). In-flight requests are abandoned; the server is
    /// idempotent by command ID, so a resend after [`start`](Self::start) is safe. The outbox stays.
    /// [`status`](Self::status) reports `Paused` from now on; the listener isn't called for that.
    /// Direct attachment transfers are cancelled separately by the caller.
    pub fn stop(&self) {
        if let Some(run) = locked(&self.run).take() {
            run.task.abort();
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
        let run = locked(&self.run);
        let running = run.as_ref().filter(|r| !r.task.is_finished());
        let started = running.is_some_and(|r| r.started.load(Ordering::Acquire));
        let s = self.client.status();
        let state = if running.is_none() {
            SyncState::Paused
        } else if !started {
            SyncState::Syncing
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
            caught_up: started && s.caught_up,
            revoked: s.revoked,
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

    /// Completes or reopens a task. Only an actual completion gets an Undo action.
    pub fn set_done(&self, id: String, done: bool) -> Result<Option<Arc<Completion>>, FfiError> {
        let id = task_id(&id)?;
        let Some(command) = intent::set_done(&self.client.view(), id, done) else {
            return Ok(None);
        };
        if done {
            return Ok(Some(Arc::new(Completion(
                self.client.submit_tracked(command)?,
            ))));
        }
        self.client.submit(command)?;
        Ok(None)
    }

    /// Cancels an unsent completion or queues its checked inverse. A later server refusal is
    /// reported by `failed`, like other outbox failures, but never with raw server text.
    pub fn undo_completion(&self, completion: Arc<Completion>) -> Result<(), FfiError> {
        self.undo_tracked(&completion.0, Undone::Completion)
    }

    /// Sets the due date (ISO, `2026-11-22`) or clears it (`None`). Clearing it also clears a repeat
    /// rule, which needs a due date (`intent::set_due`).
    pub fn set_due(&self, id: String, due: Option<String>) -> Result<(), FfiError> {
        let due = due
            .map(|d| d.trim().parse::<Date>())
            .transpose()
            .map_err(|_| invalid("That isn't a date."))?;
        let (id, state) = self.live_task(&id)?;
        self.submit_some(intent::set_due(&state, id, due))?;
        Ok(())
    }

    /// Sets the priority by its `!` marks: 0 (none), 1 (low), 2 (normal) or 3 (high).
    pub fn set_priority(&self, id: String, marks: u8) -> Result<(), FfiError> {
        let priority = view::priority_of(marks).ok_or_else(|| invalid("No such priority."))?;
        let (id, state) = self.live_task(&id)?;
        self.submit_some(intent::pick_priority(&state, id, priority))?;
        Ok(())
    }

    /// Sets the reminder the owner saw (`seen`, from [`TaskDetail::reminder`]) to `at_ms`, milliseconds
    /// since the Unix epoch, or adds one to a task that had none (`None`; `intent::reminder::set_reminder`).
    /// `None` when it is already then. A time that has passed is refused, and so is an edit on a reminder
    /// that changed or went while the picker was open: nothing is written, and the error says so. The
    /// handle undoes exactly this change.
    pub fn set_reminder(
        &self,
        id: String,
        seen: Option<ReminderRef>,
        at_ms: i64,
        now: LocalNow,
    ) -> Result<Option<Arc<ReminderChange>>, FfiError> {
        let at = Timestamp::from_millisecond(at_ms).map_err(|_| invalid("That isn't a time."))?;
        let (id, state) = self.live_task(&id)?;
        let seen = seen.map(seen_reminder).transpose()?;
        let now = Now::new(now);
        let Some(cmd) =
            intent::reminder::set_reminder(&state, id, seen, at, || self.client.new_uuid())
                .map_err(stale)?
        else {
            return Ok(None);
        };
        if at <= now.at {
            return Err(invalid(REMINDER_PAST));
        }
        self.track_reminder(cmd).map(Some)
    }

    /// Removes the reminder the owner saw (`seen`; `intent::reminder::remove_reminder`); others stay. One
    /// that changed or went meanwhile is left alone, and the error says so. The handle undoes exactly
    /// this removal.
    pub fn remove_reminder(
        &self,
        id: String,
        seen: ReminderRef,
    ) -> Result<Arc<ReminderChange>, FfiError> {
        let (id, state) = self.live_task(&id)?;
        let cmd =
            intent::reminder::remove_reminder(&state, id, seen_reminder(seen)?).map_err(stale)?;
        self.track_reminder(cmd)
    }

    /// Reverses this exact reminder change. A later conflicting change is not overwritten.
    pub fn undo_reminder(&self, change: Arc<ReminderChange>) -> Result<(), FfiError> {
        self.undo_tracked(&change.0, Undone::Reminder)
    }

    /// The reminders to alert for, from `since_ms` on (milliseconds since the Unix epoch), earliest
    /// first: live reminders of open tasks (`intent::reminder::alarms`). One that's missing has been
    /// removed, or its task completed or deleted, here or elsewhere.
    pub fn reminder_alarms(&self, since_ms: i64, now: LocalNow) -> Vec<ReminderAlarm> {
        let state = self.client.view();
        let since = Timestamp::from_millisecond(since_ms).unwrap_or(Timestamp::UNIX_EPOCH);
        let now = Now::new(now);
        intent::reminder::alarms(&state, since)
            .into_iter()
            .filter_map(|a| {
                let task = state.tasks.get(&a.task)?;
                Some(ReminderAlarm {
                    id: a.reminder.to_string(),
                    task: a.task.to_string(),
                    title: crate::labels::one_line(&task.title),
                    at_ms: a.at.as_millisecond(),
                    label: crate::labels::when(a.at, &now),
                })
            })
            .collect()
    }

    /// Deletes a task immediately. The toast holds the exact deletion for Undo.
    pub fn delete_task(&self, id: String) -> Result<Arc<Deletion>, FfiError> {
        let id = task_id(&id)?;
        let cmd =
            intent::delete_task(&self.client.view(), id).ok_or_else(|| invalid("No such task."))?;
        Ok(Arc::new(Deletion {
            tracked: self.client.submit_tracked(cmd)?,
            entity: id.into(),
        }))
    }

    /// The Move to… picker: every typed word must match the list or its collection, ignoring
    /// case and accents (the same fold as the desktop). Empty groups never get a heading.
    pub fn move_destinations(&self, id: String, filter: String) -> Vec<DestinationGroup> {
        let state = self.client.view();
        id.parse()
            .ok()
            .and_then(|id| intent::organize::task_move_groups(&state, id, &filter))
            .map_or_else(Vec::new, destinations)
    }

    /// Moves to the front of another list. Queues nothing if the picker is stale or the list is the same.
    pub fn move_task_to(
        &self,
        id: String,
        from: String,
        list: String,
    ) -> Result<Option<Arc<TaskMove>>, FfiError> {
        self.track_move(intent::organize::move_task(
            &self.client.view(),
            task_id(&id)?,
            list_id(&from)?,
            list_id(&list)?,
        ))
    }

    /// Moves past the adjacent visible open task in this manual list, not in a smart room.
    pub fn reorder_task(
        &self,
        list: String,
        id: String,
        neighbour: String,
        by: i32,
    ) -> Result<Option<Arc<TaskMove>>, FfiError> {
        self.track_move(intent::reorder_open_task(
            &self.client.view(),
            list_id(&list)?,
            task_id(&id)?,
            task_id(&neighbour)?,
            by,
        ))
    }

    /// Reverses this exact move. A later conflicting change is not overwritten.
    pub fn undo_task_move(&self, moved: Arc<TaskMove>) -> Result<(), FfiError> {
        self.undo_tracked(&moved.0, Undone::TaskMove)
    }

    /// Renames a step.
    pub fn rename_step(&self, id: String, title: String) -> Result<(), FfiError> {
        if title.trim().is_empty() {
            return Err(invalid("A step needs a title."));
        }
        let (id, state) = self.live_step(&id)?;
        self.submit_some(intent::retitle_step(&state, id, &title))?;
        Ok(())
    }

    /// Moves a step one place up (`by` < 0) or down among its task's steps. `false` at either end.
    pub fn move_step(&self, id: String, by: i32) -> Result<bool, FfiError> {
        let (id, state) = self.live_step(&id)?;
        self.submit_some(intent::nudge(&state, id.into(), by))
    }

    /// Deletes a step. The returned handle undoes exactly this deletion.
    pub fn delete_step(&self, id: String) -> Result<Arc<Deletion>, FfiError> {
        let (id, _) = self.live_step(&id)?;
        self.delete_tracked(id.into())
    }

    /// Renames a list.
    pub fn rename_list(&self, id: String, name: String) -> Result<(), FfiError> {
        if name.trim().is_empty() {
            return Err(invalid("A list needs a name."));
        }
        let (id, state) = self.live_list(&id)?;
        self.submit_some(intent::rename_list(&state, id, &name))?;
        Ok(())
    }

    /// Renames a collection.
    pub fn rename_collection(&self, id: String, name: String) -> Result<(), FfiError> {
        if name.trim().is_empty() {
            return Err(invalid("A collection needs a name."));
        }
        let id = collection_id(&id)?;
        let state = self.client.view();
        if !state.is_live(id.into()) {
            return Err(invalid("No such collection."));
        }
        self.submit_some(intent::rename_collection(&state, id, &name))?;
        Ok(())
    }

    /// Moves a list (among its collection's lists, or the top-level ones) or a collection one place up
    /// (`by` < 0) or down. `false` at either end, and for a smart list.
    pub fn move_room(&self, room: RoomKey, by: i32) -> Result<bool, FfiError> {
        let entity: EntityId = match room {
            RoomKey::List { id } => self.live_list(&id)?.0.into(),
            RoomKey::Collection { id } => collection_id(&id)?.into(),
            RoomKey::Smart { .. } | RoomKey::Trash => return Ok(false),
        };
        let state = self.client.view();
        if !state.is_live(entity) {
            return Err(invalid("No such collection."));
        }
        self.submit_some(intent::nudge(&state, entity, by))
    }

    /// What deleting a list would take with it, as the confirmation asks it; `None` if it is gone.
    pub fn delete_list_question(&self, id: String) -> Option<String> {
        delete_list_question(&self.client.view(), id.parse().ok()?)
    }

    /// Deletes a list and its tasks, if the owner saw what goes with it: `asked` is the question they
    /// confirmed ([`delete_list_question`](Self::delete_list_question)). When it reads differently now
    /// (the list was renamed, or tasks synced into it or out of it meanwhile), nothing is deleted and the
    /// new question comes back, to ask again; `None` in it when the list is gone.
    pub fn delete_list(&self, id: String, asked: String) -> Result<ListDeletion, FfiError> {
        let list = list_id(&id)?;
        let now = delete_list_question(&self.client.view(), list);
        if now.as_deref() != Some(asked.as_str()) {
            return Ok(ListDeletion::Changed { question: now });
        }
        let (list, _) = self.live_list(&id)?;
        Ok(ListDeletion::Deleted {
            undo: self.delete_tracked(list.into())?,
        })
    }

    /// Undoes a deletion: cancels it if unsent, else queues its checked inverse (a refusal shows in
    /// [`failed`](Self::failed), in plain words). Returns a notice when the restored item still can't
    /// show, because what it belongs to was deleted elsewhere meanwhile.
    pub fn undo_deletion(&self, deletion: Arc<Deletion>) -> Result<Option<String>, FfiError> {
        self.undo_tracked(&deletion.tracked, Undone::of(deletion.entity))?;
        Ok(hidden_by_parent(&self.client.view(), deletion.entity))
    }

    /// Edits multi-line plain-text notes; empty text clears them.
    pub fn set_notes(&self, id: String, notes: String) -> Result<(), FfiError> {
        let id = task_id(&id)?;
        let state = self.client.view();
        if !state
            .tasks
            .get(&id)
            .is_some_and(|t| view::task_live(&state, t))
        {
            return Err(invalid("No such task."));
        }
        if let Some(command) = intent::set_notes(&state, id, &notes) {
            self.client.submit(command)?;
        }
        Ok(())
    }

    /// A fixture-only out-of-band deletion, as from another device. A tombstone:
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
        let undos = locked(&self.undos).clone();
        self.client
            .failed()
            .into_iter()
            .map(|f| {
                let undone = undos.get(&f.entry.command_id).copied();
                FailedChange {
                    id: f.entry.command_id.to_string(),
                    what: describe(&f.entry, undone).into(),
                    reason: reason(&f, undone),
                }
            })
            .collect()
    }

    /// Removes a refused change from the list.
    pub fn dismiss_failed(&self, id: String) -> Result<bool, FfiError> {
        let id = id.parse().map_err(|_| invalid("no such change"))?;
        Ok(self.client.dismiss(id)?)
    }
}

/// A refused undo in plain words, never the server's text. `undone` names what it reversed, when known.
fn undo_failure_message(code: &str, undone: Option<Undone>) -> String {
    let subject = undone.map_or("it".to_owned(), |u| format!("the {}", u.noun()));
    match code {
        "undo_conflict" => format!("Couldn't undo: {subject} was changed again."),
        "not_found" => format!("Couldn't undo: {subject} no longer exists."),
        _ => todo_client::failure::undo_message(code).into(),
    }
}

impl Drop for TodoEngine {
    fn drop(&mut self) {
        self.stop();
        if let Some(task) = locked(&self.watch).take() {
            task.abort();
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
    fn organizing_queues_only_the_shared_builders_commands() {
        use organize::{ColorTarget, RepeatChoice};
        let now = LocalNow {
            epoch_ms: 0,
            utc_offset_secs: 0,
            zone_id: "UTC".into(),
            local_day: None,
        };
        let e = TodoEngine::fixture("world".into(), now.clone()).unwrap();
        let before = e.state();
        let task = *before
            .tasks
            .keys()
            .find(|id| before.is_live((**id).into()))
            .unwrap();
        let last = || {
            e.client
                .pending()
                .last()
                .and_then(|p| p.command().cloned())
                .unwrap()
        };
        let repeat = intent::repeat::Preset::Daily.rule(Now::new(now.clone()).today());
        e.set_repeat(
            task.to_string(),
            RepeatChoice::Preset {
                preset: intent::repeat::Preset::Daily,
            },
            now.clone(),
        )
        .unwrap();
        assert_eq!(
            Some(last()),
            intent::repeat::set_repeat(&before, task, Some(repeat), Now::new(now).today()).unwrap()
        );
        let before = e.state();
        let RoomKey::Collection { id } = e.create_collection(" Garden ".into()).unwrap() else {
            panic!("collection")
        };
        let collection = id.parse().unwrap();
        assert_eq!(last(), intent::create_collection(collection, " Garden "));
        let list = before.tasks[&task].list;
        let before = e.state();
        let moved = e
            .move_list(list.to_string(), Some(id.clone()))
            .unwrap()
            .unwrap();
        let name = crate::labels::one_line(&before.lists[&list].name);
        assert_eq!(moved.toast_text(), format!("Moved “{name}” to Garden"));
        assert_eq!(
            Some(last()),
            intent::move_list(&before, list, Some(collection))
        );
        let moved = e.move_list(list.to_string(), None).unwrap().unwrap();
        assert_eq!(moved.toast_text(), format!("Moved “{name}” out of Garden"));
        let target = ColorTarget::Collection { id };
        let before = e.state();
        let color = if before.collections[&collection].color.as_deref() == Some("fern") {
            "jade"
        } else {
            "fern"
        };
        e.set_swatch(target.clone(), color.into()).unwrap();
        assert_eq!(
            Some(last()),
            intent::set_color(&before, collection.into(), color)
        );
        let before = e.state();
        e.tag_task_named(task.to_string(), "garden".into()).unwrap();
        assert!(e.tag_named(" GARDEN ".into()));
        let tag = intent::tag_named(&e.state(), "garden").unwrap();
        let (_, expected) =
            intent::tag_task_named(&before, task, "garden", || tag.as_uuid()).unwrap();
        let pending = e.client.pending();
        assert_eq!(
            pending
                .iter()
                .rev()
                .take(2)
                .filter_map(OutboxEntry::command)
                .collect::<Vec<_>>(),
            expected.iter().rev().collect::<Vec<_>>()
        );
        let before = e.state();
        e.toggle_tag(task.to_string(), tag.to_string()).unwrap();
        assert_eq!(Some(last()), intent::toggle_tag(&before, task, tag));
        let before = e.state();
        e.rename_tag(tag.to_string(), "outside".into()).unwrap();
        assert_eq!(
            Some(last()),
            intent::rename_tag(&before, tag, "outside").unwrap()
        );
        let before = e.state();
        e.add_web_link(task.to_string(), "todo.example.com".into())
            .unwrap();
        let Command::AddLink { id: link, .. } = last() else {
            panic!("link")
        };
        assert_eq!(
            last(),
            intent::add_link(&before, task, "todo.example.com", || link.as_uuid()).unwrap()
        );
        let before = e.state();
        e.remove_link(link.to_string()).unwrap();
        assert_eq!(Some(last()), intent::delete_link(&before, link));
        let asked = e.organize_question(target.clone()).unwrap();
        let before = e.state();
        e.confirm_organize_delete(target, asked).unwrap();
        assert_eq!(Some(last()), intent::delete_collection(&before, collection));
        let target = ColorTarget::Tag {
            id: tag.to_string(),
        };
        let asked = e.organize_question(target.clone()).unwrap();
        e.confirm_organize_delete(target, asked).unwrap();
        assert_eq!(last(), intent::delete(tag));
    }

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
            zone_id: "UTC".into(),
            local_day: None,
        };
        let engine = TodoEngine::fixture("blank".into(), now.clone()).expect("fixture");
        let before = engine.state();
        let text = "Pay the invoice by fri !!";
        let id: TaskId = engine
            .add_task(RoomKey::default(), text.into(), now.clone())
            .expect("task added")
            .parse()
            .expect("task ID");
        let list = engine.state().tasks[&id].list;
        let mut ids = [list.as_uuid(), id.as_uuid()].into_iter();
        let expected = intent::quick_add_task(
            &before,
            TaskRoom::MyDay,
            text,
            &Now::new(now.clone()).eval(),
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

    fn fixture_now() -> LocalNow {
        let at: Timestamp = "2026-11-21T10:35:00Z".parse().expect("timestamp");
        LocalNow {
            epoch_ms: at.as_millisecond(),
            utc_offset_secs: 0,
            zone_id: "UTC".into(),
            local_day: None,
        }
    }

    /// Why an engine call was refused, or `None` when it went through.
    fn refusal<T>(r: Result<T, FfiError>) -> Option<String> {
        match r {
            Err(FfiError::Invalid { detail }) => Some(detail),
            Err(e) => Some(format!("{e}")),
            Ok(_) => None,
        }
    }

    /// The commands the engine queued since `from`, oldest first.
    fn queued(engine: &TodoEngine, from: usize) -> Vec<Command> {
        engine
            .client
            .pending()
            .iter()
            .skip(from)
            .filter_map(OutboxEntry::command)
            .cloned()
            .collect()
    }

    #[test]
    fn ffi_edits_submit_the_shared_builders() {
        use todo_core::model::Priority;
        let engine = TodoEngine::fixture("world".into(), fixture_now()).expect("fixture");
        let id = engine
            .add_task(RoomKey::default(), "Sow seeds".into(), fixture_now())
            .expect("task");
        let task: TaskId = id.parse().expect("task ID");
        let step = engine
            .add_step(id.clone(), "Buy soil".into())
            .expect("step");
        let step_id: StepId = step.parse().expect("step ID");
        engine.add_step(id.clone(), "Dig".into()).expect("step");
        let at = engine.client.pending().len();

        let state = engine.state();
        let due: Date = "2026-12-01".parse().expect("date");
        engine
            .set_due(id.clone(), Some(due.to_string()))
            .expect("due");
        engine.set_priority(id.clone(), 2).expect("priority");
        engine
            .rename_step(step.clone(), "Buy compost".into())
            .expect("rename");
        let mut expected = vec![
            intent::set_due(&state, task, Some(due)).expect("due"),
            intent::pick_priority(&state, task, Priority::Normal).expect("priority"),
            intent::retitle_step(&state, step_id, "Buy compost").expect("rename"),
        ];
        let state = engine.state();
        engine.move_step(step.clone(), 1).expect("move");
        expected.push(intent::nudge(&state, step_id.into(), 1).expect("move"));
        engine.delete_step(step).expect("delete");
        expected.push(intent::delete(step_id));
        assert_eq!(queued(&engine, at), expected);

        // Lists and collections, through the same builders.
        let state = engine.state();
        let &collection = state.collections.keys().next().expect("collection");
        let list = *state
            .lists
            .values()
            .find(|l| l.collection == Some(collection))
            .map(|l| &l.id)
            .expect("list");
        let at = engine.client.pending().len();
        engine
            .rename_list(list.to_string(), "Seeds".into())
            .expect("rename");
        engine
            .rename_collection(collection.to_string(), "Garden".into())
            .expect("rename");
        let mut expected = vec![
            intent::rename_list(&state, list, "Seeds").expect("rename"),
            intent::rename_collection(&state, collection, "Garden").expect("rename"),
        ];
        let state = engine.state();
        let by = if intent::nudge(&state, list.into(), 1).is_some() {
            1
        } else {
            -1
        };
        assert!(
            engine
                .move_room(
                    RoomKey::List {
                        id: list.to_string()
                    },
                    by
                )
                .expect("move")
        );
        expected.push(intent::nudge(&state, list.into(), by).expect("move"));
        let asked = engine
            .delete_list_question(list.to_string())
            .expect("question");
        assert!(matches!(
            engine.delete_list(list.to_string(), asked).expect("delete"),
            ListDeletion::Deleted { .. }
        ));
        expected.push(intent::delete(list));
        assert_eq!(queued(&engine, at), expected);
    }

    #[test]
    fn task_actions_queue_exactly_the_shared_intents() {
        let e = TodoEngine::fixture("blank".into(), fixture_now()).expect("fixture");
        let source = e
            .create_list("Source".into(), RoomKey::default())
            .expect("list");
        let RoomKey::List { id: list } = source.clone() else {
            panic!("list")
        };
        let RoomKey::List { id: dest } = e
            .create_list("Žluté".into(), RoomKey::default())
            .expect("list")
        else {
            panic!("list")
        };
        let other = e
            .add_task(source.clone(), "First".into(), fixture_now())
            .expect("task");
        let id = e
            .add_task(source, "New".into(), fixture_now())
            .expect("task");
        let task = task_id(&id).expect("id");
        let at = e.client.pending().len();
        let picks = e.move_destinations(id.clone(), "ZLUTE".into());
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].lists.len(), 1);
        assert_eq!(picks[0].lists[0].id, dest);
        assert!(!picks[0].lists[0].current);
        assert!(
            e.move_task_to(id.clone(), list.clone(), list.clone())
                .expect("same list")
                .is_none()
        );
        let cmd = intent::reorder_open_task(
            &e.state(),
            list_id(&list).expect("list"),
            task,
            task_id(&other).expect("other"),
            1,
        )
        .expect("reorder");
        e.reorder_task(list.clone(), id.clone(), other.clone(), 1)
            .expect("reorder")
            .expect("handle");
        assert!(
            e.reorder_task(list.clone(), id.clone(), other, 1)
                .expect("already moved")
                .is_none()
        );
        let moved =
            intent::move_to_list(&e.state(), task, list_id(&dest).expect("list")).expect("move");
        e.move_task_to(id.clone(), list.clone(), dest)
            .expect("move")
            .expect("handle");
        assert!(
            e.move_task_to(id.clone(), list.clone(), list)
                .expect("stale move")
                .is_none()
        );
        let deleted = intent::delete_task(&e.state(), task).expect("delete");
        e.delete_task(id).expect("delete");
        assert_eq!(queued(&e, at), [cmd, moved, deleted]);
    }

    #[test]
    fn reminders_queue_exactly_the_shared_builders_and_undo_them() {
        use intent::reminder::Seen;
        let now = fixture_now;
        let e = TodoEngine::fixture("world".into(), now()).expect("fixture");
        let id = e
            .add_task(RoomKey::default(), "Call the dentist".into(), now())
            .expect("task");
        let task = task_id(&id).expect("id");
        let at: Timestamp = "2026-11-21T14:00:00Z".parse().expect("timestamp");
        let later: Timestamp = "2026-11-22T08:00:00Z".parse().expect("timestamp");
        let seen = |e: &TodoEngine| e.task(id.clone(), now()).expect("task").reminder;
        let from = e.client.pending().len();

        let state = e.state();
        e.set_reminder(id.clone(), None, at.as_millisecond(), now())
            .expect("set")
            .expect("handle");
        let Some(Command::AddReminder { id: reminder, .. }) = queued(&e, from).first().cloned()
        else {
            panic!("expected an added reminder");
        };
        let mut expected = vec![
            intent::reminder::set_reminder(&state, task, None, at, || reminder.as_uuid())
                .expect("unchanged")
                .expect("add"),
        ];
        let shown = seen(&e).expect("shown");
        assert_eq!(
            shown,
            ReminderRef {
                id: reminder.to_string(),
                at_ms: at.as_millisecond()
            }
        );
        // The same time again queues nothing.
        assert!(
            e.set_reminder(id.clone(), Some(shown.clone()), at.as_millisecond(), now())
                .expect("same")
                .is_none()
        );
        let state = e.state();
        let moved = e
            .set_reminder(id.clone(), Some(shown), later.as_millisecond(), now())
            .expect("move")
            .expect("handle");
        let was = Seen { id: reminder, at };
        expected.push(
            intent::reminder::set_reminder(&state, task, Some(was), later, Uuid::nil)
                .expect("unchanged")
                .expect("move"),
        );
        let state = e.state();
        let shown = seen(&e).expect("shown");
        let removed = e
            .remove_reminder(id.clone(), shown.clone())
            .expect("remove");
        let was = Seen {
            id: reminder,
            at: later,
        };
        expected.push(intent::reminder::remove_reminder(&state, task, was).expect("remove"));
        // Removing it again: it has gone, so nothing is written and the owner is told.
        assert_eq!(
            refusal(e.remove_reminder(id.clone(), shown)),
            Some(REMINDER_CHANGED.into())
        );
        assert_eq!(queued(&e, from), expected);
        let detail = e.task(id.clone(), now()).expect("task");
        assert_eq!(detail.reminder, None);
        let row = detail
            .fields
            .iter()
            .find(|f| f.label == "Reminder")
            .expect("the row is always there");
        assert_eq!(
            (row.edit, row.empty),
            (Some(view::FieldEdit::Reminder), true)
        );

        // Undo brings back exactly what each change replaced.
        e.undo_reminder(removed).expect("undo");
        assert_eq!(seen(&e).map(|r| r.at_ms), Some(later.as_millisecond()));
        e.undo_reminder(moved).expect("undo");
        assert_eq!(seen(&e).map(|r| r.at_ms), Some(at.as_millisecond()));

        // A time that has passed is refused, and queues nothing.
        let pending = e.client.pending().len();
        let past = Timestamp::from_millisecond(now().epoch_ms - 60_000).expect("past");
        assert!(matches!(
            e.set_reminder(id.clone(), seen(&e), past.as_millisecond(), now()),
            Err(FfiError::Invalid { detail }) if detail == reminder_past_note()
        ));
        assert_eq!(e.client.pending().len(), pending);
    }

    #[test]
    fn a_picker_left_open_never_edits_a_reminder_that_changed_meanwhile() {
        let now = fixture_now;
        let e = TodoEngine::fixture("world".into(), now()).expect("fixture");
        let id = e
            .add_task(RoomKey::default(), "Call the dentist".into(), now())
            .expect("task");
        let at: Timestamp = "2026-11-21T14:00:00Z".parse().expect("timestamp");
        let later: Timestamp = "2026-11-22T08:00:00Z".parse().expect("timestamp");
        e.set_reminder(id.clone(), None, at.as_millisecond(), now())
            .expect("set");
        let opened = e
            .task(id.clone(), now())
            .expect("task")
            .reminder
            .expect("shown");
        // Moved elsewhere while the picker was open.
        let current = e.task(id.clone(), now()).expect("task").reminder;
        e.set_reminder(id.clone(), current, later.as_millisecond(), now())
            .expect("moved elsewhere");
        let pending = e.client.pending().len();
        let stale = Some(REMINDER_CHANGED.to_owned());
        let tomorrow = at.as_millisecond() + 86_400_000;
        assert_eq!(
            refusal(e.set_reminder(id.clone(), Some(opened.clone()), tomorrow, now())),
            stale
        );
        assert_eq!(
            refusal(e.remove_reminder(id.clone(), opened.clone())),
            stale
        );
        // Opened on a task without a reminder, which got one elsewhere: no second one is added.
        let bare = e
            .add_task(RoomKey::default(), "Water the plants".into(), now())
            .expect("task");
        e.set_reminder(bare.clone(), None, at.as_millisecond(), now())
            .expect("added elsewhere");
        let pending_bare = e.client.pending().len();
        assert_eq!(refusal(e.set_reminder(bare, None, tomorrow, now())), stale);
        assert_eq!(e.client.pending().len(), pending_bare);
        assert_eq!(pending + 2, pending_bare);
        assert_eq!(
            e.task(id, now()).expect("task").reminder.map(|r| r.at_ms),
            Some(later.as_millisecond())
        );
    }

    #[test]
    fn alarms_follow_the_reminders_of_open_tasks() {
        let now = fixture_now;
        let e = TodoEngine::fixture("blank".into(), now()).expect("fixture");
        let id = e
            .add_task(RoomKey::default(), "Call the dentist".into(), now())
            .expect("task");
        let at: Timestamp = "2026-11-21T14:00:00Z".parse().expect("timestamp");
        e.set_reminder(id.clone(), None, at.as_millisecond(), now())
            .expect("set");
        let alarms = e.reminder_alarms(now().epoch_ms, now());
        assert_eq!(alarms.len(), 1);
        assert_eq!(alarms[0].task, id);
        assert_eq!(alarms[0].title, "Call the dentist");
        assert_eq!(alarms[0].at_ms, at.as_millisecond());
        assert_eq!(alarms[0].label, "Today, 14:00");
        // Gone once it has passed the window's start, and while its task is done.
        assert!(e.reminder_alarms(at.as_millisecond() + 1, now()).is_empty());
        let done = e.set_done(id.clone(), true).expect("done").expect("handle");
        assert!(e.reminder_alarms(now().epoch_ms, now()).is_empty());
        e.undo_completion(done).expect("undo");
        assert_eq!(e.reminder_alarms(now().epoch_ms, now()).len(), 1);
        e.fixture_delete_task(id).expect("delete");
        assert!(e.reminder_alarms(now().epoch_ms, now()).is_empty());
    }

    #[test]
    fn clearing_the_due_date_also_clears_the_repeat() {
        use todo_core::command::TaskPatch;
        use todo_core::recurrence::{Recurrence, Rule};
        let engine = TodoEngine::fixture("world".into(), fixture_now()).expect("fixture");
        let id = engine
            .add_task(
                RoomKey::default(),
                "Water the plants tomorrow".into(),
                fixture_now(),
            )
            .expect("task");
        let task: TaskId = id.parse().expect("task ID");
        engine
            .client
            .submit(intent::update_task(
                task,
                TaskPatch {
                    recurrence: Some(Some(Recurrence::new(Rule::Daily { interval: 1 }))),
                    ..TaskPatch::default()
                },
            ))
            .expect("repeat");
        assert!(
            engine
                .task(id.clone(), fixture_now())
                .expect("task")
                .repeats
        );
        engine.set_due(id.clone(), None).expect("clear");
        let detail = engine.task(id, fixture_now()).expect("task");
        assert_eq!((detail.due, detail.repeats), (None, false));
        assert_eq!(engine.state().tasks[&task].recurrence, None);
    }

    #[test]
    fn refused_changes_never_show_the_servers_text() {
        let secret = "raw: seq 41 conflicts with \u{201c}Secret title\u{201d}";
        let failed = |op, code: &str| todo_client::FailedEntry {
            entry: OutboxEntry {
                command_id: Uuid::from_u128(7),
                op,
                submitted_at: Timestamp::UNIX_EPOCH,
                attempts: 1,
                acked: None,
                acked_epoch: None,
            },
            failed_at: Timestamp::UNIX_EPOCH,
            rejection: todo_client::Rejection {
                status: 409,
                code: code.into(),
                message: secret.into(),
            },
        };
        let delete = todo_client::Operation::Command {
            command: intent::delete(StepId::from_uuid(Uuid::nil())),
        };
        for code in ["deleted", "not_found", "invalid", "something_new"] {
            let f = failed(delete.clone(), code);
            let why = reason(&f, None);
            assert!(!why.contains("Secret") && !why.contains("seq"), "{why}");
            assert_eq!(why, todo_client::failure::why(&f));
            assert_eq!(describe(&f.entry, None), "Delete a step");
        }
        assert_eq!(
            reason(&failed(delete, "deleted"), None),
            "It was deleted elsewhere."
        );
        let undo = todo_client::Operation::UndoAfter {
            original: Uuid::from_u128(1),
        };
        let f = failed(undo, "undo_conflict");
        assert_eq!(
            reason(&f, Some(Undone::Step)),
            "Couldn't undo: the step was changed again."
        );
        assert_eq!(
            describe(&f.entry, Some(Undone::Step)),
            "Undo deleting a step"
        );
        assert_eq!(describe(&f.entry, None), "Undo a change");
    }

    #[test]
    fn refused_undos_are_worded_by_what_they_reverse_never_with_server_text() {
        for undone in [
            None,
            Some(Undone::TaskDelete),
            Some(Undone::TaskMove),
            Some(Undone::Completion),
            Some(Undone::Step),
            Some(Undone::List),
            Some(Undone::Reminder),
        ] {
            for code in [
                "undo_conflict",
                "undo_no_event",
                "epoch_mismatch",
                "event_mismatch",
                "not_found",
                "something_new",
            ] {
                let message = undo_failure_message(code, undone);
                assert!(message.starts_with("Couldn't undo"), "{message}");
                assert!(!message.contains(code), "{message}");
            }
        }
        assert_eq!(
            undo_failure_message("undo_conflict", Some(Undone::Completion)),
            "Couldn't undo: the task was changed again."
        );
        assert_eq!(
            undo_failure_message("not_found", Some(Undone::Step)),
            "Couldn't undo: the step no longer exists."
        );
        assert_eq!(
            undo_failure_message("undo_conflict", Some(Undone::List)),
            "Couldn't undo: the list was changed again."
        );
        assert_eq!(
            undo_failure_message("undo_conflict", None),
            "Couldn't undo: it was changed again."
        );
        // Every undo reads like the desktop's, moves and deletions included.
        assert_eq!(
            undo_failure_message("undo_conflict", Some(Undone::TaskMove)),
            "Couldn't undo: the task was changed again."
        );
        assert_eq!(
            undo_failure_message("not_found", Some(Undone::TaskDelete)),
            "Couldn't undo: the task no longer exists."
        );
        assert_eq!(
            Undone::of(TaskId::from_uuid(Uuid::nil()).into()),
            Undone::TaskDelete
        );
        assert_eq!(
            Undone::of(StepId::from_uuid(Uuid::nil()).into()),
            Undone::Step
        );
        assert_eq!(
            Undone::of(ListId::from_uuid(Uuid::nil()).into()),
            Undone::List
        );
    }
}
