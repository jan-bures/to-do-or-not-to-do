//! Local state in the state directory: what was last uploaded per kind, a process lock, and helpers.

use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Kind, Result, secure};

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

/// Writes JSON so that it is on disk before this returns, and a crash at any point leaves either the old
/// file or the new one, never a mix:
///
/// 1. write a temporary file in the same directory and fsync it;
/// 2. rename it over `path` durably ([`secure::replace_durably`]: `MoveFileExW` with
///    `MOVEFILE_WRITE_THROUGH` on Windows, `rename` plus an fsync of the directory on Unix).
///
/// The budget relies on this: a charge is durable before the request it pays for is sent.
pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let ctx = || format!("writing {}", path.display());
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| Error::Config(format!("serializing {}: {e}", path.display())))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(Error::io(ctx()))?;
    tmp.write_all(&json).map_err(Error::io(ctx()))?;
    tmp.as_file().sync_all().map_err(Error::io(ctx()))?;
    // Close the handle first: Windows can't rename an open file.
    let tmp = tmp.into_temp_path();
    secure::replace_durably(&tmp, path).map_err(Error::io(ctx()))?;
    // The temporary name no longer exists; don't try to delete it on drop.
    let _ = tmp.keep();
    Ok(())
}

/// Creates `dir` owner-only if it doesn't exist (its parents normally). An existing directory is used as it
/// is.
pub(crate) fn ensure_private_dir(dir: &Path) -> Result<()> {
    match std::fs::metadata(dir) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(Error::Config(format!(
            "{} exists but is not a directory",
            dir.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .map_err(Error::io(format!("creating {}", parent.display())))?;
            }
            match secure::create_private_dir(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(e) => Err(Error::io(format!("creating {}", dir.display()))(e)),
            }
        }
        Err(e) => Err(Error::io(format!("reading {}", dir.display()))(e)),
    }
}

/// An exclusive lock on the state directory, held for the whole run. The OS drops it if the process dies,
/// so a killed run never leaves a stale lock.
///
/// Dropping it unlocks explicitly before closing the file. A child process forked at that moment (by this
/// program or, in tests, by another thread) briefly shares the open file description until it execs, and
/// with `flock` semantics closing our descriptor alone would leave the lock held by that copy.
#[derive(Debug)]
pub struct StateLock {
    file: File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        if let Err(e) = self.file.unlock() {
            tracing::debug!("unlocking the state directory failed: {e}");
        }
    }
}

impl StateLock {
    pub const FILE: &'static str = "todo-backup.lock";

    pub fn acquire(state_dir: &Path) -> Result<Self> {
        ensure_private_dir(state_dir)?;
        let path = state_dir.join(Self::FILE);
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(Error::io(format!("opening {}", path.display())))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(Error::Locked(path)),
            Err(TryLockError::Error(e)) => Err(Error::io(format!("locking {}", path.display()))(e)),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Scratch and staging directories
//
// Temporary plaintext (snapshots, restored databases) only ever lives in directories this tool created
// itself: owner-only, and marked with a file whose exact contents the tool wrote. Cleanup deletes nothing
// else. Run scratch lives under `<state_dir>/scratch/`; restore staging lives next to the destination (so the
// final rename stays on one volume) and is registered in the state directory before it is created, so a
// killed restore's leftovers are found again.

/// The private directory under the state directory that holds per-run scratch directories.
pub const SCRATCH_DIR: &str = "scratch";
/// Prefix of restore staging directories next to a restore destination.
pub const STAGING_PREFIX: &str = ".todo-restore-";
/// The marker file inside every directory the tool may delete.
pub const MARKER_FILE: &str = ".todo-backup-marker";
const MARKER_TEXT: &str = "Created by todo-backup for temporary files. todo-backup removes this directory when it is done.\n";
/// Restore staging directories that may still exist, as absolute paths.
const STAGING_REGISTRY: &str = "restore-staging.json";

fn unique_name(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "{prefix}{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Creates `path` owner-only and writes the marker into it. Removes it again if the marker can't be written.
fn create_marked(path: &Path) -> Result<()> {
    secure::create_private_dir(path).map_err(Error::io(format!("creating {}", path.display())))?;
    let marked = (|| {
        let mut f = secure::create_private_file(&path.join(MARKER_FILE))?;
        f.write_all(MARKER_TEXT.as_bytes())?;
        f.sync_all()
    })();
    if let Err(e) = marked {
        let _ = std::fs::remove_dir_all(path);
        return Err(Error::io(format!("marking {}", path.display()))(e));
    }
    Ok(())
}

/// True for a real directory: not a symlink, junction or other reparse point.
fn is_plain_dir(m: &std::fs::Metadata) -> bool {
    m.is_dir() && secure::is_plain(m)
}

/// True only for a real directory (not a symlink, junction or other reparse point) that holds our marker,
/// itself a plain file, with our exact text.
pub(crate) fn is_marked_dir(path: &Path) -> bool {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| is_plain_dir(&m)) {
        return false;
    }
    let marker = path.join(MARKER_FILE);
    let plain_file =
        std::fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file() && secure::is_plain(&m));
    plain_file && std::fs::read_to_string(&marker).is_ok_and(|t| t == MARKER_TEXT)
}

/// A marked directory, removed with everything in it when dropped.
#[derive(Debug)]
pub struct MarkedDir {
    path: PathBuf,
}

impl MarkedDir {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MarkedDir {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %self.path.display(), error = %e, "could not remove scratch directory")
            }
        }
    }
}

