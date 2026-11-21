//! The commands: `run`, `restore`, `drill` and `status`.

use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use age::x25519::Identity;
use jiff::Timestamp;

use crate::budget::{Budget, OpClass};
use crate::config::{Config, UploadOptions};
use crate::snapshot::{self, DbInfo};
use crate::state::{self, State, StateLock, UploadRecord};
use crate::store::{Metered, ObjectStore};
use crate::{Error, Kind, Result, crypto};

/// Every object key ends with this.
pub const KEY_SUFFIX: &str = ".sqlite3.zst.age";
/// A listing never reads more than this many pages (1000 keys each).
const MAX_LIST_PAGES: usize = 10;
/// S3's limit on parts per multipart upload.
const MAX_PARTS: u64 = 10_000;

/// `<kind>/<UTC timestamp>-<seq>.sqlite3.zst.age`. Keys of one kind sort chronologically.
pub fn object_key(kind: Kind, now: Timestamp, max_seq: Option<i64>) -> String {
    format!(
        "{}{}-{}{KEY_SUFFIX}",
        kind.prefix(),
        now.strftime("%Y%m%dT%H%M%SZ"),
        max_seq.unwrap_or(0)
    )
}

/// The `seq` part of a key made by [`object_key`].
pub fn key_seq(key: &str) -> Option<i64> {
    let name = key.rsplit('/').next()?.strip_suffix(KEY_SUFFIX)?;
    name.rsplit_once('-')?.1.parse().ok()
}

/// The timestamp-and-seq part of a key, which orders keys across kinds.
fn key_name(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

// ---------------------------------------------------------------------------------------------------------
// run

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Uploaded(UploadRecord),
    /// Nothing changed since this upload of the same kind, so nothing was sent.
    Unchanged(UploadRecord),
}

/// Takes a snapshot, checks it, and uploads it unless it is identical to the last upload of `kind`.
pub fn run(
    cfg: &Config,
    store: &dyn ObjectStore,
    kind: Kind,
    force: bool,
    now: Timestamp,
) -> Result<RunOutcome> {
    let recipient = cfg.recipient()?;
    let _lock = StateLock::acquire(&cfg.state_dir)?;
    state::remove_stale_scratch(&cfg.state_dir);
    let mut budget = Budget::open(&cfg.state_dir, cfg.budget, now)?;
    // Refuse up front, before doing any work, if the month's budget cannot fit even one upload.
    budget.check(OpClass::A, 1)?;

    let scratch = state::scratch_dir(&cfg.state_dir)?;
    let snap = scratch.path().join("snapshot.sqlite3");
    snapshot::take(&cfg.db_path, &snap)?;
    let info = snapshot::inspect(&snap)?;
    let (sha256, snapshot_bytes) = snapshot::sha256_file(&snap)?;
    tracing::info!(%kind, snapshot_bytes, max_seq = ?info.max_seq, events = ?info.events_count, "snapshot ok");

    let mut st = State::load(&cfg.state_dir)?;
    if let Some(prev) = st.get(kind)
        && !force
        && prev.max_seq == info.max_seq
        && prev.snapshot_sha256 == sha256
    {
        tracing::info!(%kind, last = %prev.key, "unchanged since the last backup; skipping the upload");
        return Ok(RunOutcome::Unchanged(prev.clone()));
    }

    let sealed = scratch.path().join("snapshot.sqlite3.zst.age");
    let object_bytes = crypto::seal(&snap, &sealed, &recipient, cfg.upload.zstd_level)?;
    let key = object_key(kind, now, info.max_seq);
    let mut metered = Metered::new(store, &mut budget);
    upload(&mut metered, &key, &sealed, &cfg.upload)?;
    tracing::info!(%kind, %key, object_bytes, "uploaded");

    let record = UploadRecord {
        key,
        max_seq: info.max_seq,
        events_count: info.events_count,
        snapshot_sha256: sha256,
        snapshot_bytes,
        object_bytes,
        uploaded_at: now.strftime("%Y-%m-%dT%H:%M:%SZ").to_string(),
    };
    st.set(kind, record.clone());
    st.save(&cfg.state_dir)?;
    Ok(RunOutcome::Uploaded(record))
}

