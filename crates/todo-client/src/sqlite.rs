//! A bundled-SQLite [`Storage`] (feature `sqlite`).

use crate::outbox::CommandId;
use crate::storage::{Loaded, Storage, StorageError, StorageOp};
use rusqlite::{Connection, OptionalExtension, params};
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

fn backend(e: rusqlite::Error) -> StorageError {
    StorageError::Backend(e.to_string())
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
        Ok(Loaded {
            snapshot,
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
                    tx.execute("DELETE FROM events WHERE seq <= ?1", params![cursor])
                        .map_err(backend)?;
                }
                StorageOp::ClearConfirmed => {
                    tx.execute_batch("DELETE FROM snapshot; DELETE FROM events;")
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
