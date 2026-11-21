//! Local state in the state directory: what was last uploaded per kind, a process lock, and helpers.

use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Kind, Result};

/// What a successful upload contained. The drill compares a restored copy against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadRecord {
    pub key: String,
    /// `max(events.seq)`, or `None` if the snapshot has no `events` table or it is empty.
    pub max_seq: Option<i64>,
    /// `count(*)` of `events`, or `None` without an `events` table.
    pub events_count: Option<u64>,
    /// SHA-256 of the plaintext snapshot, hex.
    pub snapshot_sha256: String,
    pub snapshot_bytes: u64,
    pub object_bytes: u64,
    /// RFC 3339, UTC.
    pub uploaded_at: String,
}

/// `backup-state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub frequent: Option<UploadRecord>,
    pub daily: Option<UploadRecord>,
}

impl State {
    pub const FILE: &'static str = "backup-state.json";

    pub fn load(state_dir: &Path) -> Result<Self> {
        let path = state_dir.join(Self::FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                Error::Config(format!("state file {} is corrupt: {e}", path.display()))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::io(format!("reading {}", path.display()))(e)),
        }
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        write_json_atomic(&state_dir.join(Self::FILE), self)
    }

    pub fn get(&self, kind: Kind) -> Option<&UploadRecord> {
        match kind {
            Kind::Frequent => self.frequent.as_ref(),
            Kind::Daily => self.daily.as_ref(),
        }
    }

    pub fn set(&mut self, kind: Kind, record: UploadRecord) {
        match kind {
            Kind::Frequent => self.frequent = Some(record),
            Kind::Daily => self.daily = Some(record),
        }
    }

    /// The most recent upload of either kind.
    pub fn latest(&self) -> Option<&UploadRecord> {
        // RFC 3339 UTC timestamps of the same format sort chronologically.
        [self.frequent.as_ref(), self.daily.as_ref()]
            .into_iter()
            .flatten()
            .max_by(|a, b| a.uploaded_at.cmp(&b.uploaded_at))
    }
}

/// Writes JSON to a temporary file next to `path` and renames it into place, so a crash never leaves a
/// half-written file.
pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let ctx = || format!("writing {}", path.display());
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(Error::io(ctx()))?;
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| Error::Config(format!("serializing {}: {e}", path.display())))?;
    tmp.write_all(&json).map_err(Error::io(ctx()))?;
    tmp.as_file().sync_all().map_err(Error::io(ctx()))?;
    tmp.persist(path).map_err(|e| Error::io(ctx())(e.error))?;
    Ok(())
}

/// An exclusive lock on the state directory, held for the whole run. The OS drops it if the process dies,
/// so a killed run never leaves a stale lock.
#[derive(Debug)]
pub struct StateLock {
    _file: File,
}

impl StateLock {
    pub const FILE: &'static str = "todo-backup.lock";

    pub fn acquire(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir)
            .map_err(Error::io(format!("creating {}", state_dir.display())))?;
        let path = state_dir.join(Self::FILE);
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(Error::io(format!("opening {}", path.display())))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(Error::Locked(path)),
            Err(TryLockError::Error(e)) => Err(Error::io(format!("locking {}", path.display()))(e)),
        }
    }
}

/// Prefix of the scratch directories created under the state directory.
pub(crate) const SCRATCH_PREFIX: &str = ".todo-backup-";

/// Removes scratch directories left behind by a killed run. Only call it while holding the [`StateLock`].
pub(crate) fn remove_stale_scratch(state_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(state_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(SCRATCH_PREFIX)
        {
            let path: PathBuf = entry.path();
            match std::fs::remove_dir_all(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "removed stale scratch directory"),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "could not remove stale scratch")
                }
            }
        }
    }
}

pub(crate) fn scratch_dir(state_dir: &Path) -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(SCRATCH_PREFIX)
        .tempdir_in(state_dir)
        .map_err(Error::io(format!(
            "creating a scratch directory in {}",
            state_dir.display()
        )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: &str, at: &str) -> UploadRecord {
        UploadRecord {
            key: key.into(),
            max_seq: Some(1),
            events_count: Some(1),
            snapshot_sha256: "00".into(),
            snapshot_bytes: 1,
            object_bytes: 1,
            uploaded_at: at.into(),
        }
    }

    #[test]
    fn round_trips_and_picks_latest() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(State::load(dir.path()).expect("empty"), State::default());
        let mut s = State::default();
        s.set(Kind::Daily, record("daily/a", "2026-11-20T00:00:00Z"));
        s.set(Kind::Frequent, record("frequent/b", "2026-11-21T00:00:00Z"));
        s.save(dir.path()).expect("save");
        let s = State::load(dir.path()).expect("load");
        assert_eq!(s.latest().map(|r| r.key.as_str()), Some("frequent/b"));
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let held = StateLock::acquire(dir.path()).expect("first");
        assert!(matches!(
            StateLock::acquire(dir.path()),
            Err(Error::Locked(_))
        ));
        drop(held);
        StateLock::acquire(dir.path()).expect("after release");
    }
}