/// `<state_dir>/scratch`, created owner-only if missing. Refuses a symlink, junction or non-directory there.
fn scratch_root(state_dir: &Path) -> Result<PathBuf> {
    let root = state_dir.join(SCRATCH_DIR);
    match std::fs::symlink_metadata(&root) {
        Ok(m) if is_plain_dir(&m) => Ok(root),
        Ok(_) => Err(Error::Config(format!(
            "{} is not a plain directory; refusing to use it for scratch files",
            root.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            secure::create_private_dir(&root)
                .map_err(Error::io(format!("creating {}", root.display())))?;
            Ok(root)
        }
        Err(e) => Err(Error::io(format!("reading {}", root.display()))(e)),
    }
}

/// A fresh scratch directory for one run, under `<state_dir>/scratch/`.
pub(crate) fn scratch_dir(state_dir: &Path) -> Result<MarkedDir> {
    let path = scratch_root(state_dir)?.join(unique_name("run-"));
    create_marked(&path)?;
    Ok(MarkedDir { path })
}

/// Removes what killed runs left behind: marked directories directly under `<state_dir>/scratch/`, and
/// registered restore staging directories that are still marked. Never recurses into anything else, and
/// never touches anything outside those places. Only call it while holding the [`StateLock`].
pub(crate) fn clean_leftovers(state_dir: &Path) {
    clean_scratch(&state_dir.join(SCRATCH_DIR));
    clean_registered_staging(state_dir);
}

fn clean_scratch(root: &Path) {
    // Never enumerate a scratch root that has been swapped for a symlink or junction: its "entries" would be
    // somewhere else entirely.
    match std::fs::symlink_metadata(root) {
        Ok(m) if is_plain_dir(&m) => {}
        Ok(_) => {
            tracing::warn!(path = %root.display(), "scratch directory is not a plain directory; not cleaning it");
            return;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(path = %root.display(), error = %e, "could not check the scratch directory");
            return;
        }
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_marked_dir(&path) {
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), "removed leftover scratch directory")
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "could not remove leftover scratch")
                    }
                }
            } else {
                tracing::warn!(path = %path.display(), "unrecognised entry in the scratch directory; leaving it");
            }
        }
    }
}

/// The registered staging paths. A missing registry is empty; an unreadable or invalid one is an error, so
/// that no caller rewrites it and loses the paths it lists.
fn load_registry(state_dir: &Path) -> Result<Vec<PathBuf>> {
    let path = state_dir.join(STAGING_REGISTRY);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            Error::Config(format!(
                "{} is not valid ({e}); check the staging directories it lists, then remove it",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(Error::io(format!("reading {}", path.display()))(e)),
    }
}

fn save_registry(state_dir: &Path, paths: &[PathBuf]) -> Result<()> {
    write_json_atomic(&state_dir.join(STAGING_REGISTRY), &paths)
}

fn unregister(state_dir: &Path, path: &Path) {
    let mut paths = match load_registry(state_dir) {
        Ok(paths) => paths,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the restore staging registry; leaving it as it is");
            return;
        }
    };
    paths.retain(|p| p != path);
    if let Err(e) = save_registry(state_dir, &paths) {
        tracing::warn!(error = %e, "could not update the restore staging registry");
    }
}

/// Removes registered staging directories that are still ours. An entry leaves the registry only once its
/// directory is confirmed gone: removed here, or already absent (`NotFound`). Any other outcome (an error
/// looking it up, something that isn't ours at that path, a failed removal) keeps it registered, so a later
/// run looks again.
fn clean_registered_staging(state_dir: &Path) {
    let paths = match load_registry(state_dir) {
        Ok(paths) => paths,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the restore staging registry; not cleaning staging");
            return;
        }
    };
    if paths.is_empty() {
        return;
    }
    let mut keep = Vec::new();
    for path in paths {
        let named = path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(STAGING_PREFIX));
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, // confirmed gone
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not check registered staging; keeping it registered");
                keep.push(path);
                continue;
            }
        }
        if !(named && is_marked_dir(&path)) {
            tracing::warn!(path = %path.display(), "registered staging path is not ours any more; leaving it and keeping it registered");
            keep.push(path);
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "removed leftover restore staging directory")
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not remove leftover restore staging");
                keep.push(path);
            }
        }
    }
    if let Err(e) = save_registry(state_dir, &keep) {
        tracing::warn!(error = %e, "could not update the restore staging registry");
    }
}

