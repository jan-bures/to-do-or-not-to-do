//! The sync session: `todo-client` against the paired server, on a small tokio runtime of its own.
//!
//! Network I/O never runs on the UI thread. The runtime's two worker threads run the client's send and
//! event loops (`SyncClient::run`), pairing requests and attachment downloads. The UI thread only calls
//! the client's synchronous methods (`view`, `submit`, `status`), which take a lock and, for `submit`,
//! write the outbox entry to the local SQLite cache, and it learns about changes from
//! `SyncClient::changes`, a runtime-agnostic signal it awaits on GPUI's executor.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use todo_client::activity::{ActivityError, ActivityPage, ActivityQuery};
use todo_client::native::{HttpTransport, OsEntropy, TokioClock};
use todo_client::sqlite::SqliteStorage;
use todo_client::storage::StorageOp;
use todo_client::{ClientConfig, Revocation, Storage as _, StorageError, SyncClient};
use todo_core::Actor;
use todo_core::ids::AttachmentId;

use crate::config::{self, DataDir, Pairing, Paths};
use crate::model::files;

/// Why a session couldn't be opened. Only [`OpenError::Corrupt`] leads to rebuilding the cache, and even
/// then the old file is kept aside; nothing is ever deleted because opening failed.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// The saved server address or token can't be used. Pairing again fixes it.
    #[error("the saved sign-in can't be used: {0}")]
    Config(String),
    /// The cache is damaged (not a database, malformed, undecodable).
    #[error("the local data is damaged: {0}")]
    Corrupt(String),
    /// The cache can't be opened now (locked by another copy of the app, no permission, disk full).
    /// Trying again later may work.
    #[error("the local data can't be opened: {0}")]
    Unavailable(String),
}

impl From<StorageError> for OpenError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::Corrupt(why) => Self::Corrupt(why),
            StorageError::Backend(why) => Self::Unavailable(why),
        }
    }
}

/// A damaged cache that was set aside and rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovered {
    /// Unsent changes carried over to the new cache.
    pub kept: usize,
    /// Unsent changes that couldn't be read back.
    pub lost: usize,
    /// Where the damaged file was moved.
    pub quarantined: PathBuf,
}

/// The sync client the desktop runs.
pub type Client = SyncClient<HttpTransport, TokioClock>;

/// The network runtime, shared by pairing and the session.
pub type Runtime = Arc<tokio::runtime::Runtime>;

/// Builds the network runtime (two worker threads; the work is I/O).
pub fn runtime() -> anyhow::Result<Runtime> {
    Ok(Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("todo-sync")
            .enable_all()
            .build()
            .context("starting the sync runtime")?,
    ))
}

/// Runs `fut` on the network runtime; the returned future can be awaited from any executor (GPUI's).
pub fn spawn<F>(rt: &Runtime, fut: F) -> impl Future<Output = anyhow::Result<F::Output>> + use<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = rt.spawn(fut);
    async move { handle.await.context("the sync runtime stopped") }
}

/// A paired device's live session.
#[allow(missing_debug_implementations)] // Holds the client and runtime handles.
pub struct Session {
    client: Client,
    transport: HttpTransport,
    rt: Runtime,
    data: DataDir,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    downloads: Arc<Downloads>,
    updates: Box<std::sync::Mutex<Updates>>,
}

struct Updates {
    /// Consumed on start so the caller does not keep a second update credential.
    token: Option<String>,
    worker: Option<crate::updates::Worker>,
}

/// The session's attachment downloads, so signing out can stop them and wait for them before the data
/// they write to is removed.
#[derive(Default)]
struct Downloads {
    /// `None` once the session is shutting down: no new downloads.
    tasks: std::sync::Mutex<Option<Vec<tokio::task::JoinHandle<()>>>>,
}

impl Downloads {
    fn new() -> Self {
        Self {
            tasks: std::sync::Mutex::new(Some(Vec::new())),
        }
    }

