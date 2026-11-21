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
use std::time::Duration;

use anyhow::Context as _;
use todo_client::native::{HttpTransport, OsEntropy, TokioClock};
use todo_client::platform::Request;
use todo_client::protocol::error_detail;
use todo_client::sqlite::{Salvaged, SqliteStorage};
use todo_client::storage::StorageOp;
use todo_client::{ClientConfig, Storage as _, StorageError, SyncClient, Transport};
use todo_core::Actor;
use todo_core::ids::AttachmentId;

use crate::config::{self, DataDir, Pairing, Paths};
use crate::model::files;

/// How long one attachment download may take, start to finish.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

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
}

impl Session {
    /// Opens the pairing's cache and loads the replica from it (no network yet). Cheap enough to run at
    /// startup, and run off the UI thread then. Never deletes anything.
    pub fn open(rt: Runtime, paths: &Paths, pairing: &Pairing) -> Result<Self, OpenError> {
        let data = paths.data(&pairing.config.pairing);
        let transport = HttpTransport::new(&pairing.config.server, &pairing.token)
            .map_err(|e| OpenError::Config(e.to_string()))?;
        std::fs::create_dir_all(data.path())
            .map_err(|e| OpenError::Unavailable(format!("creating the data directory: {e}")))?;
        let storage = SqliteStorage::open(&data.cache())?;
        let actor = Actor::Owner {
            device: pairing.config.device.clone(),
        };
        let client = SyncClient::new(
            transport.clone(),
            TokioClock,
            OsEntropy,
            storage,
            ClientConfig::new(actor),
        )
        .map_err(|todo_client::replica::OpenError::Storage(e)| OpenError::from(e))?;
        Ok(Self {
            client,
            transport,
            rt,
            data,
            task: std::sync::Mutex::new(None),
        })
    }

    /// Opens the session; if the cache is damaged, moves it aside, carries its unsent changes into a
    /// new cache and opens that (the confirmed state comes from the server again). Any other failure is
    /// returned with the cache untouched.
    pub fn open_or_recover(
        rt: Runtime,
        paths: &Paths,
        pairing: &Pairing,
    ) -> Result<(Self, Option<Recovered>), OpenError> {
        let why = match Self::open(rt.clone(), paths, pairing) {
            Err(OpenError::Corrupt(why)) => why,
            other => return other.map(|s| (s, None)),
        };
        tracing::warn!("the local cache is damaged, setting it aside: {why}");
        let data = paths.data(&pairing.config.pairing);
        let quarantined = quarantine(&data.cache())
            .map_err(|e| OpenError::Unavailable(format!("setting the damaged cache aside: {e}")))?;
        let salvaged = SqliteStorage::salvage(&quarantined).unwrap_or_else(|e| {
            tracing::warn!("nothing could be read back from the damaged cache: {e}");
            Salvaged::default()
        });
        let mut fresh = SqliteStorage::open(&data.cache())?;
        let ops: Vec<StorageOp<'_>> = salvaged
            .outbox
            .iter()
            .map(StorageOp::PutOutbox)
            .chain(salvaged.failed.iter().map(StorageOp::PutFailed))
            .collect();
        fresh.write(&ops)?;
        drop(fresh);
        let session = Self::open(rt, paths, pairing)?;
        let recovered = Recovered {
            kept: salvaged.outbox.len() + salvaged.failed.len(),
            lost: salvaged.lost,
            quarantined,
        };
        Ok((session, Some(recovered)))
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
        }
    }

    /// Stops syncing; the returned future resolves once the sync loops have let go of the cache.
    pub fn shutdown(&self) -> impl Future<Output = ()> + use<> {
        self.client.stop();
        let task = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        async move {
            // A tokio join handle can be awaited from any executor.
            if let Some(task) = task {
                let _ = task.await;
            }
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn server(&self) -> &str {
        self.transport.base_url()
    }

    /// This pairing's data directory.
    pub fn data(&self) -> &DataDir {
        &self.data
    }

    /// Downloads an attachment to be opened: into this pairing's `attachments/<id>/<file name>`, with
    /// the Mark of the Web. Only for names [`files::can_open`] allows. Runs on the network runtime.
    pub fn download_to_open(
        &self,
        id: AttachmentId,
        file_name: &str,
    ) -> impl Future<Output = anyhow::Result<PathBuf>> + use<> {
        let name = files::safe_file_name(file_name);
        let dir = self.data.attachments().join(id.to_string());
        let allowed = files::can_open(&name);
        let fetch = self.fetch(id);
        let job = spawn(&self.rt, async move {
            anyhow::ensure!(allowed, "this kind of file can only be saved, not opened");
            let bytes = fetch.await?;
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(name);
            write_downloaded(&path, &bytes)?;
            Ok(path)
        });
        async move { job.await? }
    }

    /// Downloads an attachment to `path` (chosen by the owner), with the Mark of the Web.
    pub fn download_to(
        &self,
        id: AttachmentId,
        path: PathBuf,
    ) -> impl Future<Output = anyhow::Result<PathBuf>> + use<> {
        let fetch = self.fetch(id);
        let job = spawn(&self.rt, async move {
            let bytes = fetch.await?;
            write_downloaded(&path, &bytes)?;
            Ok(path)
        });
        async move { job.await? }
    }

    fn fetch(&self, id: AttachmentId) -> impl Future<Output = anyhow::Result<Vec<u8>>> + use<> {
        let transport = self
            .transport
            .clone()
            .with_request_timeout(DOWNLOAD_TIMEOUT);
        async move {
            let res = transport
                .send(Request::get(format!("/api/v1/attachments/{id}")))
                .await?;
            if res.status != 200 {
                let why = error_detail(&res)
                    .map_or_else(|| format!("HTTP {}", res.status), |d| d.message);
                anyhow::bail!("the server couldn't send it: {why}");
            }
            Ok(res.body)
        }
    }
}

/// Writes a downloaded file and marks it as coming from the internet (zone 3), so Office opens it in
/// Protected View and SmartScreen checks it. A file that can't be marked isn't left behind.
fn write_downloaded(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    if let Err(e) = mark_of_the_web(path) {
        let _ = std::fs::remove_file(path);
        return Err(anyhow::anyhow!(
            "the file couldn't be marked as downloaded ({e}), so it wasn't kept"
        ));
    }
    Ok(())
}

/// The `Zone.Identifier` alternate data stream Windows reads to know a file came from the internet.
pub fn mark_of_the_web(path: &Path) -> std::io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    std::fs::write(PathBuf::from(stream), b"[ZoneTransfer]\r\nZoneId=3\r\n")
}

