//! SQLite storage: schema and migrations, the append-only event log, idempotency records and the
//! projection tables.
//!
//! The event log is the source of truth. Projection tables hold the current entities (with a few
//! columns denormalised for SQL inspection and a `data` column with the full JSON). They are written
//! in the same transaction as the events that changed them, and can always be rebuilt by folding the
//! log with [`todo_core::apply`].

use crate::scope::{
    EventScope, ScopeBuilder, Whereabouts, location, record_whereabouts, whereabouts_change,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde_json::Value;
use sha2::Digest as _;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;
use todo_core::event::{Event, EventPayload};
use todo_core::ids::{EntityId, EventId, TaskId};
use todo_core::model::{Entity, LinkKind};
use todo_core::time::Timestamp;
use todo_core::{Actor, State, apply};
use uuid::Uuid;

/// Storage errors.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite failed.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Stored JSON could not be (de)serialised.
    #[error("stored JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The stored data is inconsistent.
    #[error("database is corrupt: {0}")]
    Corrupt(String),
    /// Another process (or engine) owns the database.
    #[error(
        "database {} is in use by another todo-server process (lock file {}); stop it first",
        db.display(),
        lock.display()
    )]
    Locked {
        /// The database file.
        db: PathBuf,
        /// The lock file.
        lock: PathBuf,
    },
    /// The lock file could not be created or locked.
    #[error("cannot lock {}: {source}", path.display())]
    Lock {
        /// The lock file.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// The database was written by a newer server.
    #[error("database schema version {found} is newer than this server supports ({supported})")]
    TooNew {
        /// Version in the file.
        found: i64,
        /// Newest version this build knows.
        supported: i64,
    },
}

/// Result alias for storage operations.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// Migrations, applied in order. `PRAGMA user_version` records how many have run.
const MIGRATIONS: &[&str] = &[
    r"
CREATE TABLE events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    id          TEXT    NOT NULL UNIQUE,
    command_id  TEXT    NOT NULL,
    command_idx INTEGER NOT NULL,
    group_id    TEXT    NOT NULL,
    actor       TEXT    NOT NULL,
    at          TEXT    NOT NULL,
    undoes      TEXT,
    forced      INTEGER NOT NULL,
    payload     TEXT    NOT NULL,
    UNIQUE (command_id, command_idx)
);
CREATE INDEX events_group ON events (group_id);
CREATE TRIGGER events_append_only_update BEFORE UPDATE ON events
    BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER events_append_only_delete BEFORE DELETE ON events
    BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;

