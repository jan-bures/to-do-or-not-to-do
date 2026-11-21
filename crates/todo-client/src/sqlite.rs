//! A bundled-SQLite [`Storage`] (feature `sqlite`).

use crate::outbox::{CommandId, FailedEntry, OutboxEntry};
use crate::storage::{Loaded, Storage, StorageError, StorageOp};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, params};
use std::fmt;
use std::path::Path;

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS snapshot (
    id     INTEGER PRIMARY KEY CHECK (id = 1),
    cursor INTEGER NOT NULL,
    state  TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY, event TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS outbox (
    pos        INTEGER PRIMARY KEY AUTOINCREMENT,
    command_id TEXT    NOT NULL UNIQUE,
    entry      TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS failed (
    pos        INTEGER PRIMARY KEY AUTOINCREMENT,
    command_id TEXT    NOT NULL UNIQUE,
    entry      TEXT    NOT NULL
);
";

/// A damaged or foreign file is [`StorageError::Corrupt`]; anything else (locked, out of space, no
/// permission) is [`StorageError::Backend`] and may pass.
fn backend(e: rusqlite::Error) -> StorageError {
    match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => {
            StorageError::Corrupt(e.to_string())
        }
        _ => StorageError::Backend(e.to_string()),
    }
}

/// What [`SqliteStorage::salvage`] could read from a damaged database.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Salvaged {
    /// Outbox entries that still decode, oldest first.
    pub outbox: Vec<OutboxEntry>,
    /// "Couldn't sync" entries that still decode, oldest first.
    pub failed: Vec<FailedEntry>,
    /// Rows that were there but could not be decoded.
    pub lost: usize,
}

/// [`Storage`] in one SQLite file. Every [`write`](Storage::write) is one transaction.
pub struct SqliteStorage {
    conn: Connection,
}

impl fmt::Debug for SqliteStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqliteStorage").finish_non_exhaustive()
    }
}

impl SqliteStorage {
    /// Opens (or creates) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        Self::init(Connection::open(path).map_err(backend)?)
    }

    /// A private in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::init(Connection::open_in_memory().map_err(backend)?)
    }

    /// Reads what can still be read of the unsent changes in a database that failed to open or load:
    /// the outbox and the "couldn't sync" list, row by row, skipping rows that don't decode. Opens the
    /// file read-only and changes nothing, so it can run on a quarantined copy. The confirmed state is
    /// not salvaged; the server has it.
    ///
    /// # Errors
    ///
    /// The file can't be opened, or neither table can be read.
    pub fn salvage(path: &Path) -> Result<Salvaged, StorageError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(backend)?;
        let mut out = Salvaged::default();
        let outbox = salvage_rows(
            &conn,
            "SELECT entry FROM outbox ORDER BY pos",
            &mut out.lost,
        );
        let failed = salvage_rows(
            &conn,
            "SELECT entry FROM failed ORDER BY pos",
            &mut out.lost,
        );
        match (outbox, failed) {
            (Err(e), Err(_)) => Err(e),
            (outbox, failed) => {
                out.outbox = outbox.unwrap_or_default();
                out.failed = failed.unwrap_or_default();
                Ok(out)
            }
        }
    }

    fn init(conn: Connection) -> Result<Self, StorageError> {
        // WAL with full sync: a submitted command survives a power cut once `submit` returned.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(backend)?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(backend)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(backend)?;
        if version > SCHEMA_VERSION {
            return Err(StorageError::Corrupt(format!(
                "the database has schema version {version}, newer than this client's {SCHEMA_VERSION}"
            )));
        }
        conn.execute_batch(SCHEMA).map_err(backend)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(backend)?;
        Ok(Self { conn })
    }
}

fn salvage_rows<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    lost: &mut usize,
) -> Result<Vec<T>, StorageError> {
    let mut stmt = conn.prepare(sql).map_err(backend)?;
    let mut rows = stmt.query([]).map_err(backend)?;
    let mut out = Vec::new();
    loop {
        match rows.next() {
            Ok(Some(row)) => match row
                .get::<_, String>(0)
                .ok()
                .and_then(|r| serde_json::from_str(&r).ok())
            {
                Some(entry) => out.push(entry),
                None => *lost += 1,
            },
            Ok(None) => break,
            // A damaged page ends the scan; keep what came before it.
            Err(_) => {
                *lost += 1;
                break;
            }
        }
    }
    Ok(out)
}

fn rows(conn: &Connection, sql: &str) -> Result<Vec<String>, StorageError> {
    let mut stmt = conn.prepare(sql).map_err(backend)?;
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map_err(backend)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(backend)
}

