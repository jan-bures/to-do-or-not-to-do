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
use todo_client::native::{HttpTransport, OsEntropy, TokioClock};
use todo_client::platform::Request;
use todo_client::protocol::error_detail;
use todo_client::sqlite::SqliteStorage;
use todo_client::{ClientConfig, SyncClient, Transport};
use todo_core::Actor;
use todo_core::ids::AttachmentId;

use crate::config::Pairing;

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
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Session {
    /// Opens the cache at `cache` and loads the replica from it (no network yet). Cheap enough to run
    /// at startup, and run off the UI thread then.
    pub fn open(rt: Runtime, cache: &Path, pairing: &Pairing) -> anyhow::Result<Self> {
        if let Some(dir) = cache.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let transport = HttpTransport::new(&pairing.config.server, &pairing.token)?;
        let storage = SqliteStorage::open(cache).context("opening the local cache")?;
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
        .context("loading the local cache")?;
        Ok(Self {
            client,
            transport,
            rt,
            task: std::sync::Mutex::new(None),
        })
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

    /// Downloads an attachment into `dir/<id>/<file name>` and returns the path. Runs on the network
    /// runtime, file write included.
    pub fn download(
        &self,
        id: AttachmentId,
        file_name: &str,
        dir: PathBuf,
    ) -> impl Future<Output = anyhow::Result<PathBuf>> + use<> {
        let transport = self.transport.clone();
        let name = safe_file_name(file_name);
        let job = spawn(&self.rt, async move {
            let res = transport
                .send(Request::get(format!("/api/v1/attachments/{id}")))
                .await?;
            if res.status != 200 {
                let why = error_detail(&res)
                    .map_or_else(|| format!("HTTP {}", res.status), |d| d.message);
                anyhow::bail!("the server couldn't send it: {why}");
            }
            let dir = dir.join(id.to_string());
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(name);
            std::fs::write(&path, &res.body)?;
            Ok(path)
        });
        async move { job.await? }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.client.stop();
    }
}

/// A file name safe to create on Windows: no directories, no reserved characters or names, not empty.
pub fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut clean: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    clean = clean.trim().trim_end_matches(['.', ' ']).to_string();
    let stem = clean
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if clean.is_empty() {
        clean = "attachment".into();
    } else if reserved {
        clean = format!("attachment-{clean}");
    }
    clean.chars().take(120).collect()
}

#[cfg(test)]
mod tests;