    /// Runs a download on the runtime, unless the session is shutting down.
    fn run<T, J>(&self, rt: &Runtime, job: J) -> impl Future<Output = anyhow::Result<T>> + use<T, J>
    where
        T: Send + 'static,
        J: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let (tx, rx) = futures::channel::oneshot::channel();
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let started = match tasks.as_mut() {
            Some(tasks) => {
                tasks.retain(|t| !t.is_finished());
                tasks.push(rt.spawn(async move {
                    let _ = tx.send(job.await);
                }));
                true
            }
            None => false,
        };
        async move {
            anyhow::ensure!(started, "signing out");
            rx.await
                .map_err(|_| anyhow::anyhow!("the download was stopped"))?
        }
    }

    /// Stops new downloads and cancels running ones; resolves once none is running.
    fn shut(&self) -> impl Future<Output = ()> + use<> {
        let tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or_default();
        for t in &tasks {
            t.abort();
        }
        async move {
            for t in tasks {
                let _ = t.await;
            }
        }
    }
}

/// `recovery.json` in a pairing's directory while a damaged cache is being replaced: which quarantined
/// copy holds the unsent changes, and how far the replacement got. Removed only once the changes are in
/// the cache.
#[derive(Debug, Serialize, Deserialize)]
struct RecoveryRecord {
    quarantined: String,
    phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    /// Copying the damaged cache (with its write-ahead log) aside. The original is untouched.
    Copying,
    /// The copy is complete and flushed; the original may still be there.
    Copied,
    /// The original is gone. A cache in its place now can only be the rebuilt one.
    Cleared,
}

const RECOVERY_FILE: &str = "recovery.json";
const STAGED_CACHE: &str = "cache.recovering.sqlite";

// Tests: makes recovery fail at a named step, as a crash or a full disk would.
#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

fn step(_name: &'static str) -> Result<(), OpenError> {
    #[cfg(test)]
    if FAIL_AT.with(|f| f.get()) == Some(_name) {
        return Err(OpenError::Unavailable(format!(
            "injected failure at {_name}"
        )));
    }
    Ok(())
}

fn unavailable(what: &str) -> impl FnOnce(std::io::Error) -> OpenError + '_ {
    move |e| OpenError::Unavailable(format!("{what}: {e}"))
}

impl Session {
    /// Opens the pairing's cache, finishing or starting a recovery if needed (see
    /// [`open_or_recover`](Self::open_or_recover)).
    pub fn open(rt: Runtime, paths: &Paths, pairing: &Pairing) -> Result<Self, OpenError> {
        Self::open_or_recover(rt, paths, pairing).map(|(s, _)| s)
    }

    /// Opens the cache as it is and loads the replica from it (no network yet). Never deletes anything.
    fn open_cache(rt: Runtime, paths: &Paths, pairing: &Pairing) -> Result<Self, OpenError> {
        let data = paths.data(&pairing.config.pairing);
        let transport = HttpTransport::new(&pairing.config.server, &pairing.token)
            .map_err(|e| OpenError::Config(e.to_string()))?;
        data.check()
            .map_err(unavailable("checking the data directory"))?;
        std::fs::create_dir_all(data.path()).map_err(unavailable("creating the data directory"))?;
        let cache = data.cache();
        let revocation =
            Revocation::for_cache(&cache).map_err(unavailable("reading pairing revocation"))?;
        let transport = transport.with_revocation(revocation.clone());
        let storage = SqliteStorage::open(&cache)?;
        let actor = Actor::Owner {
            device: pairing.config.device.clone(),
        };
        let client = SyncClient::with_revocation(
            transport.clone(),
            TokioClock,
            OsEntropy,
            storage,
            ClientConfig::new(actor),
            revocation,
        )
        .map_err(|todo_client::replica::OpenError::Storage(e)| OpenError::from(e))?;
        Ok(Self {
            client,
            transport,
            rt,
            data,
            task: std::sync::Mutex::new(None),
            downloads: Arc::new(Downloads::new()),
            updates: Box::new(std::sync::Mutex::new(Updates {
                token: Some(pairing.token.clone()),
                worker: None,
            })),
        })
    }

