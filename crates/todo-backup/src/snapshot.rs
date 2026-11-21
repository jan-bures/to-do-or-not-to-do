//! Consistent snapshots of the live database, and checks on a database file.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// What a snapshot or restored database contains, as far as the drill cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbInfo {
    /// `max(events.seq)`; `None` without an `events` table or when it is empty.
    pub max_seq: Option<i64>,
    /// `count(*)` of `events`; `None` without an `events` table.
    pub events_count: Option<u64>,
}

/// Writes a transactionally consistent copy of `db` to `dest`, which must not exist.
///
/// `VACUUM INTO` reads inside one read transaction. In WAL mode that never blocks the server's writers, and
/// the copy reflects exactly one committed state. The copy is switched to rollback-journal mode so it is a
/// single self-contained file.
pub fn take(db: &Path, dest: &Path) -> Result<()> {
    if !db.is_file() {
        return Err(Error::Config(format!(
            "database {} does not exist",
            db.display()
        )));
    }
    // Read-write without CREATE: a missing database is an error, not a new empty file, and a WAL database can
    // be read even when its -shm file has to be created. Nothing is written to it.
    let src = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    src.busy_timeout(Duration::from_secs(30))?;
    let dest_str = dest
        .to_str()
        .ok_or_else(|| Error::Config(format!("snapshot path {} is not UTF-8", dest.display())))?;
    src.execute("VACUUM INTO ?1", [dest_str])?;
    drop(src);

    let snap = Connection::open_with_flags(dest, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let mode: String = snap.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(Error::Integrity {
            path: dest.to_path_buf(),
            detail: format!("could not switch the snapshot to rollback-journal mode (got {mode})"),
        });
    }
    Ok(())
}

/// Runs `PRAGMA integrity_check` and reads the event counters. Opens the file read-only.
pub fn inspect(path: &Path) -> Result<DbInfo> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| Error::Integrity {
        path: path.to_path_buf(),
        detail: format!("cannot open as SQLite: {e}"),
    })?;
    integrity_check(&conn, path)?;

    let has_events: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'events'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_events {
        return Ok(DbInfo {
            max_seq: None,
            events_count: None,
        });
    }
    let (count, max_seq): (i64, Option<i64>) =
        conn.query_row("SELECT count(*), max(seq) FROM events", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    Ok(DbInfo {
        max_seq,
        events_count: Some(u64::try_from(count).unwrap_or(0)),
    })
}

fn integrity_check(conn: &Connection, path: &Path) -> Result<()> {
    let integrity = |detail: String| Error::Integrity {
        path: path.to_path_buf(),
        detail,
    };
    let mut stmt = conn
        .prepare("PRAGMA integrity_check")
        .map_err(|e| integrity(e.to_string()))?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .and_then(|rows| rows.collect())
        .map_err(|e| integrity(e.to_string()))?;
    if rows.len() == 1 && rows[0] == "ok" {
        Ok(())
    } else {
        // The messages name pages and indexes, not row contents. Keep only the first few.
        let mut detail = rows.iter().take(5).cloned().collect::<Vec<_>>().join("; ");
        if rows.len() > 5 {
            detail.push_str(&format!("; ... {} more", rows.len() - 5));
        }
        Err(integrity(detail))
    }
}

/// SHA-256 of a file, as lowercase hex, and its length.
pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let ctx = || format!("hashing {}", path.display());
    let mut file = std::fs::File::open(path).map_err(Error::io(ctx()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut len = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(Error::io(ctx()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    Ok((to_hex(&hasher.finalize()), len))
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
