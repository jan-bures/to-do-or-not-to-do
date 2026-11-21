//! `attachments.sqlite3`: which objects exist, the monthly R2 budget, and garbage-collection state.
//!
//! This is a database of its own, next to the event log, on purpose: it describes the *bucket*, not the
//! to-do history. Restoring `todo.sqlite3` from a backup leaves it alone, so it keeps telling the truth
//! about which objects exist and how much of the month's budget is spent (see the module docs of
//! [`super`] for how the garbage collector uses that after a restore).
//!
//! # Budget
//!
//! - **Storage**: the sealed size of every object the registry knows, including uploads in flight.
//! - **Class A** (PUT, DELETE) and **Class B** (GET) operations, per UTC month. A new month starts from
//!   zero. Operations are charged *before* the request is sent, so a failed request still counts.
//!
//! A charge that would pass a cap is refused and nothing is sent.
//!
//! # Schema
//!
//! Versioned with `PRAGMA user_version`: opening runs the missing steps of `MIGRATIONS` in order, one
//! transaction each, and refuses a registry newer than this code ([`RegistryError::TooNew`]) rather than
//! guess at it.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// R2's monthly free tier and storage allowance. Caps above them are refused.
pub const FREE_TIER_STORAGE_BYTES: u64 = 10_000_000_000;
/// R2's free Class A operations a month.
pub const FREE_TIER_CLASS_A: u64 = 1_000_000;
/// R2's free Class B operations a month.
pub const FREE_TIER_CLASS_B: u64 = 10_000_000;

/// Budget caps (`[attachments.budget]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BudgetCaps {
    /// Most bytes stored at once (sealed sizes).
    pub max_storage_bytes: u64,
    /// Most Class A operations (PUT, DELETE) a month.
    pub class_a_monthly: u64,
    /// Most Class B operations (GET) a month.
    pub class_b_monthly: u64,
}

impl Default for BudgetCaps {
    fn default() -> Self {
        Self {
            max_storage_bytes: 8_000_000_000,
            class_a_monthly: 800_000,
            class_b_monthly: 8_000_000,
        }
    }
}

impl BudgetCaps {
    /// Refuses caps above the free tier.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_storage_bytes > FREE_TIER_STORAGE_BYTES
            || self.class_a_monthly > FREE_TIER_CLASS_A
            || self.class_b_monthly > FREE_TIER_CLASS_B
        {
            return Err(format!(
                "attachments.budget caps must stay inside the R2 free tier ({FREE_TIER_STORAGE_BYTES} bytes, \
                 {FREE_TIER_CLASS_A} Class A and {FREE_TIER_CLASS_B} Class B operations a month)"
            ));
        }
        Ok(())
    }
}

/// An R2 operation class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpClass {
    /// Writes: PUT and DELETE (R2 does not bill deletes; they are counted anyway).
    A,
    /// Reads: GET.
    B,
}

/// Why the budget refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The object would not fit under the storage cap.
    #[error("attachment storage is full ({used} of {cap} bytes used; this needs {wanted} more)")]
    Storage {
        /// Bytes stored.
        used: u64,
        /// Bytes this upload needs.
        wanted: u64,
        /// The cap.
        cap: u64,
    },
    /// The month's operations of a class are used up.
    #[error("this month's R2 Class {class:?} operations are used up ({used} of {cap} in {month})")]
    Operations {
        /// The class.
        class: OpClass,
        /// `YYYY-MM`.
        month: String,
        /// Used this month.
        used: u64,
        /// The cap.
        cap: u64,
    },
}

/// Registry failures.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The budget refused.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// SQLite failed.
    #[error("attachment registry: {0}")]
    Sql(#[from] rusqlite::Error),
    /// The registry was written by a newer server.
    #[error(
        "attachment registry has schema version {found}, newer than this server knows ({known}); \
         run a newer server"
    )]
    TooNew {
        /// The registry's version.
        found: i64,
        /// The newest version this code knows.
        known: i64,
    },
}

/// One object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRow {
    /// The bucket key.
    pub object_key: String,
    /// The attachment it was sealed for.
    pub attachment: uuid::Uuid,
    /// The attachment's `storage_key` it belongs to (equal to `object_key` for uploads).
    pub ref_key: String,
    /// Plaintext size.
    pub plain_size: u64,
    /// Sealed size (what the bucket stores).
    pub stored_size: u64,
    /// False while the upload is in flight.
    pub stored: bool,
    /// Registration time (Unix ms).
    pub created_ms: i64,
    /// When the garbage collector first saw it unreferenced (Unix ms).
    pub gone_since_ms: Option<i64>,
    /// Claimed by the garbage collector for deletion: no write may make its attachment live again
    /// (see [`super::Attachments::collect_garbage`]).
    pub purging: bool,
}