    /// Opens the session. If the cache is damaged, it is moved aside and its unsent changes are carried
    /// into a new cache (the confirmed state comes from the server again). Any other failure is returned
    /// with the cache untouched.
    ///
    /// Recovery is restartable: `recovery.json` names the quarantined file from before it is moved until
    /// its changes are in the cache, and the new cache is built as `cache.recovering.sqlite` and only
    /// then moved into place. An interrupted recovery is finished here, before anything opens a cache,
    /// so an empty cache can never be opened while unsent changes sit in quarantine.
    pub fn open_or_recover(
        rt: Runtime,
        paths: &Paths,
        pairing: &Pairing,
    ) -> Result<(Self, Option<Recovered>), OpenError> {
        let data = paths.data(&pairing.config.pairing);
        data.check()
            .map_err(unavailable("checking the data directory"))?;
        if std::fs::symlink_metadata(data.path().join(START_FRESH_FILE)).is_ok() {
            // Files are half moved aside: nothing opens or salvages them until starting fresh is done.
            return Err(OpenError::Unavailable(
                "starting fresh was interrupted and has to be finished".into(),
            ));
        }
        let mut recovered = match read_record(&data)? {
            Some(record) => finish_recovery(&data, record)?,
            None => None,
        };
        let why = match Self::open_cache(rt.clone(), paths, pairing) {
            Err(OpenError::Corrupt(why)) if recovered.is_none() => why,
            other => return other.map(|s| (s, recovered)),
        };
        tracing::warn!("the local cache is damaged, setting it aside: {why}");
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let record = RecoveryRecord {
            quarantined: format!("cache.damaged-{ms}.sqlite"),
            phase: Phase::Copying,
        };
        write_record(&data, &record)?;
        step("recorded")?;
        recovered = finish_recovery(&data, record)?;
        let session = Self::open_cache(rt, paths, pairing)?;
        Ok((session, recovered))
    }

    /// Starts syncing: initial load or catch-up, then the live event stream, sending queued changes,
    /// and reconnecting with backoff when the network drops.
    pub fn start(&self) {
        let mut task = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if task.is_none() {
            let client = self.client.clone();
            *task = Some(self.rt.spawn(async move { client.run().await }));
            let mut updates = self
                .updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(token) = updates.token.take() {
                updates.worker = crate::updates::Worker::start(
                    self.server(),
                    &token,
                    self.client.revocation().clone(),
                );
            }
        }
    }