/// Moves a damaged cache (and its write-ahead log, which belongs to it) aside as
/// `cache.damaged-<ms>.sqlite`, next to where it was. Returns the new path.
fn quarantine(cache: &Path) -> std::io::Result<PathBuf> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let aside = cache.with_file_name(format!("cache.damaged-{ms}.sqlite"));
    std::fs::rename(cache, &aside)?;
    for suffix in ["-wal", "-shm"] {
        let (mut from, mut to) = (cache.as_os_str().to_owned(), aside.as_os_str().to_owned());
        from.push(suffix);
        to.push(suffix);
        match std::fs::rename(&from, &to) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(aside)
}

/// The saved sign-in couldn't be removed: the device is still signed in.
#[derive(Debug, thiserror::Error)]
#[error("the saved sign-in couldn't be removed: {0}")]
pub struct StillSignedIn(String);

/// How a sign-out ended.
#[derive(Debug, PartialEq, Eq)]
pub enum SignedOut {
    /// The sign-in and all of the pairing's data are gone.
    Clean,
    /// Signed out, but the data directory couldn't be removed yet (a file was still open). It is marked
    /// and removed at the next start.
    DataLeft(String),
}

/// Signs out, in order: waits until syncing has stopped (`stopped`), removes the saved sign-in, marks the
/// pairing's data as signed out, lets go of the session (`release` drops the last handles on the cache),
/// then removes the data, retrying while files are still being closed (`wait` between attempts).
///
/// If the sign-in can't be removed nothing else is touched, and the device stays signed in.
pub async fn sign_out<W: Future<Output = ()>>(
    stopped: impl Future<Output = ()>,
    paths: &Paths,
    data: &DataDir,
    release: impl FnOnce(),
    mut wait: impl FnMut() -> W,
) -> Result<SignedOut, StillSignedIn> {
    stopped.await;
    if let Err(e) = config::forget_pairing(paths) {
        // Removing the config failed after the token went: that is signed out already.
        if !matches!(config::load(paths), Ok(None)) {
            return Err(StillSignedIn(format!("{e:#}")));
        }
    }
    if let Err(e) = config::mark_signed_out(data) {
        tracing::warn!("the signed-out data couldn't be marked: {e}");
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