CREATE TABLE commands (
    command_id   TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    actor        TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    received_at  TEXT NOT NULL,
    result       TEXT NOT NULL
);

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE collections (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, color TEXT, position TEXT NOT NULL,
    created_at TEXT NOT NULL, deleted_at TEXT, data TEXT NOT NULL
);
CREATE TABLE lists (
    id TEXT PRIMARY KEY, collection_id TEXT, name TEXT NOT NULL, color TEXT, position TEXT NOT NULL,
    created_at TEXT NOT NULL, deleted_at TEXT, data TEXT NOT NULL
);
CREATE INDEX lists_collection ON lists (collection_id);
CREATE TABLE tasks (
    id TEXT PRIMARY KEY, list_id TEXT NOT NULL, title TEXT NOT NULL, due TEXT, priority TEXT NOT NULL,
    my_day TEXT, completed_at TEXT, position TEXT NOT NULL, created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL, deleted_at TEXT, merged_into TEXT, data TEXT NOT NULL
);
CREATE INDEX tasks_list ON tasks (list_id, position);
CREATE TABLE task_tags (
    task_id TEXT NOT NULL, tag_id TEXT NOT NULL, PRIMARY KEY (task_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX task_tags_tag ON task_tags (tag_id);
CREATE TABLE steps (
    id TEXT PRIMARY KEY, task_id TEXT NOT NULL, title TEXT NOT NULL, completed_at TEXT,
    position TEXT NOT NULL, created_at TEXT NOT NULL, deleted_at TEXT, data TEXT NOT NULL
);
CREATE INDEX steps_task ON steps (task_id);
CREATE TABLE tags (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, color TEXT, created_at TEXT NOT NULL,
    deleted_at TEXT, data TEXT NOT NULL
);
CREATE TABLE links (
    id TEXT PRIMARY KEY, task_id TEXT NOT NULL, kind TEXT NOT NULL, created_at TEXT NOT NULL,
    deleted_at TEXT, data TEXT NOT NULL
);
CREATE INDEX links_task ON links (task_id);
CREATE TABLE attachments (
    id TEXT PRIMARY KEY, task_id TEXT NOT NULL, file_name TEXT NOT NULL, media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL, storage_key TEXT NOT NULL, created_at TEXT NOT NULL,
    deleted_at TEXT, data TEXT NOT NULL
);
CREATE INDEX attachments_task ON attachments (task_id);
CREATE TABLE reminders (
    id TEXT PRIMARY KEY, task_id TEXT NOT NULL, at TEXT NOT NULL, created_at TEXT NOT NULL,
    deleted_at TEXT, data TEXT NOT NULL
);
CREATE INDEX reminders_task ON reminders (task_id);
",
    r"
-- Authentication (step 4a). Times are Unix milliseconds. Secrets are stored only as SHA-256 hashes.
CREATE TABLE auth_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE passkeys (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    cred_id      TEXT    NOT NULL UNIQUE,
    name         TEXT    NOT NULL,
    passkey      TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER
);
CREATE TABLE ceremonies (
    id         TEXT    PRIMARY KEY,
    kind       TEXT    NOT NULL,
    session    TEXT,
    state      TEXT    NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE TABLE sessions (
    id_hash      TEXT    PRIMARY KEY,
    csrf         TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    verified_at  INTEGER NOT NULL
);
CREATE TABLE devices (
    id           TEXT    PRIMARY KEY,
    name         TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at   INTEGER
);
CREATE TABLE agent_tokens (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id        TEXT    NOT NULL,
    token_hash      TEXT    NOT NULL UNIQUE,
    grants          TEXT    NOT NULL,
    writes_per_hour INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER,
    last_used_at    INTEGER,
    revoked_at      INTEGER
);
CREATE UNIQUE INDEX agent_tokens_active ON agent_tokens (agent_id) WHERE revoked_at IS NULL;
CREATE TABLE share_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash  TEXT    NOT NULL UNIQUE,
    target_kind TEXT    NOT NULL,
    target_id   TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    rotated_at  INTEGER,
    revoked_at  INTEGER
);
CREATE TABLE audit (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    at     INTEGER NOT NULL,
    kind   TEXT    NOT NULL,
    ip     TEXT,
    detail TEXT    NOT NULL
);
",
    // Event scopes (see `crate::scope`): one row per event, written with it. Rows for events stored
    // before this migration are backfilled at startup by `backfill_scopes`.
    r"
CREATE TABLE event_scopes (
    seq   INTEGER PRIMARY KEY REFERENCES events (seq),
    scope TEXT    NOT NULL
);
CREATE TRIGGER event_scopes_append_only_update BEFORE UPDATE ON event_scopes
    BEGIN SELECT RAISE(ABORT, 'event scopes are append-only'); END;
CREATE TRIGGER event_scopes_append_only_delete BEFORE DELETE ON event_scopes
    BEGIN SELECT RAISE(ABORT, 'event scopes are append-only'); END;
",
    // Email ingest (step 9, see `crate::ingest`). Times are Unix milliseconds.
    r"
-- Request nonces seen with a valid signature. The primary key makes reserving one atomic; rows older
-- than the timestamp window are pruned.
CREATE TABLE ingest_nonces (
    nonce   TEXT    PRIMARY KEY,
    seen_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX ingest_nonces_seen ON ingest_nonces (seen_at);
-- One row per ingested Message-ID: the SHA-256 of the Message-ID stored in the task's email link
-- (see `message_key`), written in the same transaction as the task. It is derived from the event log
-- (`ledger_entry`), and checked against it, and rebuilt if it differs, at startup and by
-- `rebuild-projections`.
CREATE TABLE ingest_messages (
    mid_hash TEXT PRIMARY KEY,
    task_id  TEXT NOT NULL
) WITHOUT ROWID;
-- The sender allowlist, managed by the owner on the admin page.
CREATE TABLE ingest_senders (
    address  TEXT    PRIMARY KEY,
    added_at INTEGER NOT NULL,
    source   TEXT    NOT NULL
) WITHOUT ROWID;
-- Short names of lists (`todo+<key>@`), set by the owner on the admin page. Several keys may name one
-- list. The Inbox is the list ID in `meta` under `ingest_inbox_list`.
CREATE TABLE list_keys (
    key     TEXT    PRIMARY KEY,
    list_id TEXT    NOT NULL,
    set_at  INTEGER NOT NULL
) WITHOUT ROWID;
",
];

/// Projection tables, in a fixed order (used for clearing and dumping).
pub const PROJECTION_TABLES: &[&str] = &[
    "collections",
    "lists",
    "tasks",
    "task_tags",
    "steps",
    "tags",
    "links",
    "attachments",
    "reminders",
];

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    Ok(())
}

/// The ownership lock file of a database: `<canonical db path>.lock`.
///
/// The database path is canonicalised first (the file itself if it exists, else its directory), so
/// relative paths, `..` and symlinks all lead to the same lock. The database must still be reached
/// through a single file: a hard link, a bind mount or a copy of the data directory under another path
/// gets a lock of its own, so never point two servers at the same file through such aliases.
pub fn lock_path(db: &Path) -> std::io::Result<PathBuf> {
    let canonical = match std::fs::canonicalize(db) {
        Ok(p) => p,
        Err(_) => {
            let parent = db
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = db.file_name().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the database path has no file name",
                )
            })?;
            std::fs::canonicalize(parent)?.join(name)
        }
    };
    let mut name = canonical.into_os_string();
    name.push(".lock");
    Ok(PathBuf::from(name))
}

/// The ownership lock of a database (see [`lock_database`]). Dropping it unlocks.
#[derive(Debug)]
pub struct OwnerLock(File);

impl Drop for OwnerLock {
    fn drop(&mut self) {
        // Unlock explicitly rather than rely on closing the file: a child process forked meanwhile
        // (by a test that runs a tool, say) holds a duplicate of the descriptor until it execs, and an
        // `flock` lock lasts while any duplicate is open. The next owner could then find it locked.
        if let Err(e) = self.0.unlock() {
            tracing::warn!(error = %e, "cannot unlock the database ownership lock");
        }
    }
}

