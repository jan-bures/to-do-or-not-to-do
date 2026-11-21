//! Attachment content: encrypted objects in a private bucket, with a hard monthly budget and a garbage
//! collector.
//!
//! | Module | Contents |
//! |---|---|
//! | [`crypto`] | the master key ring and the per-object STREAM (XChaCha20-Poly1305) construction |
//! | [`store`] | the [`BlobStore`](store::BlobStore) trait, the local-filesystem store and the R2 store |
//! | [`registry`] | `attachments.sqlite3`: known objects, the R2 budget, GC state |
//!
//! The HTTP side is [`crate::api::attachments`].
//!
//! # Life of an attachment
//!
//! 1. **Upload.** The content is sealed chunk by chunk into a spool file in the data directory (ciphertext
//!    only; plaintext never touches the disk), counted against the size cap as it arrives. The registry
//!    then, in one transaction, checks storage, charges the PUT and records the object; the object is
//!    put; the `add_attachment` command records the reference, whose `storage_key` is the object key.
//! 2. **Download.** The reference's attachment ID and `storage_key` select the object in the registry; the
//!    GET is charged; the object is decrypted and checked chunk by chunk while it streams to the client.
//! 3. **Delete.** Deleting the attachment (or its task) only writes a tombstone; the object stays, so undo
//!    and restore bring the content back.
//! 4. **Collection.** The garbage collector (at most hourly, a bounded number of deletes per run) notes when
//!    an object stops being referenced by a live attachment, and deletes it once it has stayed that way for
//!    `gc_retention_days`. An object is referenced when the attachment it was sealed for exists, is not
//!    deleted (nor is its task or list), and still carries the object's `storage_key`. Undo or restore
//!    within the window makes it referenced again and resets the clock.
//!
//! # Backups and restores
//!
//! `todo-backup` copies `todo.sqlite3`, not the bucket, not `attachments.sqlite3` and not the key file.
//!
//! - **Restoring an older database.** The registry is untouched, so it still describes the bucket. Objects
//!   uploaded after the backup are no longer referenced and are collected after the retention window.
//!   Attachments in the restored database whose objects were already collected download as 404
//!   `content_unavailable`. To make that impossible, set `gc_retention_days` above the longest backup
//!   retention you restore from (daily backups are kept 90 days); the default of 30 days covers the
//!   7-day frequent backups only.
//! - **Losing the data directory.** Back up `attachments.key` (for example in the password manager)
//!   separately: without it no object can be decrypted. Without `attachments.sqlite3`, objects can no
//!   longer be found or collected (a re-index from the bucket is not implemented).
//!
//! # Budget
//!
//! See [`registry`]. When the budget refuses (an upload that would not fit, or a month's operations used
//! up) the request fails with 507 `storage_budget_exhausted` or 429 `r2_budget_exhausted`, and the first
//! refusal of each month adds one warning task to the owner's Inbox, created by `System`. Nothing is ever
//! sent to the bucket without being charged first, so the free tier cannot be exceeded silently. The
//! counts cover attachments only; `todo-backup` keeps its own, much smaller budget for its bucket.

pub mod crypto;
pub mod registry;
pub mod store;

use crate::auth::Principal;
use crate::auth::limits::KeyedLimiter;
use crate::engine::Engine;
use crypto::{CryptoError, KeyRing, Opener, Sealer};
use registry::{BlobRow, BudgetCaps, OpClass, Refusal, Registry, RegistryError};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use store::{BlobStore, StoreError};
use todo_core::command::{Command, NewTask};
use todo_core::ids::{AttachmentId, EntityId, ListId, TaskId};
use todo_core::time::Timestamp;
use todo_core::{Actor, State};
use tokio::sync::{Semaphore, watch};

/// The hard ceiling for one file: 25 MiB. `max_upload_bytes` may be lower, never higher.
pub const MAX_UPLOAD_BYTES: u64 = 25 * 1024 * 1024;
/// Uploads that never completed (a crash between reserving and storing) are cleaned up after this.
const ABANDONED_AFTER_MS: i64 = 24 * 3600 * 1000;
const LAST_GC: &str = "last_gc_ms";
const SPOOL_DIR: &str = "attachments-spool";