/// An owner-only, marked staging directory next to a restore destination. Registered in the state directory
/// before it is created; removed and unregistered when dropped.
#[derive(Debug)]
pub(crate) struct Staging {
    path: PathBuf,
    state_dir: PathBuf,
}

impl Staging {
    pub(crate) fn begin(state_dir: &Path, dest_dir: &Path) -> Result<Self> {
        let path = std::path::absolute(dest_dir.join(unique_name(STAGING_PREFIX)))
            .map_err(Error::io(format!("resolving {}", dest_dir.display())))?;
        let mut paths = load_registry(state_dir)?;
        paths.push(path.clone());
        save_registry(state_dir, &paths)?;
        if let Err(e) = create_marked(&path) {
            unregister(state_dir, &path);
            return Err(e);
        }
        Ok(Self {
            path,
            state_dir: state_dir.to_path_buf(),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => unregister(&self.state_dir, &self.path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                unregister(&self.state_dir, &self.path)
            }
            // Stays registered, so the next restore or drill retries.
            Err(e) => {
                tracing::warn!(path = %self.path.display(), error = %e, "could not remove restore staging")
            }
        }
    }
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

    fn assert_private_dir(path: &Path) {
        secure::verify_private(path, secure::Private::Dir).expect("owner-only directory");
    }

    #[test]
    fn new_state_dir_and_scratch_are_owner_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("a").join("state");
        let _lock = StateLock::acquire(&state).expect("lock");
        assert_private_dir(&state);
        let run = scratch_dir(&state).expect("scratch");
        assert_private_dir(&state.join(SCRATCH_DIR));
        assert_private_dir(run.path());
        assert!(is_marked_dir(run.path()));
        let path = run.path().to_path_buf();
        drop(run);
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_removes_only_marked_scratch() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let _lock = StateLock::acquire(&state).expect("lock");

        // A killed run's scratch directory, with plaintext in it.
        let leftover = scratch_dir(&state).expect("scratch");
        std::fs::write(leftover.path().join("snapshot.sqlite3"), b"plain").expect("write");
        let leftover_path = leftover.path().to_path_buf();
        std::mem::forget(leftover);

        // Things that are not ours: in the scratch root, and next to it with the old prefix.
        let root = state.join(SCRATCH_DIR);
        let foreign = root.join("someone-elses");
        std::fs::create_dir(&foreign).expect("mkdir");
        std::fs::write(foreign.join("data"), b"keep").expect("write");
        let fake_marker = root.join("fake");
        std::fs::create_dir(&fake_marker).expect("mkdir");
        std::fs::write(fake_marker.join(MARKER_FILE), b"not our text").expect("write");
        std::fs::write(root.join("loose-file"), b"keep").expect("write");
        let beside = state.join(".todo-backup-old");
        std::fs::create_dir(&beside).expect("mkdir");
        std::fs::write(beside.join("data"), b"keep").expect("write");

        clean_leftovers(&state);
        assert!(!leftover_path.exists());
        assert_eq!(std::fs::read(foreign.join("data")).expect("read"), b"keep");
        assert!(fake_marker.join(MARKER_FILE).exists());
        assert!(root.join("loose-file").exists());
        assert_eq!(std::fs::read(beside.join("data")).expect("read"), b"keep");
    }