fn decode<T: serde::de::DeserializeOwned>(rows: Vec<String>) -> Result<Vec<T>, StorageError> {
    rows.iter()
        .map(|r| serde_json::from_str(r).map_err(StorageError::from))
        .collect()
}

fn key(id: CommandId) -> String {
    id.to_string()
}

impl Storage for SqliteStorage {
    fn load(&mut self) -> Result<Loaded, StorageError> {
        let snapshot = self
            .conn
            .query_row("SELECT state, cursor FROM snapshot WHERE id = 1", [], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .optional()
            .map_err(backend)?
            .map(|(state, cursor)| {
                let cursor = u64::try_from(cursor)
                    .map_err(|_| StorageError::Corrupt("negative cursor".into()))?;
                Ok::<_, StorageError>((serde_json::from_str(&state)?, cursor))
            })
            .transpose()?;
        let epoch = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'epoch'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(backend)?
            .map(|e| {
                e.parse()
                    .map_err(|_| StorageError::Corrupt("bad epoch".into()))
            })
            .transpose()?;
        Ok(Loaded {
            snapshot,
            epoch,
            events: decode(rows(&self.conn, "SELECT event FROM events ORDER BY seq")?)?,
            outbox: decode(rows(&self.conn, "SELECT entry FROM outbox ORDER BY pos")?)?,
            failed: decode(rows(&self.conn, "SELECT entry FROM failed ORDER BY pos")?)?,
        })
    }

    fn write(&mut self, ops: &[StorageOp<'_>]) -> Result<(), StorageError> {
        let tx = self.conn.transaction().map_err(backend)?;
        for op in ops {
            match op {
                StorageOp::AppendEvents(events) => {
                    let mut stmt = tx
                        .prepare_cached("INSERT INTO events (seq, event) VALUES (?1, ?2)")
                        .map_err(backend)?;
                    for e in *events {
                        let seq = e
                            .event
                            .seq
                            .and_then(|s| i64::try_from(s).ok())
                            .ok_or_else(|| StorageError::Backend("event without seq".into()))?;
                        stmt.execute(params![seq, serde_json::to_string(e)?])
                            .map_err(backend)?;
                    }
                }
                StorageOp::Snapshot { state, cursor } => {
                    let cursor = i64::try_from(*cursor)
                        .map_err(|_| StorageError::Backend("cursor out of range".into()))?;
                    tx.execute(
                        "INSERT INTO snapshot (id, cursor, state) VALUES (1, ?1, ?2)
                         ON CONFLICT (id) DO UPDATE SET cursor = excluded.cursor, state = excluded.state",
                        params![cursor, serde_json::to_string(state)?],
                    )
                    .map_err(backend)?;
                    tx.execute("DELETE FROM events", []).map_err(backend)?;
                }
                StorageOp::ClearConfirmed => {
                    tx.execute_batch(
                        "DELETE FROM snapshot; DELETE FROM events; DELETE FROM meta WHERE key = 'epoch';",
                    )
                    .map_err(backend)?;
                }
                StorageOp::SetEpoch(e) => {
                    tx.execute(
                        "INSERT INTO meta (key, value) VALUES ('epoch', ?1)
                         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                        params![e.to_string()],
                    )
                    .map_err(backend)?;
                }
                StorageOp::PutOutbox(e) => {
                    tx.execute(
                        "INSERT INTO outbox (command_id, entry) VALUES (?1, ?2)
                         ON CONFLICT (command_id) DO UPDATE SET entry = excluded.entry",
                        params![key(e.command_id), serde_json::to_string(e)?],
                    )
                    .map_err(backend)?;
                }
                StorageOp::RemoveOutbox(id) => {
                    tx.execute(
                        "DELETE FROM outbox WHERE command_id = ?1",
                        params![key(*id)],
                    )
                    .map_err(backend)?;
                }
                StorageOp::PutFailed(f) => {
                    tx.execute(
                        "INSERT INTO failed (command_id, entry) VALUES (?1, ?2)
                         ON CONFLICT (command_id) DO UPDATE SET entry = excluded.entry",
                        params![key(f.entry.command_id), serde_json::to_string(f)?],
                    )
                    .map_err(backend)?;
                }
                StorageOp::RemoveFailed(id) => {
                    tx.execute(
                        "DELETE FROM failed WHERE command_id = ?1",
                        params![key(*id)],
                    )
                    .map_err(backend)?;
                }
            }
        }
        tx.commit().map_err(backend)
    }
}