/// Which store holds the objects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreKind {
    /// Files in `local_dir` (default `<data_dir>/blobs`). For tests and running without R2.
    #[default]
    Local,
    /// The R2 bucket named in `credentials_file`.
    R2,
}

/// `[attachments]` in the server config.
///
/// ```toml
/// [attachments]
/// store = "r2"                      # or "local" (the default)
/// credentials_file = "/etc/todo/r2-attachments.json"
/// # local_dir = "/var/lib/todo/blobs"
/// max_upload_bytes = 26214400       # 25 MiB, the most allowed
/// upload_timeout_ms = 600000
/// max_concurrent_uploads = 2
/// max_concurrent_downloads = 4
/// downloads_per_minute = 30         # per token
/// download_burst = 10
/// gc_retention_days = 30
/// gc_interval_mins = 60             # at least 60
/// gc_max_deletes_per_run = 100
///
/// [attachments.budget]
/// max_storage_bytes = 8000000000
/// class_a_monthly = 800000
/// class_b_monthly = 8000000
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AttachmentsConfig {
    /// Where objects go.
    pub store: StoreKind,
    /// Directory of the local store.
    pub local_dir: Option<PathBuf>,
    /// The R2 credentials file (JSON: `endpoint`, `bucket`, `access_key_id`, `secret_access_key`).
    pub credentials_file: Option<PathBuf>,
    /// Largest accepted file.
    pub max_upload_bytes: u64,
    /// Deadline for an upload request, including reading the body.
    pub upload_timeout_ms: u64,
    /// Uploads in progress at once; more get 503 `overloaded`.
    pub max_concurrent_uploads: usize,
    /// Downloads streaming at once; more get 503 `overloaded`.
    pub max_concurrent_downloads: usize,
    /// Sustained downloads per minute per token.
    pub downloads_per_minute: u32,
    /// Download burst per token.
    pub download_burst: u32,
    /// How long an unreferenced object is kept before it is deleted.
    pub gc_retention_days: u64,
    /// How often the garbage collector runs (at least hourly is refused: 60 minutes minimum).
    pub gc_interval_mins: u64,
    /// Most objects deleted per run.
    pub gc_max_deletes_per_run: usize,
    /// The R2 budget.
    pub budget: BudgetCaps,
}

impl Default for AttachmentsConfig {
    fn default() -> Self {
        Self {
            store: StoreKind::Local,
            local_dir: None,
            credentials_file: None,
            max_upload_bytes: MAX_UPLOAD_BYTES,
            upload_timeout_ms: 600_000,
            max_concurrent_uploads: 2,
            max_concurrent_downloads: 4,
            downloads_per_minute: 30,
            download_burst: 10,
            gc_retention_days: 30,
            gc_interval_mins: 60,
            gc_max_deletes_per_run: 100,
            budget: BudgetCaps::default(),
        }
    }
}

impl AttachmentsConfig {
    /// Checks the values.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_upload_bytes == 0 || self.max_upload_bytes > MAX_UPLOAD_BYTES {
            return Err(format!(
                "attachments.max_upload_bytes must be between 1 and {MAX_UPLOAD_BYTES}"
            ));
        }
        if self.gc_interval_mins < 60 {
            return Err("attachments.gc_interval_mins must be at least 60".into());
        }
        for (name, v) in [
            ("upload_timeout_ms", self.upload_timeout_ms),
            ("max_concurrent_uploads", self.max_concurrent_uploads as u64),
            (
                "max_concurrent_downloads",
                self.max_concurrent_downloads as u64,
            ),
            ("downloads_per_minute", u64::from(self.downloads_per_minute)),
            ("download_burst", u64::from(self.download_burst)),
            ("gc_retention_days", self.gc_retention_days),
            ("gc_max_deletes_per_run", self.gc_max_deletes_per_run as u64),
        ] {
            if v == 0 {
                return Err(format!("attachments.{name} must be > 0"));
            }
        }
        if self.store == StoreKind::R2 && self.credentials_file.is_none() {
            return Err("attachments.store = \"r2\" needs attachments.credentials_file".into());
        }
        self.budget.validate()
    }

    /// The upload deadline.
    #[must_use]
    pub fn upload_timeout(&self) -> Duration {
        Duration::from_millis(self.upload_timeout_ms)
    }
}