/// Takes the exclusive ownership lock of the database at `db` (an OS file lock on [`lock_path`]),
/// without waiting. The lock is held until the returned guard is dropped, and the OS releases it if
/// the process dies. Fails with [`StoreError::Locked`] if another holder exists, in this process or
/// another.
pub fn lock_database(db: &Path) -> Result<OwnerLock> {
    let path = lock_path(db).map_err(|source| StoreError::Lock {
        path: db.to_owned(),
        source,
    })?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|source| StoreError::Lock {
            path: path.clone(),
            source,
        })?;
    match file.try_lock() {
        Ok(()) => Ok(OwnerLock(file)),
        Err(TryLockError::WouldBlock) => Err(StoreError::Locked {
            db: db.to_owned(),
            lock: path,
        }),
        Err(TryLockError::Error(source)) => Err(StoreError::Lock { path, source }),
    }
}

/// Opens (creating if needed) the database for writing, enables WAL and runs migrations.
pub fn open_writer(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::Corrupt(format!(
            "could not enable WAL mode (got `{mode}`)"
        )));
    }
    configure(&conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// Opens a read-only connection to an existing database.
pub fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    configure(&conn)?;
    Ok(conn)
}

/// Applies pending migrations.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let supported = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if current > supported {
        return Err(StoreError::TooNew {
            found: current,
            supported,
        });
    }
    let start = usize::try_from(current).unwrap_or(0);
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(start) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(
            None,
            "user_version",
            i64::try_from(i + 1).unwrap_or(i64::MAX),
        )?;
        tx.commit()?;
        tracing::info!(version = i + 1, "applied database migration");
    }
    Ok(())
}

// ---- events ---------------------------------------------------------------------------------------

fn to_seq(v: i64) -> Result<u64> {
    u64::try_from(v).map_err(|_| StoreError::Corrupt(format!("negative seq {v}")))
}

fn from_seq(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn corrupt<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> StoreError {
    move |e| StoreError::Corrupt(format!("{what}: {e}"))
}

const EVENT_COLUMNS: &str = "seq, id, group_id, actor, at, undoes, forced, payload";
const LOG_COLUMNS: &str =
    "seq, id, group_id, actor, at, undoes, forced, payload, command_id, command_idx";

/// A stored event as the API returns it: the event plus the command (or undo) that produced it.
///
/// The event's fields stay at the top level (flattened), so a client that only knows [`Event`] reads
/// it unchanged.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct LogEvent {
    /// The event.
    #[serde(flatten)]
    pub event: Event,
    /// The client-chosen ID of the command or undo that produced it.
    pub command_id: Uuid,
    /// Its position among that command's events (0-based).
    pub command_idx: u32,
}

type RawEvent = (
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
    bool,
    String,
);

fn raw_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawEvent> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

fn decode_event(raw: RawEvent) -> Result<Event> {
    let (seq, id, group, actor, at, undoes, forced, payload) = raw;
    Ok(Event {
        id: id.parse().map_err(corrupt("event id"))?,
        seq: Some(to_seq(seq)?),
        actor: serde_json::from_str::<Actor>(&actor)?,
        at: at.parse::<Timestamp>().map_err(corrupt("event time"))?,
        group: group.parse().map_err(corrupt("event group"))?,
        undoes: undoes
            .map(|u| u.parse::<EventId>())
            .transpose()
            .map_err(corrupt("event undoes"))?,
        forced,
        payload: serde_json::from_str::<EventPayload>(&payload)?,
    })
}