    /// Stops syncing and downloading; the returned future resolves once the sync loops have let go of
    /// the cache and no download is still writing.
    pub fn shutdown(&self) -> impl Future<Output = ()> + use<> {
        self.client.stop();
        let downloads = self.downloads.shut();
        let task = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        {
            let mut updates = self
                .updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            updates.token.take();
            updates.worker.take(); // Drop cancels the install-wide worker without blocking sign-out.
        }
        async move {
            // A tokio join handle can be awaited from any executor.
            if let Some(task) = task {
                let _ = task.await;
            }
            downloads.await;
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Reads a page of activity on the network runtime, with the same token and transport as sync.
    /// It never changes the replica. A 401 can only mark this pairing's existing directory.
    pub fn activity(
        &self,
        query: ActivityQuery,
    ) -> impl Future<Output = Result<ActivityPage, ActivityError>> + use<> {
        let client = self.client.clone();
        let read = spawn(&self.rt, async move { client.activity(&query).await });
        async move { read.await.unwrap_or(Err(ActivityError::Unavailable)) }
    }

    pub fn server(&self) -> &str {
        self.transport.base_url()
    }

    /// This pairing's data directory.
    pub fn data(&self) -> &DataDir {
        &self.data
    }

    /// Downloads an attachment to be opened, into a new owner-only directory of its own under this
    /// pairing's `attachments` (a fresh random name each time, created new, never reused or followed
    /// through a link), with the Mark of the Web. Only for names [`files::can_open`] allows. Runs on the
    /// network runtime; signing out cancels it. The directories go with the pairing's data.
    pub fn download_to_open(
        &self,
        id: AttachmentId,
        file_name: &str,
    ) -> impl Future<Output = anyhow::Result<PathBuf>> + use<> {
        let name = files::safe_file_name(file_name);
        let allowed = files::can_open(&name);
        let data = self.data.clone();
        let fetch = self.fetch(id);
        self.downloads.run(&self.rt, async move {
            anyhow::ensure!(allowed, "this kind of file can only be saved, not opened");
            let bytes = fetch.await?;
            write_to_open(&data, &name, &bytes)
        })
    }

    /// Downloads an attachment to `path` (chosen by the owner). The previous file there is replaced only
    /// once the new one is completely written; see [`save_replacing`].
    pub fn download_to(
        &self,
        id: AttachmentId,
        path: PathBuf,
    ) -> impl Future<Output = anyhow::Result<Saved>> + use<> {
        let fetch = self.fetch(id);
        self.downloads.run(&self.rt, async move {
            let bytes = fetch.await?;
            let marked = save_replacing(&path, &bytes, mark_of_the_web)?;
            Ok(Saved { path, marked })
        })
    }

    fn fetch(&self, id: AttachmentId) -> impl Future<Output = anyhow::Result<Vec<u8>>> + use<> {
        let transport = self.transport.clone();
        async move {
            transport
                .download_attachment_bytes(id)
                .await
                .map_err(|error| anyhow::anyhow!(error.download_message()))
        }
    }
}

/// A saved attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub path: PathBuf,
    /// Whether it carries the Mark of the Web. False on file systems without alternate data streams
    /// (FAT, exFAT): the file is saved anyway, and the owner is told.
    pub marked: bool,
}

/// 32 random hex digits, for names nobody can guess in advance.
fn random_name() -> String {
    use todo_client::Entropy as _;
    let mut bytes = [0u8; 16];
    OsEntropy.fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Saves `bytes` at `path` without ever losing what was there: they are written to a new temporary file
/// in the same directory, flushed and marked with `mark`, and only then moved over `path` in one step
/// (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`). On any failure the temporary file is removed and
/// `path` is untouched. A mark that can't be written (no alternate data streams) doesn't stop the save;
/// the result says so.
pub fn save_replacing(
    path: &Path,
    bytes: &[u8],
    mark: impl FnOnce(&Path) -> std::io::Result<()>,
) -> anyhow::Result<bool> {
    use std::io::Write as _;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} has no directory", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("{} has no file name", path.display()))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.download", &random_name()[..12]));
    let tmp = dir.join(tmp_name);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("creating a file in {}", dir.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        let marked = match mark(&tmp) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("the downloaded file couldn't be marked: {e}");
                false
            }
        };
        todo_fs::replace_durably(&tmp, path)
            .with_context(|| format!("replacing {}", path.display()))?;
        Ok(marked)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Writes an attachment to be opened: a new owner-only directory with a random name under the pairing's
/// `attachments` (itself checked to be a plain directory, not a link), and the file created new inside
/// it. It must carry the Mark of the Web, or it isn't kept.
fn write_to_open(data: &DataDir, name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    use std::io::Write as _;
    data.check_ancestors()?;
    for dir in [data.path().to_path_buf(), data.attachments()] {
        match todo_fs::create_private_dir(&dir) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(e).with_context(|| format!("creating {}", dir.display()));
            }
            _ => {}
        }
        let m = std::fs::symlink_metadata(&dir)?;
        anyhow::ensure!(
            todo_fs::is_plain(&m) && m.is_dir(),
            "{} is a link or not a directory",
            dir.display()
        );
    }
    let dir = data.attachments().join(random_name());
    todo_fs::create_private_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(name);
    let mut file = todo_fs::create_private_file(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if let Err(e) = mark_of_the_web(&path) {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        anyhow::bail!("the file couldn't be marked as downloaded ({e}), so it wasn't opened");
    }
    Ok(path)
}

/// The `Zone.Identifier` alternate data stream Windows reads to know a file came from the internet.
pub fn mark_of_the_web(path: &Path) -> std::io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    std::fs::write(PathBuf::from(stream), b"[ZoneTransfer]\r\nZoneId=3\r\n")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