    #[test]
    fn scratch_root_must_be_a_plain_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        std::fs::create_dir(&state).expect("mkdir");
        std::fs::write(state.join(SCRATCH_DIR), b"a file").expect("write");
        assert!(scratch_dir(&state).is_err());
    }

    #[test]
    fn killed_restore_staging_is_cleaned_next_time() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).expect("mkdir");
        let _lock = StateLock::acquire(&state).expect("lock");

        // A restore that dies mid-way: its staging directory (with plaintext) stays behind.
        let staging = Staging::begin(&state, &dest).expect("staging");
        assert_private_dir(staging.path());
        std::fs::write(staging.path().join("restore.sqlite3"), b"plain").expect("write");
        let staged = staging.path().to_path_buf();
        std::mem::forget(staging);
        assert_eq!(
            load_registry(&state).expect("registry"),
            std::slice::from_ref(&staged)
        );

        // A registered path that has since been replaced by something that isn't ours is left alone.
        let impostor =
            std::path::absolute(dest.join(format!("{STAGING_PREFIX}impostor"))).expect("abs");
        std::fs::create_dir(&impostor).expect("mkdir");
        std::fs::write(impostor.join("data"), b"keep").expect("write");
        let mut reg = load_registry(&state).expect("registry");
        reg.push(impostor.clone());
        save_registry(&state, &reg).expect("save");

        clean_leftovers(&state);
        assert!(!staged.exists());
        assert_eq!(std::fs::read(impostor.join("data")).expect("read"), b"keep");
        // Not removed, so it stays registered; once it's confirmed gone, it's dropped.
        assert_eq!(
            load_registry(&state).expect("registry"),
            std::slice::from_ref(&impostor)
        );
        std::fs::remove_dir_all(&impostor).expect("rm");
        clean_leftovers(&state);
        assert!(load_registry(&state).expect("registry").is_empty());

        // A normal staging directory unregisters itself when dropped.
        let s = Staging::begin(&state, &dest).expect("staging");
        let p = s.path().to_path_buf();
        drop(s);
        assert!(!p.exists());
        assert!(load_registry(&state).expect("registry").is_empty());
    }

    #[test]
    fn staging_entries_survive_lookup_errors_and_a_corrupt_registry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let _lock = StateLock::acquire(&state).expect("lock");

        // A path whose lookup fails with something other than NotFound.
        #[cfg(windows)]
        let unreadable =
            std::path::absolute(tmp.path().join(format!("{STAGING_PREFIX}bad<name"))).expect("abs");
        #[cfg(unix)]
        let unreadable = {
            let file = tmp.path().join("a-file");
            std::fs::write(&file, b"x").expect("write");
            file.join(format!("{STAGING_PREFIX}under-a-file"))
        };
        let err = std::fs::symlink_metadata(&unreadable).expect_err("lookup fails");
        assert_ne!(err.kind(), std::io::ErrorKind::NotFound, "{err}");
        let gone =
            std::path::absolute(tmp.path().join(format!("{STAGING_PREFIX}gone"))).expect("abs");
        save_registry(&state, &[unreadable.clone(), gone]).expect("save");
        clean_leftovers(&state);
        assert_eq!(
            load_registry(&state).expect("registry"),
            std::slice::from_ref(&unreadable)
        );

        // A registry that can't be parsed is neither rewritten by cleanup nor replaced by a new restore.
        let registry = state.join(STAGING_REGISTRY);
        std::fs::write(&registry, b"[\"half").expect("write");
        clean_leftovers(&state);
        let dest = tmp.path().join("dest");
        std::fs::create_dir(&dest).expect("mkdir");
        assert!(matches!(
            Staging::begin(&state, &dest),
            Err(Error::Config(_))
        ));
        assert_eq!(std::fs::read(&registry).expect("read"), b"[\"half");
        assert_eq!(std::fs::read_dir(&dest).expect("ls").count(), 0);
    }

    /// Points `link` at the directory `target`: a junction on Windows, a symlink on Unix.
    fn redirect(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .stdout(std::process::Stdio::null())
                .status()
                .expect("mklink");
            assert!(status.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("symlink");
    }

    #[test]
    fn cleanup_never_follows_a_redirected_scratch_root_or_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = tmp.path().join("state");
        let _lock = StateLock::acquire(&state).expect("lock");
        // Somewhere else, a directory that looks exactly like a leftover run.
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).expect("mkdir");
        let victim = elsewhere.join("run-victim");
        create_marked(&victim).expect("marked");
        std::fs::write(victim.join("data"), b"keep").expect("write");

        // The scratch root itself redirected there.
        redirect(&state.join(SCRATCH_DIR), &elsewhere);
        clean_leftovers(&state);
        assert_eq!(std::fs::read(victim.join("data")).expect("read"), b"keep");
        assert!(scratch_dir(&state).is_err());

        // A real scratch root with an entry redirected there.
        // Removes the link only: a junction is an (empty) directory entry, a symlink a file entry.
        #[cfg(windows)]
        std::fs::remove_dir(state.join(SCRATCH_DIR)).expect("unlink");
        #[cfg(unix)]
        std::fs::remove_file(state.join(SCRATCH_DIR)).expect("unlink");
        assert!(victim.join("data").exists());
        let root = scratch_root(&state).expect("root");
        redirect(&root.join("run-link"), &victim);
        assert!(!is_marked_dir(&root.join("run-link")));
        clean_leftovers(&state);
        assert_eq!(std::fs::read(victim.join("data")).expect("read"), b"keep");
        assert!(root.join("run-link").exists());
    }

    #[test]
    fn json_writes_replace_and_leave_no_temp_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("x.json");
        write_json_atomic(&path, &1).expect("first");
        write_json_atomic(&path, &2).expect("second");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "2");
        assert_eq!(std::fs::read_dir(tmp.path()).expect("ls").count(), 1);
    }
}