/// This month's use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    /// `YYYY-MM`, UTC.
    pub month: String,
    /// Class A operations.
    pub class_a: u64,
    /// Class B operations.
    pub class_b: u64,
    /// Sealed bytes stored (including uploads in flight).
    pub storage_bytes: u64,
    /// Objects known.
    pub objects: u64,
}

/// The registry. Every method blocks: call from the blocking pool.
#[derive(Debug)]
pub struct Registry {
    conn: Mutex<Connection>,
    caps: BudgetCaps,
}

/// The schema, as ordered migrations: `MIGRATIONS[n]` takes a registry from version `n` to `n + 1`
/// (`PRAGMA user_version`). Append only; never edit a step that has shipped.
const MIGRATIONS: &[&str] = &[
    // 1: objects, budget, meta.
    "CREATE TABLE blobs (
        object_key TEXT PRIMARY KEY,
        attachment_id TEXT NOT NULL,
        ref_key TEXT NOT NULL,
        plain_size INTEGER NOT NULL,
        stored_size INTEGER NOT NULL,
        stored INTEGER NOT NULL DEFAULT 0,
        created_ms INTEGER NOT NULL,
        gone_since_ms INTEGER
    );
    CREATE INDEX blobs_by_attachment ON blobs (attachment_id, ref_key);
    CREATE TABLE budget (
        month TEXT PRIMARY KEY,
        class_a INTEGER NOT NULL DEFAULT 0,
        class_b INTEGER NOT NULL DEFAULT 0,
        warned INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE meta (name TEXT PRIMARY KEY, value INTEGER NOT NULL);",
    // 2: the garbage collector's claims.
    "ALTER TABLE blobs ADD COLUMN purging INTEGER NOT NULL DEFAULT 0;",
];