/// Copies an SQLite database with its write-ahead log and shared memory (whatever exists), flushing each
/// copy. The original is left as it is.
fn copy_database(from: &Path, to: &Path) -> std::io::Result<()> {
    remove_database(to)?;
    for suffix in ["", "-wal", "-shm"] {
        let (f, t) = (with_suffix(from, suffix), with_suffix(to, suffix));
        match std::fs::copy(&f, &t) {
            Ok(_) => std::fs::OpenOptions::new()
                .write(true)
                .open(&t)?
                .sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Moves a freshly built database (closed, so without a log) into place, then any log it still has.
fn move_database(from: &Path, to: &Path) -> std::io::Result<()> {
    todo_fs::replace_durably(from, to)?;
    for suffix in ["-wal", "-shm"] {
        match todo_fs::replace_durably(&with_suffix(from, suffix), &with_suffix(to, suffix)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

fn remove_database(path: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        match std::fs::remove_file(Path::new(&p)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

fn read_record(data: &DataDir) -> Result<Option<RecoveryRecord>, OpenError> {
    let path = data.path().join(RECOVERY_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unavailable("reading the recovery record")(e)),
    };
    let record: RecoveryRecord = serde_json::from_slice(&bytes)
        .map_err(|e| OpenError::Unavailable(format!("the recovery record is unreadable: {e}")))?;
    // Only ever a file of ours in this directory.
    let name = &record.quarantined;
    let ours = name.starts_with("cache.damaged-")
        && name.ends_with(".sqlite")
        && !name.contains(['/', '\\', ':'])
        && !name.contains("..");
    if !ours {
        return Err(OpenError::Unavailable(format!(
            "the recovery record names an unexpected file: {name:?}"
        )));
    }
    Ok(Some(record))
}

fn write_record(data: &DataDir, record: &RecoveryRecord) -> Result<(), OpenError> {
    let bytes = serde_json::to_vec(record)
        .map_err(|e| OpenError::Unavailable(format!("writing the recovery record: {e}")))?;
    write_durably(&data.path().join(RECOVERY_FILE), &bytes)
        .map_err(unavailable("writing the recovery record"))
}

/// Writes `bytes` to `path` through `<path>.tmp`, flushed, and moves it into place durably.
fn write_durably(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&tmp)?
        .sync_all()?;
    todo_fs::replace_durably(&tmp, path)
}

/// Replaces a damaged cache, from wherever `record` says an earlier attempt stopped, and removes the
/// record once the unsent changes are in the new cache. Each phase is recorded before the next starts,
/// so this is safe to run again after a failure or crash at any point:
///
/// 1. **Copying:** the damaged cache and its log are copied aside (again, from the untouched original).
/// 2. **Copied:** the original and its log are removed.
/// 3. **Cleared:** the unsent changes are read from the copy, and any read error stops here to be retried
///    (never taken as "nothing to keep"). With no cache in place they are written to
///    `cache.recovering.sqlite` (rebuilt from scratch every time), which is then moved into place. A
///    cache already in place can only be that rebuilt one (the record removal failed), so they are
///    written into it again; entries are keyed by command ID, so nothing is doubled.
fn finish_recovery(
    data: &DataDir,
    mut record: RecoveryRecord,
) -> Result<Option<Recovered>, OpenError> {
    let quarantined = data.path().join(&record.quarantined);
    let record_path = data.path().join(RECOVERY_FILE);
    let cache = data.cache();
    if record.phase == Phase::Copying {
        if !cache.exists() {
            // Nothing was ever there to recover.
            std::fs::remove_file(&record_path)
                .map_err(unavailable("removing the recovery record"))?;
            return Ok(None);
        }
        copy_database(&cache, &quarantined)
            .map_err(unavailable("copying the damaged cache aside"))?;
        step("copied")?;
        record.phase = Phase::Copied;
        write_record(data, &record)?;
    }
    if record.phase == Phase::Copied {
        remove_database(&cache).map_err(unavailable("removing the damaged cache"))?;
        step("cleared")?;
        record.phase = Phase::Cleared;
        write_record(data, &record)?;
    }
    // Every read must succeed: an error here (the copy is locked, the disk fails, a page can't be read)
    // stops the recovery with the record and the copy kept, so the next start tries again. Only a copy
    // that reads cleanly to the end and has no unsent changes counts as empty.
    step("salvaging")?;
    let salvaged = SqliteStorage::salvage_all(&quarantined).map_err(|e| {
        OpenError::Unavailable(format!(
            "the unsent changes couldn't be read back from {} yet: {e}",
            record.quarantined
        ))
    })?;
    let ops: Vec<StorageOp<'_>> = salvaged
        .outbox
        .iter()
        .map(StorageOp::PutOutbox)
        .chain(salvaged.failed.iter().map(StorageOp::PutFailed))
        .collect();
    if cache.exists() {
        SqliteStorage::open(&cache)?.write(&ops)?;
    } else {
        let staged = data.path().join(STAGED_CACHE);
        remove_database(&staged).map_err(unavailable("clearing an earlier attempt"))?;
        let mut storage = SqliteStorage::open(&staged)?;
        storage.write(&ops)?;
        drop(storage); // The last connection: SQLite folds the log into the file and removes it.
        step("staged")?;
        move_database(&staged, &cache).map_err(unavailable("moving the rebuilt cache in place"))?;
    }
    step("swapped")?;
    std::fs::remove_file(&record_path).map_err(unavailable("removing the recovery record"))?;
    Ok(Some(Recovered {
        kept: salvaged.outbox.len() + salvaged.failed.len(),
        lost: salvaged.lost,
        quarantined,
    }))
}

/// Where [`Session::start_fresh`] put the damaged data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedFresh {
    /// `damaged\<ms>\` in the pairing's directory: the damaged cache, its copy, anything rebuilt so far
    /// and the records, moved as they were. Removed with the pairing's data at sign-out.
    pub kept: PathBuf,
}

/// Something unfinished that opening stopped at, and so what the screen offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    /// A recovery of damaged data couldn't finish: try again, or start fresh.
    Recovery,
    /// Starting fresh was interrupted with files half moved: it can only be finished.
    StartFresh,
}

/// The pairing's folder for damaged data set aside by "Start fresh".
const DAMAGED_DIR: &str = "damaged";
/// `start-fresh.json` while the damaged data is being moved aside: the folder it goes to. Written before
/// anything moves and moved there last, so an interrupted start fresh finishes into the same folder, and
/// nothing opens or salvages the half-moved files meanwhile.
const START_FRESH_FILE: &str = "start-fresh.json";

#[derive(Debug, Serialize, Deserialize)]
struct StartFreshRecord {
    /// The folder's name under `damaged`: digits only.
    dest: String,
}

impl Session {
    /// What opening stopped at, if anything is unfinished.
    pub fn pending(paths: &Paths, pairing: &Pairing) -> Option<Pending> {
        let dir = paths.data(&pairing.config.pairing).path().to_path_buf();
        let exists = |name: &str| std::fs::symlink_metadata(dir.join(name)).is_ok();
        if exists(START_FRESH_FILE) {
            Some(Pending::StartFresh)
        } else if exists(RECOVERY_FILE) {
            Some(Pending::Recovery)
        } else {
            None
        }
    }

    /// Gives up on a recovery that can't finish: moves the damaged data (never deleting it) to a new
    /// folder `damaged\<ms>\`, then opens an empty cache, which loads from the server once started.
    /// Unsent changes in the damaged copy are not recovered.
    ///
    /// The folder is recorded in `start-fresh.json` before anything moves. Each database moves with its
    /// log files, then the recovery record, then `start-fresh.json` last. An interrupted run is finished
    /// by the next one, into the same folder.
    pub fn start_fresh(
        rt: Runtime,
        paths: &Paths,
        pairing: &Pairing,
    ) -> Result<(Self, StartedFresh), OpenError> {
        let data = paths.data(&pairing.config.pairing);
        let kept = set_aside(&data)?;
        tracing::warn!("started fresh; the damaged data was kept aside");
        let session = Self::open_cache(rt, paths, pairing)?;
        Ok((session, StartedFresh { kept }))
    }
}

/// Whether `name` in a pairing's directory is part of a damaged cache or its recovery: the cache, the
/// quarantined copies, the staged rebuild (each with its log files) and a half-written record.
fn damaged_name(name: &str) -> bool {
    let base = database_of(name);
    base == config::CACHE_FILE
        || base == STAGED_CACHE
        || (base.starts_with("cache.damaged-") && base.ends_with(".sqlite"))
        || name == format!("{RECOVERY_FILE}.tmp")
}

/// The database a file belongs to: its own name, without a `-wal` or `-shm` suffix.
fn database_of(name: &str) -> &str {
    name.strip_suffix("-wal")
        .or_else(|| name.strip_suffix("-shm"))
        .unwrap_or(name)
}

fn aside(e: std::io::Error) -> OpenError {
    OpenError::Unavailable(format!("setting the damaged data aside: {e}"))
}

fn refused(what: String) -> OpenError {
    OpenError::Unavailable(format!("{what}, so nothing was set aside"))
}

/// The damaged files to move, grouped by database, each group's log files before the database itself.
/// Every one must be a plain file: anything else (a link, a directory) refuses the whole move.
fn damaged_files(data: &DataDir) -> Result<Vec<Vec<String>>, OpenError> {
    let mut groups: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for entry in std::fs::read_dir(data.path()).map_err(aside)? {
        let entry = entry.map_err(aside)?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !damaged_name(&name) {
            continue;
        }
        let m = std::fs::symlink_metadata(entry.path()).map_err(aside)?;
        if !todo_fs::is_plain(&m) || !m.is_file() {
            return Err(refused(format!("{name} is a link or not a file")));
        }
        groups
            .entry(database_of(&name).to_owned())
            .or_default()
            .push(name);
    }
    Ok(groups
        .into_iter()
        .map(|(base, mut names)| {
            names.sort_by_key(|n| (*n == base, n.clone()));
            names
        })
        .collect())
}

/// Creates an owner-only directory, or accepts one that is already there, as long as it is a plain
/// directory.
fn private_dir(dir: &Path) -> Result<(), OpenError> {
    match todo_fs::create_private_dir(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(aside(e)),
        _ => {}
    }
    let m = std::fs::symlink_metadata(dir).map_err(aside)?;
    if todo_fs::is_plain(&m) && m.is_dir() {
        Ok(())
    } else {
        Err(refused(format!(
            "{} is a link or not a directory",
            dir.display()
        )))
    }
}

/// Reads `start-fresh.json`, if a start fresh is under way.
fn read_start_fresh(path: &Path) -> Result<Option<String>, OpenError> {
    let m = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(aside(e)),
    };
    if !todo_fs::is_plain(&m) || !m.is_file() {
        return Err(refused(format!(
            "{START_FRESH_FILE} is a link or not a file"
        )));
    }
    let bytes = std::fs::read(path).map_err(aside)?;
    let record: StartFreshRecord = serde_json::from_slice(&bytes)
        .map_err(|e| OpenError::Unavailable(format!("{START_FRESH_FILE} is unreadable: {e}")))?;
    let digits = !record.dest.is_empty() && record.dest.bytes().all(|b| b.is_ascii_digit());
    if !digits {
        return Err(OpenError::Unavailable(format!(
            "{START_FRESH_FILE} names an unexpected folder: {:?}",
            record.dest
        )));
    }
    Ok(Some(record.dest))
}

/// Moves `from` to `to` if it is there: a plain file, durably. A link or a directory refuses.
fn move_if_there(from: &Path, to: &Path) -> Result<(), OpenError> {
    match std::fs::symlink_metadata(from) {
        Ok(m) if todo_fs::is_plain(&m) && m.is_file() => {
            todo_fs::replace_durably(from, to).map_err(aside)
        }
        Ok(_) => Err(refused(format!(
            "{} is a link or not a file",
            from.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(aside(e)),
    }
}

fn set_aside(data: &DataDir) -> Result<PathBuf, OpenError> {
    // The pairing's directory itself must be a plain directory before anything is created or listed.
    data.check().map_err(aside)?;
    let groups = damaged_files(data)?;
    let record = data.path().join(START_FRESH_FILE);
    let dest = match read_start_fresh(&record)? {
        Some(dest) => dest,
        None => {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            let dest = ms.to_string();
            let bytes = serde_json::to_vec(&StartFreshRecord { dest: dest.clone() })
                .map_err(|e| OpenError::Unavailable(format!("writing {START_FRESH_FILE}: {e}")))?;
            write_durably(&record, &bytes).map_err(aside)?;
            dest
        }
    };
    step("recorded-fresh")?;
    let damaged = data.path().join(DAMAGED_DIR);
    private_dir(&damaged)?;
    let kept = damaged.join(&dest);
    private_dir(&kept)?;
    for names in groups {
        for name in names {
            todo_fs::replace_durably(&data.path().join(&name), &kept.join(&name)).map_err(aside)?;
            if name.ends_with("-wal") {
                step("moved-wal")?;
            }
        }
    }
    // The live cache must be vacant before the records go, or the next start would open what is there.
    for suffix in ["", "-wal", "-shm"] {
        let path = with_suffix(&data.cache(), suffix);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(aside(e)),
            Ok(_) => return Err(refused(format!("{} is still there", path.display()))),
        }
    }
    move_if_there(&data.path().join(RECOVERY_FILE), &kept.join(RECOVERY_FILE))?;
    // Last: once this is gone, the next start opens the new cache.
    move_if_there(&record, &kept.join(START_FRESH_FILE))?;
    Ok(kept)
}

/// Signing out stopped before anything was removed: the device is still signed in, and signing out
/// can simply be tried again.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StillSignedIn(String);

impl StillSignedIn {
    /// What the status bar says.
    pub fn notice(&self) -> String {
        format!(
            "Couldn't sign out: {}. You're still signed in; try again.",
            self.0
        )
    }
}

/// How a sign-out ended.
#[derive(Debug, PartialEq, Eq)]
pub enum SignedOut {
    /// The sign-in and all of the pairing's data are gone.
    Clean,
    /// Signed out, but the data directory couldn't be removed yet (a file was still open). It is marked
    /// and removed at the next start.
    DataLeft(String),
}

/// Signs out, in order: waits until syncing and downloads have stopped (`stopped`, from
/// [`Session::shutdown`]), records that the pairing's data is to be removed (beside it, so a crash from
/// here on is finished at the next start), removes the saved sign-in, lets go of the session (`release`
/// drops the last handles on the cache), then removes the data, retrying while files are still being
/// closed (`wait` between attempts), and the record last.
///
/// If the record can't be written and flushed, nothing is removed and the device stays signed in. If
/// the sign-in can't be removed, the record is withdrawn, nothing else is touched, and the device stays
/// signed in. Either way signing out can be tried again.
pub async fn sign_out<W: Future<Output = ()>>(
    stopped: impl Future<Output = ()>,
    paths: &Paths,
    data: &DataDir,
    release: impl FnOnce(),
    mut wait: impl FnMut() -> W,
) -> Result<SignedOut, StillSignedIn> {
    stopped.await;
    if let Err(e) = config::mark_signed_out(data) {
        // Without the record a crash later on could leave the data behind for good: stop while
        // nothing is removed yet.
        return Err(StillSignedIn(format!(
            "the data to remove couldn't be recorded: {e}"
        )));
    }
    if let Err(e) = config::forget_pairing(paths) {
        // Removing the config failed after the token went: that is signed out already.
        if !matches!(config::load(paths), Ok(None)) {
            config::unmark_signed_out(data);
            return Err(StillSignedIn(format!(
                "the saved sign-in couldn't be removed: {e:#}"
            )));
        }
    }
    release();
    let mut last = String::new();
    for _ in 0..50 {
        match config::remove_data(data) {
            Ok(()) => return Ok(SignedOut::Clean),
            Err(e) => {
                last = e.to_string();
                wait().await;
            }
        }
    }
    tracing::warn!("the signed-out data couldn't be removed yet: {last}");
    Ok(SignedOut::DataLeft(last))
}

impl Drop for Session {
    fn drop(&mut self) {
        self.client.stop();
    }
}

#[cfg(test)]
mod tests;
