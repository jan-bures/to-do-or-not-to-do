//! Recovering from a damaged cache without losing unsent changes, as the desktop does
//! (`apps/desktop/src/session.rs`).
//!
//! When the cache won't open because it is damaged, it is copied aside (`todo.damaged-<ms>.sqlite`),
//! the original is removed, and the unsent changes (the outbox and the "couldn't sync" list) are read
//! back from the copy into a new cache, built as `<cache>.recovering` and only then moved into place.
//! The confirmed state comes from the server again on the next sync.
//!
//! Each step is recorded in `<cache>.recovery.json` before the next starts, so a recovery interrupted at
//! any point (a crash, a full disk, a copy that can't be read yet) is finished by the next open, and an
//! empty cache is never opened while unsent changes sit in the copy. A read error never counts as
//! "nothing to keep": the recovery stops and is retried. The quarantined copy is kept until the app is
//! unpaired.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use todo_client::sqlite::SqliteStorage;
use todo_client::storage::{Storage, StorageError, StorageOp};

/// What a recovery carried over, for the app to tell the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CacheRecovery {
    /// Unsent changes carried over to the new cache.
    pub kept: u32,
    /// Unsent changes that were there but couldn't be read back.
    pub lost: u32,
}

/// A recovery couldn't finish (yet). Nothing was deleted: the next open tries again.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RecoveryError(String);

impl From<StorageError> for RecoveryError {
    fn from(e: StorageError) -> Self {
        Self(e.to_string())
    }
}

fn io(what: &'static str) -> impl FnOnce(std::io::Error) -> RecoveryError {
    move |e| RecoveryError(format!("{what}: {e}"))
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    /// The quarantined copy's file name, next to the cache.
    quarantined: String,
    phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    /// Copying the damaged cache (with its log) aside. The original is untouched.
    Copying,
    /// The copy is complete and flushed; the original may still be there.
    Copied,
    /// The original is gone. A cache in its place now can only be the rebuilt one.
    Cleared,
}

/// The files a cache's recovery uses, all in the cache's directory.
#[derive(Clone, Debug)]
pub(crate) struct Files {
    pub(crate) cache: PathBuf,
    record: PathBuf,
    staged: PathBuf,
    dir: PathBuf,
    stem: String,
}

impl Files {
    pub(crate) fn new(cache: &Path) -> Self {
        let dir = cache.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = cache
            .file_stem()
            .map_or_else(|| "cache".into(), |s| s.to_string_lossy().into_owned());
        Self {
            cache: cache.to_path_buf(),
            record: with_suffix(cache, ".recovery.json"),
            staged: with_suffix(cache, ".recovering"),
            dir,
            stem,
        }
    }

    fn quarantine_name(&self) -> String {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        format!("{}.damaged-{ms}.sqlite", self.stem)
    }

    fn is_ours(&self, name: &str) -> bool {
        name.starts_with(&format!("{}.damaged-", self.stem))
            && name.ends_with(".sqlite")
            && !name.contains(['/', '\\', ':'])
            && !name.contains("..")
    }
}

/// Finishes a recovery an earlier open started, if there is one. Call it before opening the cache.
pub(crate) fn resume(files: &Files) -> Result<Option<CacheRecovery>, RecoveryError> {
    match read_record(files)? {
        Some(record) => finish(files, record).map(Some),
        None => Ok(None),
    }
}

/// Sets the damaged cache aside and carries its unsent changes into a new one.
pub(crate) fn recover(files: &Files) -> Result<CacheRecovery, RecoveryError> {
    let record = Record {
        quarantined: files.quarantine_name(),
        phase: Phase::Copying,
    };
    write_record(files, &record)?;
    finish(files, record)
}

fn read_record(files: &Files) -> Result<Option<Record>, RecoveryError> {
    let bytes = match std::fs::read(&files.record) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io("reading the recovery record")(e)),
    };
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|e| RecoveryError(format!("the recovery record is unreadable: {e}")))?;
    if !files.is_ours(&record.quarantined) {
        return Err(RecoveryError(
            "the recovery record names an unexpected file".into(),
        ));
    }
    Ok(Some(record))
}

fn write_record(files: &Files, record: &Record) -> Result<(), RecoveryError> {
    let bytes = serde_json::to_vec(record)
        .map_err(|e| RecoveryError(format!("writing the recovery record: {e}")))?;
    let tmp = with_suffix(&files.record, ".tmp");
    std::fs::write(&tmp, &bytes).map_err(io("writing the recovery record"))?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&tmp)
        .and_then(|f| f.sync_all())
        .map_err(io("writing the recovery record"))?;
    todo_fs::replace_durably(&tmp, &files.record).map_err(io("writing the recovery record"))
}

/// Runs the recovery from wherever `record` says an earlier attempt stopped:
///
/// 1. **Copying:** the damaged cache and its log are copied aside (again, from the untouched original).
/// 2. **Copied:** the original and its log are removed.
/// 3. **Cleared:** the unsent changes are read from the copy (every read must succeed) and written to a
///    freshly built staged cache, which is moved into place. A cache already in place can only be that
///    rebuilt one (removing the record failed), so they are written into it again; entries are keyed by
///    command ID, so nothing is doubled.
fn finish(files: &Files, mut record: Record) -> Result<CacheRecovery, RecoveryError> {
    let quarantined = files.dir.join(&record.quarantined);
    if record.phase == Phase::Copying {
        if !files.cache.exists() {
            // Nothing was ever there to recover.
            std::fs::remove_file(&files.record).map_err(io("removing the recovery record"))?;
            return Ok(CacheRecovery { kept: 0, lost: 0 });
        }
        copy_database(&files.cache, &quarantined).map_err(io("copying the damaged cache aside"))?;
        record.phase = Phase::Copied;
        write_record(files, &record)?;
    }
    if record.phase == Phase::Copied {
        remove_database(&files.cache).map_err(io("removing the damaged cache"))?;
        record.phase = Phase::Cleared;
        write_record(files, &record)?;
    }
    let salvaged = SqliteStorage::salvage_all(&quarantined)
        .map_err(|e| RecoveryError(format!("the unsent changes couldn't be read back yet: {e}")))?;
    let ops: Vec<StorageOp<'_>> = salvaged
        .outbox
        .iter()
        .map(StorageOp::PutOutbox)
        .chain(salvaged.failed.iter().map(StorageOp::PutFailed))
        .collect();
    if files.cache.exists() {
        SqliteStorage::open(&files.cache)?.write(&ops)?;
    } else {
        remove_database(&files.staged).map_err(io("clearing an earlier attempt"))?;
        let mut storage = SqliteStorage::open(&files.staged)?;
        storage.write(&ops)?;
        drop(storage); // The last connection: SQLite folds the log into the file.
        move_database(&files.staged, &files.cache)
            .map_err(io("moving the rebuilt cache in place"))?;
    }
    std::fs::remove_file(&files.record).map_err(io("removing the recovery record"))?;
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Ok(CacheRecovery {
        kept: count(salvaged.outbox.len() + salvaged.failed.len()),
        lost: count(salvaged.lost),
    })
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

/// Copies an SQLite database with its log files (whatever exists), flushing each copy.
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

/// Moves a freshly built (closed) database into place, then any log it still has.
fn move_database(from: &Path, to: &Path) -> std::io::Result<()> {
    remove_database(to)?;
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
    for suffix in ["", "-wal", "-shm", "-journal"] {
        match std::fs::remove_file(with_suffix(path, suffix)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}