/// One PUT up to the multipart threshold; multipart above it.
fn upload<S: ObjectStore + ?Sized>(
    metered: &mut Metered<'_, S>,
    key: &str,
    path: &Path,
    opts: &UploadOptions,
) -> Result<()> {
    let ctx = || format!("reading {}", path.display());
    let len = std::fs::metadata(path).map_err(Error::io(ctx()))?.len();
    if len <= opts.multipart_threshold_bytes {
        let body = std::fs::read(path).map_err(Error::io(ctx()))?;
        return metered.put(key, &body);
    }

    let parts = len.div_ceil(opts.part_size_bytes);
    if parts > MAX_PARTS {
        return Err(Error::Config(format!(
            "{len} bytes need {parts} parts; raise upload.part_size_bytes"
        )));
    }
    // Never start an upload the budget cannot finish: create + parts + complete.
    metered.check(OpClass::A, parts + 2)?;
    let upload_id = metered.create_multipart(key)?;
    match upload_parts(metered, key, &upload_id, path, parts, opts.part_size_bytes) {
        Ok(()) => Ok(()),
        Err(e) => {
            if let Err(abort) = metered.abort_multipart(key, &upload_id) {
                tracing::warn!(error = %abort, "could not abort the multipart upload; the bucket's lifecycle rule removes it after a day");
            }
            Err(e)
        }
    }
}

fn upload_parts<S: ObjectStore + ?Sized>(
    metered: &mut Metered<'_, S>,
    key: &str,
    upload_id: &str,
    path: &Path,
    parts: u64,
    part_size: u64,
) -> Result<()> {
    let ctx = || format!("reading {}", path.display());
    let mut file = File::open(path).map_err(Error::io(ctx()))?;
    let part_size = usize::try_from(part_size)
        .map_err(|_| Error::Config("upload.part_size_bytes is too large".into()))?;
    let mut buf = vec![0u8; part_size];
    let mut etags = Vec::new();
    for n in 1..=parts {
        let mut filled = 0;
        while filled < part_size {
            let r = file.read(&mut buf[filled..]).map_err(Error::io(ctx()))?;
            if r == 0 {
                break;
            }
            filled += r;
        }
        let part_number =
            u16::try_from(n).map_err(|_| Error::Config("too many multipart parts".into()))?;
        etags.push(metered.upload_part(key, upload_id, part_number, &buf[..filled])?);
        tracing::debug!(part_number, bytes = filled, "uploaded part");
    }
    metered.complete_multipart(key, upload_id, &etags)
}

// ---------------------------------------------------------------------------------------------------------
// restore

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Key(String),
    /// The newest object of a kind. Costs one LIST (a Class A operation).
    Latest(Kind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    pub key: String,
    pub info: DbInfo,
    pub sha256: String,
    pub bytes: u64,
}

/// Downloads, decrypts and decompresses a backup into `out`, after checking its integrity. Never overwrites.
pub fn restore(
    cfg: &Config,
    store: &dyn ObjectStore,
    target: &Target,
    identities: &[Identity],
    out: &Path,
    now: Timestamp,
) -> Result<Restored> {
    // Check before spending any operation; the final rename re-checks atomically.
    if out.exists() {
        return Err(Error::OutputExists(out.to_path_buf()));
    }
    let _lock = StateLock::acquire(&cfg.state_dir)?;
    state::remove_stale_scratch(&cfg.state_dir);
    let mut budget = Budget::open(&cfg.state_dir, cfg.budget, now)?;
    let mut metered = Metered::new(store, &mut budget);
    let key = match target {
        Target::Key(k) => k.clone(),
        Target::Latest(kind) => latest_key(&mut metered, &[*kind])?,
    };
    let scratch = state::scratch_dir(&cfg.state_dir)?;
    restore_key(&mut metered, scratch.path(), &key, identities, out)
}