/// Attachment content failures.
#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    /// The budget refused.
    #[error(transparent)]
    Refused(Refusal),
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Sealing or opening failed (including authentication failures).
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    /// The registry failed.
    #[error("attachment registry: {0}")]
    Registry(rusqlite::Error),
    /// Local I/O failed.
    #[error("attachment spool: {0}")]
    Io(#[from] std::io::Error),
}

impl From<RegistryError> for AttachmentError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::Refused(r) => Self::Refused(r),
            RegistryError::Sql(e) => Self::Registry(e),
        }
    }
}

/// What a garbage-collection run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Objects still referenced.
    pub kept: usize,
    /// Unreferenced objects still inside the retention window.
    pub waiting: usize,
    /// Objects deleted.
    pub deleted: usize,
    /// True if the run stopped early (delete cap or budget).
    pub stopped_early: bool,
}

/// An object being sealed into the spool directory. Dropping it removes the file.
#[derive(Debug)]
pub struct Spool {
    path: PathBuf,
    sealer: Option<Sealer<BufWriter<File>>>,
}

impl Spool {
    /// Plaintext bytes accepted so far.
    #[must_use]
    pub fn plain_len(&self) -> u64 {
        self.sealer.as_ref().map_or(0, Sealer::plain_len)
    }

    /// Seals more content.
    pub fn push(&mut self, data: &[u8]) -> Result<(), AttachmentError> {
        match &mut self.sealer {
            Some(s) => Ok(s.push(data)?),
            None => Err(CryptoError::Corrupt("spool finished").into()),
        }
    }