/// The schema version this code writes.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// Brings the registry to [`SCHEMA_VERSION`], one transaction per step. Refuses a newer registry.
fn migrate(conn: &mut Connection) -> Result<(), RegistryError> {
    let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(RegistryError::TooNew {
            found,
            known: SCHEMA_VERSION,
        });
    }
    for (step, sql) in MIGRATIONS
        .iter()
        .enumerate()
        .skip(usize::try_from(found).unwrap_or(0))
    {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", step as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// The registry file in the data directory.
pub const REGISTRY_FILE: &str = "attachments.sqlite3";

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BlobRow> {
    let att: String = r.get(1)?;
    Ok(BlobRow {
        object_key: r.get(0)?,
        attachment: att.parse().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?,
        ref_key: r.get(2)?,
        plain_size: u(r.get(3)?),
        stored_size: u(r.get(4)?),
        stored: r.get::<_, i64>(5)? != 0,
        created_ms: r.get(6)?,
        gone_since_ms: r.get(7)?,
        purging: r.get::<_, i64>(8)? != 0,
    })
}

const COLUMNS: &str = "object_key, attachment_id, ref_key, plain_size, stored_size, stored, created_ms, gone_since_ms, purging";

impl Registry {
    /// Opens (creating) the registry.
    pub fn open(path: &Path, caps: BudgetCaps) -> Result<Self, RegistryError> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            caps,
        })
    }

    /// The caps.
    #[must_use]
    pub fn caps(&self) -> BudgetCaps {
        self.caps
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn ops(conn: &Connection, month: &str) -> rusqlite::Result<(u64, u64)> {
        Ok(conn
            .query_row(
                "SELECT class_a, class_b FROM budget WHERE month = ?1",
                [month],
                |r| Ok((u(r.get(0)?), u(r.get(1)?))),
            )
            .optional()?
            .unwrap_or((0, 0)))
    }

    fn storage(conn: &Connection) -> rusqlite::Result<(u64, u64)> {
        conn.query_row(
            "SELECT COALESCE(SUM(stored_size), 0), COUNT(*) FROM blobs",
            [],
            |r| Ok((u(r.get(0)?), u(r.get(1)?))),
        )
    }

    fn charge_in(
        &self,
        conn: &Connection,
        month: &str,
        class: OpClass,
    ) -> Result<(), RegistryError> {
        let (a, b) = Self::ops(conn, month)?;
        let (used, cap) = match class {
            OpClass::A => (a, self.caps.class_a_monthly),
            OpClass::B => (b, self.caps.class_b_monthly),
        };
        if used.saturating_add(1) > cap {
            return Err(Refusal::Operations {
                class,
                month: month.to_owned(),
                used,
                cap,
            }
            .into());
        }
        let col = match class {
            OpClass::A => "class_a",
            OpClass::B => "class_b",
        };
        conn.execute(
            &format!(
                "INSERT INTO budget (month, {col}) VALUES (?1, 1)
                 ON CONFLICT (month) DO UPDATE SET {col} = {col} + 1"
            ),
            [month],
        )?;
        Ok(())
    }

    /// This month's use.
    pub fn usage(&self, month: &str) -> Result<Usage, RegistryError> {
        let conn = self.lock();
        let (class_a, class_b) = Self::ops(&conn, month)?;
        let (storage_bytes, objects) = Self::storage(&conn)?;
        Ok(Usage {
            month: month.to_owned(),
            class_a,
            class_b,
            storage_bytes,
            objects,
        })
    }

    /// Charges one operation of `class` to `month`, or refuses.
    pub fn charge(&self, month: &str, class: OpClass) -> Result<(), RegistryError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        self.charge_in(&tx, month, class)?;
        tx.commit()?;
        Ok(())
    }

    /// Fails if an upload of `stored_size` sealed bytes would be refused now. Charges nothing.
    pub fn check_upload(&self, month: &str, stored_size: u64) -> Result<(), RegistryError> {
        let conn = self.lock();
        self.check_upload_in(&conn, month, stored_size)
    }

    fn check_upload_in(
        &self,
        conn: &Connection,
        month: &str,
        stored_size: u64,
    ) -> Result<(), RegistryError> {
        let (a, _) = Self::ops(conn, month)?;
        if a.saturating_add(1) > self.caps.class_a_monthly {
            return Err(Refusal::Operations {
                class: OpClass::A,
                month: month.to_owned(),
                used: a,
                cap: self.caps.class_a_monthly,
            }
            .into());
        }
        let (used, _) = Self::storage(conn)?;
        if used.saturating_add(stored_size) > self.caps.max_storage_bytes {
            return Err(Refusal::Storage {
                used,
                wanted: stored_size,
                cap: self.caps.max_storage_bytes,
            }
            .into());
        }
        Ok(())
    }

    /// Atomically: checks storage, charges the PUT (Class A) and records the object as in flight.
    pub fn reserve_upload(&self, month: &str, blob: &BlobRow) -> Result<(), RegistryError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        self.check_upload_in(&tx, month, blob.stored_size)?;
        self.charge_in(&tx, month, OpClass::A)?;
        tx.execute(
            &format!("INSERT INTO blobs ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, NULL, 0)"),
            params![
                blob.object_key,
                blob.attachment.to_string(),
                blob.ref_key,
                i(blob.plain_size),
                i(blob.stored_size),
                blob.created_ms,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Marks an upload as complete.
    pub fn mark_stored(&self, object_key: &str) -> Result<(), RegistryError> {
        self.lock().execute(
            "UPDATE blobs SET stored = 1 WHERE object_key = ?1",
            [object_key],
        )?;
        Ok(())
    }

    /// Forgets an object (after it was deleted from the bucket, or its upload failed).
    pub fn remove(&self, object_key: &str) -> Result<(), RegistryError> {
        self.lock()
            .execute("DELETE FROM blobs WHERE object_key = ?1", [object_key])?;
        Ok(())
    }

    /// The newest stored object for an attachment and its `storage_key`.
    pub fn find(
        &self,
        attachment: &uuid::Uuid,
        ref_key: &str,
    ) -> Result<Option<BlobRow>, RegistryError> {
        Ok(self
            .lock()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM blobs WHERE attachment_id = ?1 AND ref_key = ?2 AND stored = 1
                     ORDER BY created_ms DESC, object_key DESC LIMIT 1"
                ),
                params![attachment.to_string(), ref_key],
                row,
            )
            .optional()?)
    }

    /// Up to `limit` objects with keys after `cursor` (`""` for the start), in key order.
    pub fn page_after(&self, cursor: &str, limit: usize) -> Result<Vec<BlobRow>, RegistryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM blobs WHERE object_key > ?1 ORDER BY object_key LIMIT ?2"
        ))?;
        let rows = stmt
            .query_map(
                params![cursor, i64::try_from(limit).unwrap_or(i64::MAX)],
                row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every object claimed for deletion.
    pub fn purging(&self) -> Result<Vec<BlobRow>, RegistryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM blobs WHERE purging = 1 ORDER BY object_key"
        ))?;
        let rows = stmt.query_map([], row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Claims an object for deletion, or gives the claim back.
    pub fn set_purging(&self, object_key: &str, purging: bool) -> Result<(), RegistryError> {
        self.lock().execute(
            "UPDATE blobs SET purging = ?2 WHERE object_key = ?1",
            params![object_key, i64::from(purging)],
        )?;
        Ok(())
    }

    /// Every object, oldest first. For tests and tools; the garbage collector pages instead.
    pub fn all(&self) -> Result<Vec<BlobRow>, RegistryError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM blobs ORDER BY created_ms, object_key"
        ))?;
        let rows = stmt.query_map([], row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Sets or clears when an object was first seen unreferenced.
    pub fn set_gone_since(&self, object_key: &str, at: Option<i64>) -> Result<(), RegistryError> {
        self.lock().execute(
            "UPDATE blobs SET gone_since_ms = ?2 WHERE object_key = ?1",
            params![object_key, at],
        )?;
        Ok(())
    }

    /// True exactly once per month: the caller should file the budget warning.
    pub fn claim_warning(&self, month: &str) -> Result<bool, RegistryError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO budget (month) VALUES (?1) ON CONFLICT (month) DO NOTHING",
            [month],
        )?;
        Ok(conn.execute(
            "UPDATE budget SET warned = 1 WHERE month = ?1 AND warned = 0",
            [month],
        )? == 1)
    }

    /// Gives a claimed warning back (it could not be filed; the next refusal tries again).
    pub fn release_warning(&self, month: &str) -> Result<(), RegistryError> {
        self.lock()
            .execute("UPDATE budget SET warned = 0 WHERE month = ?1", [month])?;
        Ok(())
    }

    /// A stored number.
    pub fn meta(&self, name: &str) -> Result<Option<i64>, RegistryError> {
        Ok(self
            .lock()
            .query_row("SELECT value FROM meta WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Stores a number.
    pub fn set_meta(&self, name: &str, value: i64) -> Result<(), RegistryError> {
        self.lock().execute(
            "INSERT INTO meta (name, value) VALUES (?1, ?2)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value",
            params![name, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(key: &str, size: u64) -> BlobRow {
        BlobRow {
            object_key: key.into(),
            attachment: uuid::Uuid::new_v4(),
            ref_key: key.into(),
            plain_size: size,
            stored_size: size,
            stored: false,
            created_ms: 1,
            gone_since_ms: None,
            purging: false,
        }
    }

    #[test]
    fn budget_counts_refuses_and_resets_monthly() {
        let dir = tempfile::tempdir().unwrap();
        let caps = BudgetCaps {
            max_storage_bytes: 100,
            class_a_monthly: 2,
            class_b_monthly: 1,
        };
        let r = Registry::open(&dir.path().join(REGISTRY_FILE), caps).unwrap();
        r.reserve_upload("2026-11", &blob("k1", 60)).unwrap();
        assert!(matches!(
            r.reserve_upload("2026-11", &blob("k2", 60)),
            Err(RegistryError::Refused(Refusal::Storage { used: 60, .. }))
        ));
        r.reserve_upload("2026-11", &blob("k2", 40)).unwrap();
        r.remove("k2").unwrap();
        // Two PUTs used up Class A; the refused one charged nothing.
        assert!(matches!(
            r.reserve_upload("2026-11", &blob("k3", 1)),
            Err(RegistryError::Refused(Refusal::Operations {
                class: OpClass::A,
                used: 2,
                ..
            }))
        ));
        r.charge("2026-11", OpClass::B).unwrap();
        assert!(r.charge("2026-11", OpClass::B).is_err());
        let u = r.usage("2026-11").unwrap();
        assert_eq!(
            (u.class_a, u.class_b, u.storage_bytes, u.objects),
            (2, 1, 60, 1)
        );
        // A new month: operations reset, storage does not.
        let dec = r.usage("2026-12").unwrap();
        assert_eq!((dec.class_a, dec.class_b, dec.storage_bytes), (0, 0, 60));
        r.charge("2026-12", OpClass::B).unwrap();
        drop(r);
        let r = Registry::open(&dir.path().join(REGISTRY_FILE), caps).unwrap();
        assert_eq!(r.usage("2026-11").unwrap().class_a, 2, "persisted");
    }

    /// The schema as the first release (`bbb092d`) wrote it, before claims existed.
    const V1_AS_SHIPPED: &str = "
        CREATE TABLE IF NOT EXISTS blobs (
            object_key TEXT PRIMARY KEY,
            attachment_id TEXT NOT NULL,
            ref_key TEXT NOT NULL,
            plain_size INTEGER NOT NULL,
            stored_size INTEGER NOT NULL,
            stored INTEGER NOT NULL DEFAULT 0,
            created_ms INTEGER NOT NULL,
            gone_since_ms INTEGER
        );
        CREATE INDEX IF NOT EXISTS blobs_by_attachment ON blobs (attachment_id, ref_key);
        CREATE TABLE IF NOT EXISTS budget (
            month TEXT PRIMARY KEY,
            class_a INTEGER NOT NULL DEFAULT 0,
            class_b INTEGER NOT NULL DEFAULT 0,
            warned INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS meta (name TEXT PRIMARY KEY, value INTEGER NOT NULL);
        PRAGMA user_version = 1;
        INSERT INTO blobs VALUES ('k1', '0190c4f0-0000-7000-8000-000000000001', 'k1', 5, 90, 1, 7, NULL);
        INSERT INTO budget VALUES ('2026-11', 3, 4, 1);
    ";

    #[test]
    fn a_first_release_registry_is_migrated_and_keeps_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REGISTRY_FILE);
        Connection::open(&path)
            .unwrap()
            .execute_batch(V1_AS_SHIPPED)
            .unwrap();
        let r = Registry::open(&path, BudgetCaps::default()).unwrap();
        let version: i64 = r
            .lock()
            .pragma_query_value(None, "user_version", |x| x.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let rows = r.all().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].stored && !rows[0].purging);
        assert_eq!(r.usage("2026-11").unwrap().class_a, 3);
        assert!(!r.claim_warning("2026-11").unwrap(), "the flag survived");
        // Claims work on the migrated schema and load after a reopen.
        assert!(r.purging().unwrap().is_empty());
        r.set_purging("k1", true).unwrap();
        drop(r);
        let r = Registry::open(&path, BudgetCaps::default()).unwrap();
        assert_eq!(r.purging().unwrap()[0].object_key, "k1");
    }

    #[test]
    fn a_registry_from_a_newer_server_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REGISTRY_FILE);
        Registry::open(&path, BudgetCaps::default()).unwrap();
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        match Registry::open(&path, BudgetCaps::default()) {
            Err(RegistryError::TooNew { found, known }) => {
                assert_eq!((found, known), (SCHEMA_VERSION + 1, SCHEMA_VERSION));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pages_resume_after_the_cursor_and_claims_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REGISTRY_FILE);
        let r = Registry::open(&path, BudgetCaps::default()).unwrap();
        for k in ["k3", "k1", "k2", "k4"] {
            r.reserve_upload("2026-11", &blob(k, 1)).unwrap();
        }
        let keys = |rows: Vec<BlobRow>| rows.into_iter().map(|b| b.object_key).collect::<Vec<_>>();
        assert_eq!(keys(r.page_after("", 3).unwrap()), ["k1", "k2", "k3"]);
        assert_eq!(keys(r.page_after("k3", 3).unwrap()), ["k4"]);
        assert!(r.page_after("k4", 3).unwrap().is_empty());
        r.set_purging("k2", true).unwrap();
        drop(r);
        let r = Registry::open(&path, BudgetCaps::default()).unwrap();
        assert_eq!(keys(r.purging().unwrap()), ["k2"]);
        r.set_purging("k2", false).unwrap();
        assert!(r.purging().unwrap().is_empty());
    }

    #[test]
    fn warnings_are_claimed_once_a_month() {
        let dir = tempfile::tempdir().unwrap();
        let r = Registry::open(&dir.path().join(REGISTRY_FILE), BudgetCaps::default()).unwrap();
        assert!(r.claim_warning("2026-11").unwrap());
        assert!(!r.claim_warning("2026-11").unwrap());
        r.release_warning("2026-11").unwrap();
        assert!(r.claim_warning("2026-11").unwrap());
        assert!(r.claim_warning("2026-12").unwrap());
    }

    #[test]
    fn caps_above_the_free_tier_are_refused() {
        BudgetCaps::default().validate().unwrap();
        let c = BudgetCaps {
            class_b_monthly: FREE_TIER_CLASS_B + 1,
            ..BudgetCaps::default()
        };
        assert!(c.validate().is_err());
    }
}