fn restore_key<S: ObjectStore + ?Sized>(
    metered: &mut Metered<'_, S>,
    scratch: &Path,
    key: &str,
    identities: &[Identity],
    out: &Path,
) -> Result<Restored> {
    if out.exists() {
        return Err(Error::OutputExists(out.to_path_buf()));
    }
    let encrypted = scratch.join("download.age");
    {
        let ctx = || format!("writing {}", encrypted.display());
        let mut w = BufWriter::new(File::create_new(&encrypted).map_err(Error::io(ctx()))?);
        metered.get(key, &mut w)?;
        w.flush().map_err(Error::io(ctx()))?;
    }

    let out_dir = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut tmp = tempfile::Builder::new()
        .prefix(".todo-restore-")
        .tempfile_in(out_dir)
        .map_err(Error::io(format!(
            "creating a temporary file in {}",
            out_dir.display()
        )))?;
    let bytes = crypto::open(&encrypted, tmp.as_file_mut(), identities)?;
    // Close our handle so SQLite (and Windows) can open and rename the file.
    let tmp = tmp.into_temp_path();
    let info = snapshot::inspect(&tmp)?;
    let (sha256, _) = snapshot::sha256_file(&tmp)?;
    tmp.persist_noclobber(out).map_err(|e| {
        if out.exists() {
            Error::OutputExists(out.to_path_buf())
        } else {
            Error::io(format!("moving the restored database to {}", out.display()))(e.error)
        }
    })?;
    tracing::info!(%key, bytes, max_seq = ?info.max_seq, "restored");
    Ok(Restored {
        key: key.to_owned(),
        info,
        sha256,
        bytes,
    })
}

/// The newest backup key among `kinds`, by listing. Each page is one Class A operation.
fn latest_key<S: ObjectStore + ?Sized>(
    metered: &mut Metered<'_, S>,
    kinds: &[Kind],
) -> Result<String> {
    let mut best: Option<String> = None;
    for kind in kinds {
        let mut token: Option<String> = None;
        for _ in 0..MAX_LIST_PAGES {
            let page = metered.list_page(kind.prefix(), token.as_deref())?;
            for k in page.keys.into_iter().filter(|k| k.ends_with(KEY_SUFFIX)) {
                if best.as_deref().is_none_or(|b| key_name(&k) > key_name(b)) {
                    best = Some(k);
                }
            }
            token = page.next;
            if token.is_none() {
                break;
            }
        }
    }
    best.ok_or_else(|| {
        let names: Vec<&str> = kinds.iter().map(|k| k.prefix()).collect();
        Error::NoBackup(format!("no objects under {}", names.join(", ")))
    })
}

// ---------------------------------------------------------------------------------------------------------
// drill

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrillReport {
    pub key: String,
    /// Whether the expectations came from the local record of the upload (or only from the key).
    pub recorded: bool,
    pub restored: Restored,
    pub checks: Vec<Check>,
}

impl DrillReport {
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
}

impl fmt::Display for DrillReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "restore drill: {}",
            if self.passed() { "PASSED" } else { "FAILED" }
        )?;
        writeln!(f, "  object:   {}", self.key)?;
        writeln!(f, "  restored: {} bytes", self.restored.bytes)?;
        let opt = |v: Option<String>| v.unwrap_or_else(|| "-".into());
        writeln!(
            f,
            "  events:   {} rows, max(seq) {}",
            opt(self.restored.info.events_count.map(|c| c.to_string())),
            opt(self.restored.info.max_seq.map(|s| s.to_string()))
        )?;
        if !self.recorded {
            writeln!(
                f,
                "  note:     no local record of this upload; checked against the key only"
            )?;
        }
        for c in &self.checks {
            writeln!(
                f,
                "  [{}] {}: {}",
                if c.ok { " ok " } else { "FAIL" },
                c.name,
                c.detail
            )?;
        }
        Ok(())
    }
}

/// Restores the latest backup (of `kind`, or of either kind) into a scratch directory and checks it against
/// what was recorded when it was uploaded. Uses the local record when there is one, so it costs one GET;
/// otherwise one LIST per kind as well.
///
/// A corrupt, truncated or undecryptable object, or a failed integrity check, is an `Err`. Mismatched
/// contents give a report whose [`DrillReport::passed`] is false.
pub fn drill(
    cfg: &Config,
    store: &dyn ObjectStore,
    identities: &[Identity],
    kind: Option<Kind>,
    now: Timestamp,
) -> Result<DrillReport> {
    let _lock = StateLock::acquire(&cfg.state_dir)?;
    state::remove_stale_scratch(&cfg.state_dir);
    let st = State::load(&cfg.state_dir)?;
    let record = match kind {
        Some(k) => st.get(k),
        None => st.latest(),
    }
    .cloned();

    let mut budget = Budget::open(&cfg.state_dir, cfg.budget, now)?;
    let mut metered = Metered::new(store, &mut budget);
    let key = match &record {
        Some(r) => r.key.clone(),
        None => {
            let kinds: Vec<Kind> = kind.map_or_else(|| Kind::ALL.to_vec(), |k| vec![k]);
            latest_key(&mut metered, &kinds)?
        }
    };

    let scratch = state::scratch_dir(&cfg.state_dir)?;
    let out = scratch.path().join("drill.sqlite3");
    let restored = restore_key(&mut metered, scratch.path(), &key, identities, &out)?;
    let info = restored.info;

    let mut checks = vec![Check {
        name: "integrity_check",
        ok: true,
        detail: "ok".into(),
    }];
    if let Some(r) = &record {
        checks.push(eq_check(
            "sha256 matches the uploaded snapshot",
            &r.snapshot_sha256,
            &restored.sha256,
        ));
        checks.push(eq_check(
            "max(seq) matches the record",
            &r.max_seq,
            &info.max_seq,
        ));
        checks.push(eq_check(
            "event count matches the record",
            &r.events_count,
            &info.events_count,
        ));
    }
    checks.push(eq_check(
        "max(seq) matches the key",
        &key_seq(&key),
        &Some(info.max_seq.unwrap_or(0)),
    ));
    checks.push(sanity_check(info));

    Ok(DrillReport {
        key,
        recorded: record.is_some(),
        restored,
        checks,
    })
}