    /// Seals the last chunk; returns the sealed file (rewound) and its length.
    pub fn finish(mut self) -> Result<Sealed, AttachmentError> {
        let sealer = self
            .sealer
            .take()
            .ok_or(CryptoError::Corrupt("spool finished"))?;
        let plain = sealer.plain_len();
        let (w, len) = sealer.finish()?;
        let mut file = w.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        file.rewind()?;
        Ok(Sealed {
            path: std::mem::take(&mut self.path),
            file: Some(file),
            len,
            plain,
        })
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        drop(self.sealer.take());
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// A sealed object in the spool directory. Dropping it removes the file.
#[derive(Debug)]
pub struct Sealed {
    path: PathBuf,
    file: Option<File>,
    /// Sealed length.
    pub len: u64,
    /// Plaintext length.
    pub plain: u64,
}

impl Drop for Sealed {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Attachment content: key ring, store, registry and the limits of the HTTP side.
pub struct Attachments {
    cfg: AttachmentsConfig,
    engine: Arc<Engine>,
    ring: KeyRing,
    store: Arc<dyn BlobStore>,
    registry: Registry,
    spool_dir: PathBuf,
    /// Upload permits.
    pub uploads: Arc<Semaphore>,
    /// Download permits.
    pub downloads: Arc<Semaphore>,
    /// Downloads per token.
    pub download_limiter: KeyedLimiter<Principal>,
}

impl std::fmt::Debug for Attachments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attachments")
            .field("store", &self.store.kind())
            .finish_non_exhaustive()
    }
}

fn month_of(t: Timestamp) -> String {
    t.strftime("%Y-%m").to_string()
}

/// Seconds from `now` to the start of the next UTC month (when the operation counts reset).
#[must_use]
pub fn seconds_to_next_month(now: Timestamp) -> u32 {
    let utc = jiff::tz::TimeZone::UTC;
    let date = jiff::ZonedDateTime::new(now, utc.clone()).date();
    let (y, m) = if date.month() == 12 {
        (date.year().saturating_add(1), 1)
    } else {
        (date.year(), date.month() + 1)
    };
    jiff::civil::Date::new(y, m, 1)
        .and_then(|d| d.to_zoned_datetime(utc))
        .map_or(86_400, |n| {
            u32::try_from((n.timestamp().as_second() - now.as_second()).max(1)).unwrap_or(u32::MAX)
        })
}

fn deterministic(seed: &str) -> uuid::Uuid {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(seed.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    uuid::Uuid::new_v8(b)
}

/// The command ID of an upload's `add_attachment`: derived from the attachment ID, so a retried upload
/// with the same `attachment_id` can never add a second reference.
#[must_use]
pub fn upload_command_id(attachment: AttachmentId) -> uuid::Uuid {
    deterministic(&format!("todo-server attachment upload {attachment}"))
}

/// True if `row`'s attachment exists, is live (with its task and list), and still names `row`'s key.
fn referenced(state: &State, row: &BlobRow) -> bool {
    let id = AttachmentId::from_uuid(row.attachment);
    // `is_live` also checks the task and its list.
    state
        .attachments
        .get(&id)
        .is_some_and(|a| a.storage_key == row.ref_key)
        && state.is_live(EntityId::Attachment(id))
}

impl Attachments {
    /// Opens the key ring (creating it on first use), the registry and the configured store. Blocking.
    pub fn open(
        cfg: &AttachmentsConfig,
        data_dir: &Path,
        engine: Arc<Engine>,
    ) -> Result<Self, AttachmentError> {
        let store: Arc<dyn BlobStore> = match cfg.store {
            StoreKind::Local => {
                let dir = cfg
                    .local_dir
                    .clone()
                    .unwrap_or_else(|| data_dir.join("blobs"));
                Arc::new(store::LocalStore::open(&dir)?)
            }
            StoreKind::R2 => {
                let path = cfg
                    .credentials_file
                    .as_deref()
                    .ok_or_else(|| StoreError::Config("no credentials_file".into()))?;
                Arc::new(store::R2Store::new(&store::Credentials::load(path)?)?)
            }
        };
        Self::with_store(cfg, data_dir, engine, store)
    }

    /// [`open`](Self::open) with a given store.
    pub fn with_store(
        cfg: &AttachmentsConfig,
        data_dir: &Path,
        engine: Arc<Engine>,
        store: Arc<dyn BlobStore>,
    ) -> Result<Self, AttachmentError> {
        let ring = KeyRing::load_or_create(data_dir)?;
        let registry = Registry::open(&data_dir.join(registry::REGISTRY_FILE), cfg.budget)?;
        let spool_dir = data_dir.join(SPOOL_DIR);
        if spool_dir.is_dir() {
            // Leftovers of interrupted uploads (ciphertext only).
            for e in std::fs::read_dir(&spool_dir)?.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        } else {
            todo_fs::create_private_dir(&spool_dir)?;
        }
        tracing::info!(store = store.kind(), "attachment store ready");
        Ok(Self {
            uploads: Arc::new(Semaphore::new(cfg.max_concurrent_uploads)),
            downloads: Arc::new(Semaphore::new(cfg.max_concurrent_downloads)),
            download_limiter: KeyedLimiter::new(cfg.downloads_per_minute, cfg.download_burst),
            cfg: cfg.clone(),
            engine,
            ring,
            store,
            registry,
            spool_dir,
        })
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &AttachmentsConfig {
        &self.cfg
    }

    /// The registry (budget and objects).
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Now, by the engine's clock.
    #[must_use]
    pub fn now(&self) -> Timestamp {
        self.engine.now()
    }

    /// The current UTC month, by the engine's clock.
    #[must_use]
    pub fn month(&self) -> String {
        month_of(self.engine.now())
    }

    fn now_ms(&self) -> i64 {
        self.engine.now().as_millisecond()
    }

    /// Fails if an upload of at most `plain` bytes would be refused now. Blocking.
    pub fn precheck_upload(&self, plain: u64) -> Result<(), AttachmentError> {
        Ok(self
            .registry
            .check_upload(&self.month(), crypto::sealed_len(plain))?)
    }

    /// Starts sealing a new object. Blocking.
    pub fn spool(
        &self,
        object_key: &str,
        attachment: AttachmentId,
    ) -> Result<Spool, AttachmentError> {
        let path = self.spool_dir.join(format!(
            "{}.sealed",
            object_key.trim_start_matches(store::KEY_PREFIX)
        ));
        let file = todo_fs::create_private_file(&path)?;
        // From here on, dropping `spool` removes the file (also if sealing cannot start).
        let mut spool = Spool { path, sealer: None };
        spool.sealer = Some(Sealer::new(
            &self.ring,
            object_key,
            &attachment.as_uuid(),
            BufWriter::with_capacity(crypto::CHUNK + crypto::TAG, file),
        )?);
        Ok(spool)
    }

    /// Charges and stores a sealed object for `attachment` under `object_key`, recorded as belonging to
    /// the reference whose `storage_key` is `ref_key`. Blocking.
    pub fn store(
        &self,
        object_key: &str,
        attachment: AttachmentId,
        ref_key: &str,
        sealed: &Sealed,
    ) -> Result<BlobRow, AttachmentError> {
        let file = sealed
            .file
            .as_ref()
            .ok_or(CryptoError::Corrupt("spool finished"))?;
        let row = BlobRow {
            object_key: object_key.to_owned(),
            attachment: attachment.as_uuid(),
            ref_key: ref_key.to_owned(),
            plain_size: sealed.plain,
            stored_size: sealed.len,
            stored: false,
            created_ms: self.now_ms(),
            gone_since_ms: None,
        };
        self.registry.reserve_upload(&self.month(), &row)?;
        if let Err(e) = self.store.put(object_key, file, sealed.len) {
            // The PUT stays charged; the object may or may not exist. Forget it only if it surely
            // does not; otherwise the collector deletes it once it is found unreferenced.
            if matches!(e, StoreError::Http { .. } | StoreError::Config(_)) {
                let _ = self.registry.remove(object_key);
            }
            return Err(e.into());
        }
        self.registry.mark_stored(object_key)?;
        Ok(BlobRow {
            stored: true,
            ..row
        })
    }

    /// The stored object of an attachment reference, if any. Blocking.
    pub fn find(
        &self,
        attachment: AttachmentId,
        storage_key: &str,
    ) -> Result<Option<BlobRow>, AttachmentError> {
        Ok(self.registry.find(&attachment.as_uuid(), storage_key)?)
    }

    /// Charges a GET and opens the object for streaming decryption. Blocking.
    pub fn open_object(
        &self,
        row: &BlobRow,
    ) -> Result<Opener<Box<dyn Read + Send>>, AttachmentError> {
        self.registry.charge(&self.month(), OpClass::B)?;
        let src = self.store.get(&row.object_key)?;
        Ok(Opener::new(
            &self.ring,
            &row.object_key,
            &row.attachment,
            src,
        )?)
    }

    /// Files the month's budget warning in the owner's Inbox, once per month. Blocking; never fails
    /// (problems are logged).
    pub fn warn_refused(&self, refusal: &Refusal) {
        let month = self.month();
        match self.registry.claim_warning(&month) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                tracing::error!(error = %e, "cannot record the attachment budget warning");
                return;
            }
        }
        if let Err(why) = self.file_warning(&month, refusal) {
            tracing::warn!(reason = %why, "cannot file the attachment budget warning; will retry");
            let _ = self.registry.release_warning(&month);
        }
    }

    fn file_warning(&self, month: &str, refusal: &Refusal) -> Result<(), String> {
        let state = self.engine.state();
        let inbox: Option<ListId> = state
            .lists
            .values()
            .filter(|l| state.is_live(l.id.into()) && todo_core::query::fold(&l.name) == "inbox")
            .min_by_key(|l| (l.collection.is_some(), l.position.to_string(), l.id))
            .map(|l| l.id);
        let inbox = inbox.ok_or("no list named Inbox")?;
        let task = TaskId::from_uuid(deterministic(&format!(
            "todo-server attachment budget warning {month}"
        )));
        let mut t = NewTask::new(
            task,
            inbox,
            format!("Attachment storage budget reached ({month})"),
        );
        let u = self.registry.usage(month).map_err(|e| e.to_string())?;
        let caps = self.registry.caps();
        t.notes = format!(
            "The server refused an attachment request: {refusal}.\n\n\
             This month: {} of {} Class A and {} of {} Class B operations; {} of {} bytes stored in {} objects.\n\n\
             Uploads (or downloads) stay refused until space is freed or the month ends. Deleted \
             attachments are removed from storage {} days after deletion. The caps are in \
             [attachments.budget] and must stay inside the R2 free tier.",
            u.class_a,
            caps.class_a_monthly,
            u.class_b,
            caps.class_b_monthly,
            u.storage_bytes,
            caps.max_storage_bytes,
            u.objects,
            self.cfg.gc_retention_days,
        );
        let id = deterministic(&format!(
            "todo-server attachment budget warning command {month}"
        ));
        match self
            .engine
            .execute(&Actor::System, id, &Command::CreateTask(Box::new(t)))
        {
            Ok(_) => {
                tracing::warn!(month, "attachment budget reached; warning task filed");
                Ok(())
            }
            // Already filed (the registry flag was lost): nothing to do.
            Err(crate::engine::EngineError::Domain(todo_core::DomainError::AlreadyExists(_))) => {
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Runs the garbage collector if `gc_interval_mins` have passed since the last run. Blocking.
    pub fn collect_garbage_if_due(&self) -> Result<Option<GcReport>, AttachmentError> {
        let now = self.now_ms();
        let interval = i64::try_from(self.cfg.gc_interval_mins)
            .unwrap_or(i64::MAX / 60_000)
            .saturating_mul(60_000);
        if let Some(last) = self.registry.meta(LAST_GC)?
            && now < last.saturating_add(interval)
        {
            return Ok(None);
        }
        self.registry.set_meta(LAST_GC, now)?;
        self.collect_garbage().map(Some)
    }

    /// One garbage-collection run (see the module docs). Blocking.
    pub fn collect_garbage(&self) -> Result<GcReport, AttachmentError> {
        let now = self.now_ms();
        let retention = i64::try_from(self.cfg.gc_retention_days)
            .unwrap_or(i64::MAX / 86_400_000)
            .saturating_mul(86_400_000);
        let state = self.engine.state();
        let rows = self.registry.all()?;
        // The newest stored object of each reference; older ones (a re-upload) are superseded.
        let mut newest: BTreeMap<(uuid::Uuid, &str), &BlobRow> = BTreeMap::new();
        for r in rows.iter().filter(|r| r.stored) {
            newest
                .entry((r.attachment, r.ref_key.as_str()))
                .and_modify(|n| {
                    if (r.created_ms, &r.object_key) > (n.created_ms, &n.object_key) {
                        *n = r;
                    }
                })
                .or_insert(r);
        }
        let mut report = GcReport::default();
        let mut doomed = Vec::new();
        for r in &rows {
            if !r.stored {
                if now.saturating_sub(r.created_ms) >= ABANDONED_AFTER_MS {
                    doomed.push(r);
                }
                continue;
            }
            let current = newest
                .get(&(r.attachment, r.ref_key.as_str()))
                .is_some_and(|n| n.object_key == r.object_key);
            if current && referenced(&state, r) {
                report.kept += 1;
                if r.gone_since_ms.is_some() {
                    self.registry.set_gone_since(&r.object_key, None)?;
                }
                continue;
            }
            match r.gone_since_ms {
                None => {
                    self.registry.set_gone_since(&r.object_key, Some(now))?;
                    report.waiting += 1;
                }
                Some(since) if now.saturating_sub(since) >= retention => doomed.push(r),
                Some(_) => report.waiting += 1,
            }
        }
        let month = self.month();
        for r in doomed {
            if report.deleted >= self.cfg.gc_max_deletes_per_run {
                report.stopped_early = true;
                break;
            }
            if let Err(e) = self.registry.charge(&month, OpClass::A) {
                tracing::warn!(error = %e, "attachment collection paused by the budget");
                report.stopped_early = true;
                break;
            }
            self.store.delete(&r.object_key)?;
            self.registry.remove(&r.object_key)?;
            report.deleted += 1;
        }
        if report.deleted > 0 || report.waiting > 0 {
            tracing::info!(
                deleted = report.deleted,
                waiting = report.waiting,
                kept = report.kept,
                "attachment garbage collection"
            );
        }
        Ok(report)
    }

    /// Runs [`collect_garbage_if_due`](Self::collect_garbage_if_due) every few minutes until shutdown.
    pub fn spawn_collector(self: &Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let me = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5 * 60));
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = shutdown.changed() => return,
                }
                if *shutdown.borrow() {
                    return;
                }
                let m = me.clone();
                match tokio::task::spawn_blocking(move || m.collect_garbage_if_due()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "attachment garbage collection failed")
                    }
                    Err(e) => tracing::error!(error = %e, "attachment garbage collection panicked"),
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