fn query_log_events(
    conn: &Connection,
    sql: &str,
    p: impl rusqlite::Params,
) -> Result<Vec<LogEvent>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let raws = stmt
        .query_map(p, |r| {
            Ok((raw_event(r)?, r.get::<_, String>(8)?, r.get::<_, i64>(9)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raws.into_iter()
        .map(|(raw, command_id, idx)| {
            Ok(LogEvent {
                event: decode_event(raw)?,
                command_id: command_id.parse().map_err(corrupt("command id"))?,
                command_idx: u32::try_from(idx).map_err(corrupt("command index"))?,
            })
        })
        .collect()
}

fn query_events(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let raws = stmt
        .query_map(p, raw_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raws.into_iter().map(decode_event).collect()
}

/// Appends an event and returns its sequence number.
pub fn insert_event(
    tx: &Transaction<'_>,
    event: &Event,
    command_id: &str,
    command_idx: usize,
) -> Result<u64> {
    tx.prepare_cached(
        "INSERT INTO events (id, command_id, command_idx, group_id, actor, at, undoes, forced, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?
    .execute(params![
        event.id.to_string(),
        command_id,
        i64::try_from(command_idx).unwrap_or(i64::MAX),
        event.group.to_string(),
        serde_json::to_string(&event.actor)?,
        event.at.to_string(),
        event.undoes.map(|u| u.to_string()),
        event.forced,
        serde_json::to_string(&event.payload)?,
    ])?;
    to_seq(tx.last_insert_rowid())
}

/// Stores an event's scope.
pub fn insert_scope(tx: &Connection, seq: u64, scope: &EventScope) -> Result<()> {
    tx.prepare_cached("INSERT INTO event_scopes (seq, scope) VALUES (?1, ?2)")?
        .execute(params![from_seq(seq), serde_json::to_string(scope)?])?;
    Ok(())
}

/// The scopes of the events with `lo <= seq <= hi`. Events without a stored scope are missing from
/// the map (callers treat them as unreadable for agents).
pub fn scopes_between(conn: &Connection, lo: u64, hi: u64) -> Result<BTreeMap<u64, EventScope>> {
    let rows = conn
        .prepare_cached("SELECT seq, scope FROM event_scopes WHERE seq >= ?1 AND seq <= ?2")?
        .query_map(params![from_seq(lo), from_seq(hi)], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(seq, json)| Ok((to_seq(seq)?, serde_json::from_str(&json)?)))
        .collect()
}

/// Records the scope of every event that has none, by folding the whole log (once, after the
/// migration that added scopes). Returns how many were added.
pub fn backfill_scopes(conn: &mut Connection) -> Result<u64> {
    let missing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events WHERE seq NOT IN (SELECT seq FROM event_scopes)",
        [],
        |r| r.get(0),
    )?;
    if missing == 0 {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    let mut state = State::new();
    let mut after = 0;
    let mut added = 0;
    loop {
        let page = events_after(&tx, after, 1000)?;
        let Some(last) = page.last().and_then(|e| e.seq) else {
            break;
        };
        let have = scopes_between(&tx, after + 1, last)?;
        for e in &page {
            let seq = e.seq.unwrap_or_default();
            let mut b = ScopeBuilder::default();
            b.observe(&state, &e.payload);
            apply(&mut state, e).map_err(|err| {
                StoreError::Corrupt(format!("event seq {seq} does not apply: {err}"))
            })?;
            if !have.contains_key(&seq) {
                b.observe(&state, &e.payload);
                insert_scope(&tx, seq, &b.finish())?;
                added += 1;
            }
        }
        after = last;
    }
    tx.commit()?;
    Ok(added)
}

/// Events with `seq > after`, oldest first, at most `limit`.
pub fn events_after(conn: &Connection, after: u64, limit: usize) -> Result<Vec<Event>> {
    query_events(
        conn,
        &format!("SELECT {EVENT_COLUMNS} FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2"),
        params![from_seq(after), i64::try_from(limit).unwrap_or(i64::MAX)],
    )
}

/// Rough JSON size of an event beyond its payload and actor (IDs, times, field names).
const EVENT_OVERHEAD: u64 = 256;

/// A page of the log read within a byte budget.
#[derive(Debug, Clone, Default)]
pub struct CappedEvents {
    /// The events, in the order asked for.
    pub events: Vec<LogEvent>,
    /// Their scopes, by seq (see [`crate::scope`]).
    pub scopes: BTreeMap<u64, EventScope>,
    /// True if the page stopped early because of the byte budget.
    pub truncated: bool,
    /// The seq of the first event in range when that event alone exceeds the budget. It was neither
    /// decoded nor returned; skip it (it can still be found with a larger budget offline).
    pub oversized: Option<u64>,
}

/// Picks the seqs that fit `max_bytes`, from `(seq, size)` rows in page order.
fn fit(sizes: &[(i64, i64)], max_bytes: usize) -> Result<(Option<u64>, bool, Option<u64>)> {
    let budget = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    let mut total = 0u64;
    let mut last = None;
    for (i, (seq, size)) in sizes.iter().enumerate() {
        let size = u64::try_from(*size).unwrap_or(0) + EVENT_OVERHEAD;
        total = total.saturating_add(size);
        if total > budget {
            let oversized = (i == 0).then(|| to_seq(*seq)).transpose()?;
            return Ok((last, true, oversized));
        }
        last = Some(to_seq(*seq)?);
    }
    Ok((last, false, None))
}

const SIZE_EXPR: &str = "length(CAST(payload AS BLOB)) + length(CAST(actor AS BLOB))";

/// Like [`events_after`], but stops before the events' JSON would exceed `max_bytes`. Row sizes are
/// read first, so nothing beyond the budget is fetched or decoded.
pub fn events_after_capped(
    conn: &Connection,
    after: u64,
    limit: usize,
    max_bytes: usize,
) -> Result<CappedEvents> {
    let sizes = conn
        .prepare_cached(&format!(
            "SELECT seq, {SIZE_EXPR} FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2"
        ))?
        .query_map(
            params![from_seq(after), i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (last, truncated, oversized) = fit(&sizes, max_bytes)?;
    let (events, scopes) = match last {
        None => (Vec::new(), BTreeMap::new()),
        Some(last) => (
            query_log_events(
                conn,
                &format!(
                    "SELECT {LOG_COLUMNS} FROM events WHERE seq > ?1 AND seq <= ?2 ORDER BY seq"
                ),
                params![from_seq(after), from_seq(last)],
            )?,
            scopes_between(conn, after + 1, last)?,
        ),
    };
    Ok(CappedEvents {
        events,
        scopes,
        truncated,
        oversized,
    })
}

/// Like [`events_before`], but within a byte budget (see [`events_after_capped`]).
pub fn events_before_capped(
    conn: &Connection,
    before: Option<u64>,
    limit: usize,
    max_bytes: usize,
) -> Result<CappedEvents> {
    let sizes = conn
        .prepare_cached(&format!(
            "SELECT seq, {SIZE_EXPR} FROM events WHERE seq < ?1 ORDER BY seq DESC LIMIT ?2"
        ))?
        .query_map(
            params![
                before.map_or(i64::MAX, from_seq),
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (last, truncated, oversized) = fit(&sizes, max_bytes)?;
    let (events, scopes) = match last {
        None => (Vec::new(), BTreeMap::new()),
        Some(last) => (
            query_log_events(
                conn,
                &format!(
                    "SELECT {LOG_COLUMNS} FROM events WHERE seq < ?1 AND seq >= ?2 ORDER BY seq DESC"
                ),
                params![before.map_or(i64::MAX, from_seq), from_seq(last)],
            )?,
            scopes_between(
                conn,
                last,
                before.map_or(u64::MAX >> 1, |b| b.saturating_sub(1)),
            )?,
        ),
    };
    Ok(CappedEvents {
        events,
        scopes,
        truncated,
        oversized,
    })
}

/// Events with `seq < before` (or the newest ones), newest first, at most `limit`.
pub fn events_before(conn: &Connection, before: Option<u64>, limit: usize) -> Result<Vec<Event>> {
    query_events(
        conn,
        &format!("SELECT {EVENT_COLUMNS} FROM events WHERE seq < ?1 ORDER BY seq DESC LIMIT ?2"),
        params![
            before.map_or(i64::MAX, from_seq),
            i64::try_from(limit).unwrap_or(i64::MAX)
        ],
    )
}

/// The event with this sequence number.
pub fn event_by_seq(conn: &Connection, seq: u64) -> Result<Option<Event>> {
    Ok(query_events(
        conn,
        &format!("SELECT {EVENT_COLUMNS} FROM events WHERE seq = ?1"),
        params![from_seq(seq)],
    )?
    .pop())
}

/// All events of a command group, oldest first.
pub fn events_in_group(conn: &Connection, group: EventId) -> Result<Vec<Event>> {
    query_events(
        conn,
        &format!("SELECT {EVENT_COLUMNS} FROM events WHERE group_id = ?1 ORDER BY seq"),
        params![group.to_string()],
    )
}

/// Sequence number of an event ID.
pub fn seq_of(conn: &Connection, id: EventId) -> Result<Option<u64>> {
    conn.prepare_cached("SELECT seq FROM events WHERE id = ?1")?
        .query_row(params![id.to_string()], |r| r.get::<_, i64>(0))
        .optional()?
        .map(to_seq)
        .transpose()
}

/// The highest sequence number (0 for an empty log).
pub fn head_seq(conn: &Connection) -> Result<u64> {
    to_seq(conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?)
}

/// Folds the whole log into a fresh state.
pub fn fold_log(conn: &Connection) -> Result<State> {
    Ok(fold_history(conn)?.0)
}

/// Folds the event log, also collecting the last known location of every entity that is no longer
/// in the state (see [`crate::scope::Whereabouts`]).
pub fn fold_history(conn: &Connection) -> Result<(State, Whereabouts)> {
    let (state, gone, _) = fold_all(conn)?;
    Ok((state, gone))
}

/// Everything derived from the log in one pass: the state, the whereabouts of removed entities and
/// the Message-ID ledger (sorted by key; the first event wins for a key).
pub fn fold_all(conn: &Connection) -> Result<(State, Whereabouts, Vec<LedgerRow>)> {
    let mut state = State::new();
    let mut gone = Whereabouts::new();
    let mut ledger = BTreeMap::<String, LedgerRow>::new();
    let mut after = 0;
    loop {
        let page = events_after(conn, after, 1000)?;
        let Some(last) = page.last().and_then(|e| e.seq) else {
            break;
        };
        for e in &page {
            let target = e.payload.target();
            let before = location(&state, target);
            apply(&mut state, e).map_err(|err| {
                StoreError::Corrupt(format!("event seq {:?} does not apply: {err}", e.seq))
            })?;
            record_whereabouts(
                &mut gone,
                target,
                whereabouts_change(before, &state, target),
            );
            if let Some(r) = ledger_entry(e) {
                ledger.entry(r.key.clone()).or_insert(r);
            }
        }
        after = last;
    }
    Ok((state, gone, ledger.into_values().collect()))
}

// ---- idempotency records --------------------------------------------------------------------------

/// A stored command result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    /// `command` or `undo`.
    pub kind: String,
    /// Hash of the actor and the request.
    pub request_hash: String,
    /// The JSON result returned to the client.
    pub result: String,
}

/// Looks up a command by its client-chosen ID.
pub fn find_command(conn: &Connection, command_id: &str) -> Result<Option<CommandRecord>> {
    Ok(conn
        .prepare_cached("SELECT kind, request_hash, result FROM commands WHERE command_id = ?1")?
        .query_row(params![command_id], |r| {
            Ok(CommandRecord {
                kind: r.get(0)?,
                request_hash: r.get(1)?,
                result: r.get(2)?,
            })
        })
        .optional()?)
}

/// A stored command of a given actor: its kind, result and when it was handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorCommand {
    /// `command` or `undo`.
    pub kind: String,
    /// The JSON result returned to the client.
    pub result: String,
    /// When it was handled.
    pub received_at: Timestamp,
}

/// Looks up a command by its client-chosen ID, only if `actor` sent it.
pub fn find_command_of(
    conn: &Connection,
    command_id: &str,
    actor: &Actor,
) -> Result<Option<ActorCommand>> {
    let row = conn
        .prepare_cached(
            "SELECT kind, result, received_at FROM commands WHERE command_id = ?1 AND actor = ?2",
        )?
        .query_row(params![command_id, serde_json::to_string(actor)?], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .optional()?;
    row.map(|(kind, result, at)| {
        Ok(ActorCommand {
            kind,
            result,
            received_at: at.parse::<Timestamp>().map_err(corrupt("command time"))?,
        })
    })
    .transpose()
}

/// Records a handled command.
pub fn insert_command(
    tx: &Transaction<'_>,
    command_id: &str,
    actor: &Actor,
    received_at: Timestamp,
    record: &CommandRecord,
) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO commands (command_id, kind, actor, request_hash, received_at, result)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?
    .execute(params![
        command_id,
        record.kind,
        serde_json::to_string(actor)?,
        record.request_hash,
        received_at.to_string(),
        record.result,
    ])?;
    Ok(())
}

// ---- projections ----------------------------------------------------------------------------------

fn opt_str<T: ToString>(v: Option<&T>) -> Option<String> {
    v.map(ToString::to_string)
}

/// Writes (inserts or replaces) one entity's projection row.
pub fn write_entity(tx: &Connection, entity: &Entity) -> Result<()> {
    let data = entity_json(entity)?;
    match entity {
        Entity::Collection(e) => {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO collections (id, name, color, position, created_at, deleted_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute(params![
                e.id.to_string(),
                e.name,
                e.color,
                e.position.to_string(),
                e.created.at.to_string(),
                opt_str(e.deleted.as_ref().map(|t| &t.at)),
                data
            ])?;
        }
        Entity::List(e) => {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO lists (id, collection_id, name, color, position, created_at, deleted_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?
            .execute(params![
                e.id.to_string(),
                opt_str(e.collection.as_ref()),
                e.name,
                e.color,
                e.position.to_string(),
                e.created.at.to_string(),
                opt_str(e.deleted.as_ref().map(|t| &t.at)),
                data
            ])?;
        }
        Entity::Task(e) => {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO tasks (id, list_id, title, due, priority, my_day, completed_at, position,
                 created_at, updated_at, deleted_at, merged_into, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?
            .execute(params![
                e.id.to_string(),
                e.list.to_string(),
                e.title,
                opt_str(e.due.as_ref()),
                e.priority.name(),
                opt_str(e.my_day.as_ref()),
                opt_str(e.completed_at.as_ref()),
                e.position.to_string(),
                e.created.at.to_string(),
                e.updated.at.to_string(),
                opt_str(e.deleted.as_ref().map(|t| &t.at)),
                opt_str(e.merged_into.as_ref()),
                data
            ])?;
            let id = e.id.to_string();
            tx.prepare_cached("DELETE FROM task_tags WHERE task_id = ?1")?
                .execute(params![id])?;
            let mut ins =
                tx.prepare_cached("INSERT INTO task_tags (task_id, tag_id) VALUES (?1, ?2)")?;
            for tag in &e.tags {
                ins.execute(params![id, tag.to_string()])?;
            }
        }
        Entity::Step(e) => {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO steps (id, task_id, title, completed_at, position, created_at, deleted_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?
            .execute(params![
                e.id.to_string(),
                e.task.to_string(),
                e.title,
                opt_str(e.completed_at.as_ref()),
                e.position.to_string(),
                e.created.at.to_string(),
                opt_str(e.deleted.as_ref().map(|t| &t.at)),
                data
            ])?;
        }
        Entity::Tag(e) => {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO tags (id, name, color, created_at, deleted_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                e.id.to_string(),
                e.name,
                e.color,
                e.created.at.to_string(),
                opt_str(e.deleted.as_ref().map(|t| &t.at)),
                data
            ])?;
        }
        Entity::Link(e) => {
            let kind = match e.kind {
                LinkKind::Web { .. } => "web",
                LinkKind::Email(_) => "email",
            };
            tx.prepare_cached(
                "INSERT OR REPLACE INTO links (id, task_id, kind, created_at, deleted_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                e.id.to_string(),
                e.task.to_string(),
                kind,
                e.created.at.to_string(),
                opt_str(e.deleted.as_ref().map(|t| &t.at)),
                data
            ])?;
        }
        Entity::Attachment(e) => {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO attachments (id, task_id, file_name, media_type, size_bytes, storage_key,
                 created_at, deleted_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?
            .execute(params![
                e.id.to_string(),
                e.task.to_string(),
                e.file_name,
                e.media_type,
                i64::try_from(e.size_bytes).unwrap_or(i64::MAX),
                e.storage_key,
                e.created.at.to_string(),
                opt_str(e.deleted.as_ref().map(|t| &t.at)),
                data
            ])?;
        }
        Entity::Reminder(e) => {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO reminders (id, task_id, at, created_at, deleted_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                e.id.to_string(),
                e.task.to_string(),
                e.at.to_string(),
                e.created.at.to_string(),
                opt_str(e.deleted.as_ref().map(|t| &t.at)),
                data
            ])?;
        }
    }
    Ok(())
}

/// The JSON of the entity itself, without the `kind` wrapper (what the `data` column holds).
fn entity_json(entity: &Entity) -> serde_json::Result<String> {
    match entity {
        Entity::Collection(e) => serde_json::to_string(e),
        Entity::List(e) => serde_json::to_string(e),
        Entity::Task(e) => serde_json::to_string(e),
        Entity::Step(e) => serde_json::to_string(e),
        Entity::Tag(e) => serde_json::to_string(e),
        Entity::Link(e) => serde_json::to_string(e),
        Entity::Attachment(e) => serde_json::to_string(e),
        Entity::Reminder(e) => serde_json::to_string(e),
    }
}

fn table_of(id: EntityId) -> &'static str {
    match id {
        EntityId::Collection(_) => "collections",
        EntityId::List(_) => "lists",
        EntityId::Task(_) => "tasks",
        EntityId::Step(_) => "steps",
        EntityId::Tag(_) => "tags",
        EntityId::Link(_) => "links",
        EntityId::Attachment(_) => "attachments",
        EntityId::Reminder(_) => "reminders",
    }
}

fn raw_id(id: EntityId) -> String {
    match id {
        EntityId::Collection(i) => i.to_string(),
        EntityId::List(i) => i.to_string(),
        EntityId::Task(i) => i.to_string(),
        EntityId::Step(i) => i.to_string(),
        EntityId::Tag(i) => i.to_string(),
        EntityId::Link(i) => i.to_string(),
        EntityId::Attachment(i) => i.to_string(),
        EntityId::Reminder(i) => i.to_string(),
    }
}

/// Removes one entity's projection row (used when a creation is withdrawn).
pub fn remove_entity(tx: &Connection, id: EntityId) -> Result<()> {
    let raw = raw_id(id);
    tx.prepare_cached(&format!("DELETE FROM {} WHERE id = ?1", table_of(id)))?
        .execute(params![raw])?;
    if matches!(id, EntityId::Task(_)) {
        tx.prepare_cached("DELETE FROM task_tags WHERE task_id = ?1")?
            .execute(params![raw])?;
    }
    Ok(())
}

/// Brings the projection rows of `ids` in line with `state`.
pub fn sync_entities(
    tx: &Connection,
    state: &State,
    ids: impl IntoIterator<Item = EntityId>,
) -> Result<()> {
    for id in ids {
        match state.entity(id) {
            Some(e) => write_entity(tx, &e)?,
            None => remove_entity(tx, id)?,
        }
    }
    Ok(())
}

/// The dedupe key of an ingested email: the lowercase hex SHA-256 of the `Message-ID` exactly as its
/// email link stores it.
#[must_use]
pub fn message_key(stored_message_id: &str) -> String {
    hex::encode(sha2::Sha256::digest(stored_message_id.as_bytes()))
}

/// A row of the Message-ID ledger (`ingest_messages`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LedgerRow {
    /// [`message_key`] of the Message-ID.
    pub key: String,
    /// The task created for it.
    pub task: TaskId,
}

/// The ledger row an event implies: an email link created by email ingest (the event that
/// [`crate::engine::Engine::execute_unique`] stores with the row). Undoing the task does not remove
/// it, so a Message-ID stays taken.
#[must_use]
pub fn ledger_entry(e: &Event) -> Option<LedgerRow> {
    if !matches!(e.actor, Actor::EmailIngest) || e.undoes.is_some() {
        return None;
    }
    let EventPayload::Created {
        entity: Entity::Link(l),
    } = &e.payload
    else {
        return None;
    };
    let LinkKind::Email(m) = &l.kind else {
        return None;
    };
    Some(LedgerRow {
        key: message_key(&m.message_id),
        task: l.task,
    })
}

/// The task recorded with a unique key (see [`crate::engine::Engine::execute_unique`]).
pub fn unique_task(conn: &Connection, key: &str) -> Result<Option<TaskId>> {
    let id: Option<String> = conn
        .query_row(
            "SELECT task_id FROM ingest_messages WHERE mid_hash = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|s| s.parse().map_err(corrupt("ingest_messages.task_id")))
        .transpose()
}

/// Records a unique key with its task. `false` if the key is already taken (nothing is written).
pub fn insert_unique(tx: &Connection, key: &str, task: TaskId) -> Result<bool> {
    match tx.execute(
        "INSERT INTO ingest_messages (mid_hash, task_id) VALUES (?1, ?2)",
        params![key, task.to_string()],
    ) {
        Ok(_) => Ok(true),
        Err(e) if is_constraint(&e) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// The Message-ID ledger, sorted by key.
pub fn load_ledger(conn: &Connection) -> Result<Vec<LedgerRow>> {
    let mut stmt =
        conn.prepare("SELECT mid_hash, task_id FROM ingest_messages ORDER BY mid_hash")?;
    let raw = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raw.into_iter()
        .map(|(key, task)| {
            Ok(LedgerRow {
                key,
                task: task.parse().map_err(corrupt("ingest_messages.task_id"))?,
            })
        })
        .collect()
}

/// Replaces the Message-ID ledger with `rows`.
pub fn write_ledger(conn: &mut Connection, rows: &[LedgerRow]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM ingest_messages", [])?;
    for r in rows {
        insert_unique(&tx, &r.key, r.task)?;
    }
    tx.commit()?;
    Ok(())
}

/// Checks the Message-ID ledger against the one derived from the log, rewriting it if they differ.
/// Returns whether it was rewritten.
pub fn sync_ledger(conn: &mut Connection, derived: &[LedgerRow]) -> Result<bool> {
    if load_ledger(conn)? == derived {
        return Ok(false);
    }
    write_ledger(conn, derived)?;
    Ok(true)
}

/// True for a unique or primary-key constraint violation.
#[must_use]
pub fn is_constraint(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// The log epoch: a random ID of this event log, created with the database. A restore from a backup
/// must rotate it (`todo-server rotate-log-epoch`) so that clients notice the log is not the one they
/// followed.
pub fn log_epoch(conn: &Connection) -> Result<Option<Uuid>> {
    conn.query_row("SELECT value FROM meta WHERE key = 'log_epoch'", [], |r| {
        r.get::<_, String>(0)
    })
    .optional()?
    .map(|v| v.parse().map_err(corrupt("log epoch")))
    .transpose()
}

/// Sets the log epoch.
pub fn set_log_epoch(conn: &Connection, epoch: Uuid) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('log_epoch', ?1)",
        params![epoch.to_string()],
    )?;
    Ok(())
}

/// Records the sequence number the projections reflect.
pub fn set_projection_seq(tx: &Connection, seq: u64) -> Result<()> {
    tx.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES ('projection_seq', ?1)")?
        .execute(params![seq.to_string()])?;
    Ok(())
}

/// The sequence number the projections reflect, if recorded.
pub fn projection_seq(conn: &Connection) -> Result<Option<u64>> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'projection_seq'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    v.map(|s| s.parse().map_err(corrupt("projection_seq")))
        .transpose()
}

fn load_table<T: serde::de::DeserializeOwned>(conn: &Connection, table: &str) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(&format!("SELECT data FROM {table} ORDER BY id"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.iter()
        .map(|s| serde_json::from_str(s).map_err(StoreError::from))
        .collect()
}

/// Loads the entities from the projection tables (grants and the journal stay empty).
pub fn load_projections(conn: &Connection) -> Result<State> {
    let mut s = State::new();
    s.collections = load_table::<todo_core::model::Collection>(conn, "collections")?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    s.lists = load_table::<todo_core::model::List>(conn, "lists")?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    s.tasks = load_table::<todo_core::model::Task>(conn, "tasks")?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    s.steps = load_table::<todo_core::model::Step>(conn, "steps")?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    s.tags = load_table::<todo_core::model::Tag>(conn, "tags")?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    s.links = load_table::<todo_core::model::Link>(conn, "links")?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    s.attachments = load_table::<todo_core::model::AttachmentRef>(conn, "attachments")?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    s.reminders = load_table::<todo_core::model::Reminder>(conn, "reminders")?
        .into_iter()
        .map(|e| (e.id, e))
        .collect();
    Ok(s)
}

fn all_entities(state: &State) -> Vec<Entity> {
    let mut v: Vec<Entity> = Vec::new();
    v.extend(state.collections.values().cloned().map(Entity::Collection));
    v.extend(state.lists.values().cloned().map(Entity::List));
    v.extend(state.tasks.values().cloned().map(Entity::Task));
    v.extend(state.steps.values().cloned().map(Entity::Step));
    v.extend(state.tags.values().cloned().map(Entity::Tag));
    v.extend(state.links.values().cloned().map(Entity::Link));
    v.extend(state.attachments.values().cloned().map(Entity::Attachment));
    v.extend(state.reminders.values().cloned().map(Entity::Reminder));
    v
}

/// Replaces every projection row with the entities of `state`, recording `seq` as the projection head.
pub fn write_all_projections(conn: &mut Connection, state: &State, seq: u64) -> Result<()> {
    let tx = conn.transaction()?;
    for table in PROJECTION_TABLES {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    for e in all_entities(state) {
        write_entity(&tx, &e)?;
    }
    set_projection_seq(&tx, seq)?;
    tx.commit()?;
    Ok(())
}

/// Rebuilds the projection tables and the Message-ID ledger from the event log. Returns the folded
/// state.
pub fn rebuild_projections(conn: &mut Connection) -> Result<State> {
    let (state, _, ledger) = fold_all(conn)?;
    let head = head_seq(conn)?;
    write_all_projections(conn, &state, head)?;
    write_ledger(conn, &ledger)?;
    Ok(state)
}

fn cell(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => Value::from(f),
        ValueRef::Text(t) | ValueRef::Blob(t) => Value::from(String::from_utf8_lossy(t)),
    }
}

/// Every projection row, as JSON values, keyed by table name.
pub type ProjectionDump = BTreeMap<String, Vec<Vec<Value>>>;

/// Every projection row, as JSON values, keyed by table. Used to compare live and rebuilt projections.
pub fn dump_projections(conn: &Connection) -> Result<ProjectionDump> {
    let mut out = BTreeMap::new();
    for table in PROJECTION_TABLES {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY 1, 2"))?;
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                (0..n)
                    .map(|i| r.get_ref(i).map(cell))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.insert((*table).to_owned(), rows);
    }
    Ok(out)
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn dropping_the_lock_releases_it_while_a_duplicate_descriptor_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("todo.sqlite3");
        let lock = lock_database(&db).unwrap();
        // What a child process forked at the wrong moment holds until it execs.
        let duplicate = lock.0.try_clone().unwrap();
        assert!(matches!(lock_database(&db), Err(StoreError::Locked { .. })));
        drop(lock);
        let again = lock_database(&db).expect("released despite the duplicate");
        drop(duplicate);
        drop(again);
    }
}