fn eq_check<T: PartialEq + fmt::Debug>(name: &'static str, expected: &T, actual: &T) -> Check {
    Check {
        name,
        ok: expected == actual,
        detail: if expected == actual {
            format!("{actual:?}")
        } else {
            format!("expected {expected:?}, got {actual:?}")
        },
    }
}

/// Without a record, at least the counters must make sense: seq numbers start at 1 and only grow, so there
/// can't be more events than the highest seq.
fn sanity_check(info: DbInfo) -> Check {
    let (ok, detail) = match (info.events_count, info.max_seq) {
        (None, None) => (true, "no events table".to_string()),
        (Some(0), None) => (true, "events table is empty".to_string()),
        (Some(c), Some(m)) if m >= 1 && c >= 1 && c <= m.unsigned_abs() => {
            (true, format!("{c} events, max(seq) {m}"))
        }
        (c, m) => (false, format!("implausible: {c:?} events, max(seq) {m:?}")),
    };
    Check {
        name: "event counters are sane",
        ok,
        detail,
    }
}

// ---------------------------------------------------------------------------------------------------------
// status

/// Budget usage and the last upload of each kind. Performs no S3 operations.
pub fn status(cfg: &Config, now: Timestamp) -> Result<String> {
    let budget = Budget::open(&cfg.state_dir, cfg.budget, now)?;
    let st = State::load(&cfg.state_dir)?;
    let u = budget.usage();
    let caps = budget.caps();
    let mut s = format!(
        "budget {}: Class A {}/{}, Class B {}/{}\n",
        u.month, u.class_a, caps.class_a_monthly, u.class_b, caps.class_b_monthly
    );
    for kind in Kind::ALL {
        match st.get(kind) {
            Some(r) => s.push_str(&format!(
                "{kind}: {} ({} bytes, max(seq) {:?}) at {}\n",
                r.key, r.object_bytes, r.max_seq, r.uploaded_at
            )),
            None => s.push_str(&format!("{kind}: no upload recorded\n")),
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_sort() {
        let t1: Timestamp = "2026-11-21T06:00:00Z".parse().expect("ts");
        let t2: Timestamp = "2026-11-21T12:00:00Z".parse().expect("ts");
        let a = object_key(Kind::Frequent, t1, Some(42));
        assert_eq!(a, "frequent/20261121T060000Z-42.sqlite3.zst.age");
        assert_eq!(key_seq(&a), Some(42));
        assert_eq!(key_seq(&object_key(Kind::Daily, t1, None)), Some(0));
        assert!(object_key(Kind::Frequent, t2, Some(1)) > a);
        assert_eq!(key_seq("frequent/garbage"), None);
    }

    #[test]
    fn sanity() {
        let i = |c, m| DbInfo {
            events_count: c,
            max_seq: m,
        };
        assert!(sanity_check(i(None, None)).ok);
        assert!(sanity_check(i(Some(0), None)).ok);
        assert!(sanity_check(i(Some(5), Some(7))).ok);
        assert!(!sanity_check(i(Some(8), Some(7))).ok);
        assert!(!sanity_check(i(Some(0), Some(7))).ok);
        assert!(!sanity_check(i(None, Some(1))).ok);
    }
}
