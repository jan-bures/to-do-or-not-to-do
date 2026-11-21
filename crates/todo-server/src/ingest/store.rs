//! SQLite storage for email ingest: request nonces, the sender allowlist, list keys and the Inbox
//! (tables from migration 4 in [`crate::db`]). The Message-ID ledger is written by the engine, in the
//! same transaction as the task (see [`crate::engine::Engine::execute_unique`]).
//!
//! Like [`crate::auth::store::AuthStore`], the store has its own connection to the engine's database
//! file, and every method blocks. Every change the owner makes is written to the audit log in the
//! same transaction as the change, so neither can be stored without the other.
//!
//! The lookups the engine's writer makes inside its own transaction ([`list_for_key`],
//! [`inbox_list`], [`sender_allowed`]) are free functions over any connection.

use crate::auth::service::audit;
use crate::auth::store::insert_audit;
use crate::db::{StoreError, is_constraint};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::json;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use todo_core::ids::ListId;

/// Result alias.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// How long a nonce is remembered. A request is accepted only within ±5 minutes of its timestamp, so
/// a replay can come at most 10 minutes after the first use; the rest is slack for clock steps.
pub const NONCE_KEEP_MS: i64 = 15 * 60 * 1000;

/// The list key that names the Inbox (`todo+inbox@`). It can't be given to a list.
pub const INBOX_KEY: &str = "inbox";

const META_SEEDED: &str = "ingest_senders_seeded";
const META_INBOX: &str = "ingest_inbox_list";

/// An allowed sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderRow {
    /// The normalized address.
    pub address: String,
    /// When it was added.
    pub added_at: i64,
    /// `config` (the seed) or `admin`.
    pub source: String,
}

/// Why a list key could not be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// Not `[a-z0-9][a-z0-9_-]{0,63}`.
    Invalid,
    /// [`INBOX_KEY`], which always means the Inbox.
    Reserved,
    /// The key is set already.
    Taken,
}

/// Who made a change, for its audit entry.
#[derive(Debug, Clone, Copy)]
pub struct By {
    /// When (Unix milliseconds).
    pub at: i64,
    /// The client IP, if known.
    pub ip: Option<IpAddr>,
    /// Who, when not the owner on the admin pages (recorded as the entry's `actor`, for example
    /// [`crate::auth::service::ADMIN_API_ACTOR`]).
    pub actor: Option<&'static str>,
}

impl By {
    /// The owner, on the admin pages.
    #[must_use]
    pub fn owner(at: i64, ip: Option<IpAddr>) -> Self {
        Self {
            at,
            ip,
            actor: None,
        }
    }
}

fn record(tx: &Transaction<'_>, by: By, kind: &str, detail: &serde_json::Value) -> Result<()> {
    let ip = by.ip.map(|i| i.to_string());
    let mut detail = detail.clone();
    if let (Some(actor), Some(m)) = (by.actor, detail.as_object_mut()) {
        m.insert("actor".into(), json!(actor));
    }
    insert_audit(tx, by.at, kind, ip.as_deref(), &detail.to_string())
}

/// Whether `key` is a valid list key: what the Worker accepts after `todo+`.
#[must_use]
pub fn valid_key(key: &str) -> bool {
    let b = key.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

fn parse_list(s: &str, what: &str) -> Result<ListId> {
    s.parse()
        .map_err(|e| StoreError::Corrupt(format!("{what}: {e}")))
}

/// The list a key names, read through any connection (the engine's writer uses it).
pub fn list_for_key(conn: &Connection, key: &str) -> Result<Option<ListId>> {
    let id: Option<String> = conn
        .query_row("SELECT list_id FROM list_keys WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?;
    id.map(|s| parse_list(&s, "list_keys.list_id")).transpose()
}

/// The list set as the Inbox, if any (it may have been deleted since).
pub fn inbox_list(conn: &Connection) -> Result<Option<ListId>> {
    let id: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [META_INBOX], |r| {
            r.get(0)
        })
        .optional()?;
    id.map(|s| parse_list(&s, META_INBOX)).transpose()
}

/// Whether a normalized address is on the allowlist, read through any connection (the engine's
/// writer checks it again inside the transaction that stores the task).
pub fn sender_allowed(conn: &Connection, address: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM ingest_senders WHERE address = ?1",
            [address],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// The ingest store.
pub struct IngestStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for IngestStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IngestStore")
    }
}

impl IngestStore {
    /// Opens a connection to the (already migrated) database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- nonces -----------------------------------------------------------------------------------

    /// Reserves a nonce with one `INSERT` under the primary key: `false` if it was already used (a
    /// replay). Prunes nonces older than [`NONCE_KEEP_MS`] first.
    pub fn reserve_nonce(&self, nonce: &str, now_ms: i64) -> Result<bool> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM ingest_nonces WHERE seen_at < ?1",
            [now_ms.saturating_sub(NONCE_KEEP_MS)],
        )?;
        match conn.execute(
            "INSERT INTO ingest_nonces (nonce, seen_at) VALUES (?1, ?2)",
            params![nonce, now_ms],
        ) {
            Ok(_) => Ok(true),
            Err(e) if is_constraint(&e) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// How many nonces are remembered (for tests).
    pub fn nonce_count(&self) -> Result<u64> {
        let n: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM ingest_nonces", [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    // ---- senders ----------------------------------------------------------------------------------

    /// The allowlist, sorted by address.
    pub fn senders(&self) -> Result<Vec<SenderRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT address, added_at, source FROM ingest_senders ORDER BY address",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(SenderRow {
                    address: r.get(0)?,
                    added_at: r.get(1)?,
                    source: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Whether a normalized address is on the allowlist.
    pub fn is_allowed(&self, address: &str) -> Result<bool> {
        sender_allowed(&self.conn(), address)
    }

    /// Adds a normalized address (audited); `false` if it was already there.
    pub fn add_sender(&self, address: &str, by: By) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let n = tx.execute(
            "INSERT OR IGNORE INTO ingest_senders (address, added_at, source) VALUES (?1, ?2, 'admin')",
            params![address, by.at],
        )?;
        if n == 1 {
            record(
                &tx,
                by,
                audit::INGEST_SENDER_ADDED,
                &json!({ "address": address }),
            )?;
        }
        tx.commit()?;
        Ok(n == 1)
    }

    /// Removes an address (audited); `false` if it was not there.
    pub fn remove_sender(&self, address: &str, by: By) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let n = tx.execute("DELETE FROM ingest_senders WHERE address = ?1", [address])?;
        if n == 1 {
            record(
                &tx,
                by,
                audit::INGEST_SENDER_REMOVED,
                &json!({ "address": address }),
            )?;
        }
        tx.commit()?;
        Ok(n == 1)
    }

    /// Adds `addresses` (normalized) unless the allowlist was seeded before, audited with the count.
    /// Returns how many were added, or `None` if it had been seeded already.
    pub fn seed_senders(&self, addresses: &[String], now_ms: i64) -> Result<Option<usize>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let seeded = tx
            .query_row("SELECT 1 FROM meta WHERE key = ?1", [META_SEEDED], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if seeded {
            return Ok(None);
        }
        let mut added = 0;
        for a in addresses {
            added += tx.execute(
                "INSERT OR IGNORE INTO ingest_senders (address, added_at, source) VALUES (?1, ?2, 'config')",
                params![a, now_ms],
            )?;
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![META_SEEDED, now_ms.to_string()],
        )?;
        let by = By::owner(now_ms, None);
        record(
            &tx,
            by,
            audit::INGEST_SENDERS_SEEDED,
            &json!({ "count": added }),
        )?;
        tx.commit()?;
        Ok(Some(added))
    }

    // ---- list keys and the Inbox ------------------------------------------------------------------

    /// Every list key, as `(key, list)`, sorted by key. The lists may have been deleted since.
    pub fn list_keys(&self) -> Result<Vec<(String, ListId)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached("SELECT key, list_id FROM list_keys ORDER BY key")?;
        let raw = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.into_iter()
            .map(|(k, l)| Ok((k, parse_list(&l, "list_keys.list_id")?)))
            .collect()
    }

    /// Makes `key` name `list` (audited). The caller checks that the list exists.
    pub fn add_list_key(
        &self,
        key: &str,
        list: ListId,
        by: By,
    ) -> Result<std::result::Result<(), KeyError>> {
        if key == INBOX_KEY {
            return Ok(Err(KeyError::Reserved));
        }
        if !valid_key(key) {
            return Ok(Err(KeyError::Invalid));
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        match tx.execute(
            "INSERT INTO list_keys (key, list_id, set_at) VALUES (?1, ?2, ?3)",
            params![key, list.to_string(), by.at],
        ) {
            Ok(_) => {}
            Err(e) if is_constraint(&e) => return Ok(Err(KeyError::Taken)),
            Err(e) => return Err(e.into()),
        }
        record(
            &tx,
            by,
            audit::LIST_KEY_ADDED,
            &json!({ "key": key, "list": list }),
        )?;
        tx.commit()?;
        Ok(Ok(()))
    }

    /// Removes a key (audited). Returns the list it named, or `None` if there was no such key.
    pub fn remove_list_key(&self, key: &str, by: By) -> Result<Option<ListId>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let Some(list) = list_for_key(&tx, key)? else {
            return Ok(None);
        };
        tx.execute("DELETE FROM list_keys WHERE key = ?1", [key])?;
        record(
            &tx,
            by,
            audit::LIST_KEY_REMOVED,
            &json!({ "key": key, "list": list }),
        )?;
        tx.commit()?;
        Ok(Some(list))
    }

    /// The list set as the Inbox, if any.
    pub fn inbox(&self) -> Result<Option<ListId>> {
        inbox_list(&self.conn())
    }

    /// Sets (or with `None` unsets) the Inbox (audited). Returns whether it changed. The caller checks
    /// that the list exists.
    pub fn set_inbox(&self, list: Option<ListId>, by: By) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let old = inbox_list(&tx)?;
        if old == list {
            return Ok(false);
        }
        match list {
            Some(l) => tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                params![META_INBOX, l.to_string()],
            )?,
            None => tx.execute("DELETE FROM meta WHERE key = ?1", [META_INBOX])?,
        };
        record(
            &tx,
            by,
            audit::INGEST_INBOX_SET,
            &json!({ "old": old, "new": list }),
        )?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn store() -> (tempfile::TempDir, IngestStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite3");
        drop(crate::db::open_writer(&path).unwrap());
        (dir, IngestStore::open(&path).unwrap())
    }

    const BY: By = By {
        at: 1,
        ip: None,
        actor: None,
    };

    fn audit_kinds(s: &IngestStore) -> Vec<String> {
        let conn = s.conn();
        let mut stmt = conn.prepare("SELECT kind FROM audit ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    #[test]
    fn nonces_are_single_use_and_pruned_after_the_window() {
        let (_d, s) = store();
        assert!(s.reserve_nonce("a", 1_000_000).unwrap());
        assert!(!s.reserve_nonce("a", 1_000_001).unwrap(), "replay");
        assert!(s.reserve_nonce("b", 1_000_002).unwrap());
        assert_eq!(s.nonce_count().unwrap(), 2);
        // Still remembered within the window ...
        assert!(!s.reserve_nonce("a", 1_000_000 + NONCE_KEEP_MS).unwrap());
        // ... and pruned after it.
        assert!(s.reserve_nonce("c", 1_000_003 + NONCE_KEEP_MS).unwrap());
        assert_eq!(s.nonce_count().unwrap(), 1);
    }

    #[test]
    fn the_seed_applies_once_and_every_change_is_audited_with_it() {
        let (_d, s) = store();
        let seed = vec!["you@example.com".to_owned()];
        assert_eq!(s.seed_senders(&seed, 1).unwrap(), Some(1));
        assert!(s.remove_sender("you@example.com", BY).unwrap());
        assert!(!s.remove_sender("you@example.com", BY).unwrap());
        assert_eq!(s.seed_senders(&seed, 2).unwrap(), None);
        assert!(!s.is_allowed("you@example.com").unwrap());
        assert!(s.add_sender("you@example.com", BY).unwrap());
        assert!(!s.add_sender("you@example.com", BY).unwrap());
        assert_eq!(s.senders().unwrap()[0].source, "admin");
        // One entry per change; none for the no-ops.
        assert_eq!(
            audit_kinds(&s),
            [
                audit::INGEST_SENDERS_SEEDED,
                audit::INGEST_SENDER_REMOVED,
                audit::INGEST_SENDER_ADDED
            ]
        );
    }

    #[test]
    fn list_keys_are_explicit_unique_and_validated() {
        let (_d, s) = store();
        let a = ListId::from_uuid(Uuid::new_v4());
        let b = ListId::from_uuid(Uuid::new_v4());
        assert_eq!(s.add_list_key("shopping", a, BY).unwrap(), Ok(()));
        assert_eq!(
            s.add_list_key("shopping", b, BY).unwrap(),
            Err(KeyError::Taken)
        );
        assert_eq!(
            s.add_list_key("Bad Key", b, BY).unwrap(),
            Err(KeyError::Invalid)
        );
        assert_eq!(
            s.add_list_key(INBOX_KEY, b, BY).unwrap(),
            Err(KeyError::Reserved)
        );
        // Several keys may name one list.
        assert_eq!(s.add_list_key("groceries", a, BY).unwrap(), Ok(()));
        assert_eq!(s.read_key("shopping"), Some(a));
        assert_eq!(
            s.list_keys().unwrap(),
            [("groceries".to_owned(), a), ("shopping".to_owned(), a)]
        );
        assert_eq!(s.remove_list_key("shopping", BY).unwrap(), Some(a));
        assert_eq!(s.remove_list_key("shopping", BY).unwrap(), None);
        assert_eq!(s.read_key("shopping"), None);
        for (k, ok) in [
            ("a", true),
            ("0-x_y", true),
            ("-a", false),
            ("", false),
            ("Ab", false),
        ] {
            assert_eq!(valid_key(k), ok, "{k}");
        }
        assert!(valid_key(&"a".repeat(64)));
        assert!(!valid_key(&"a".repeat(65)));
        // The Inbox is its own setting.
        assert_eq!(s.inbox().unwrap(), None);
        assert!(s.set_inbox(Some(b), BY).unwrap());
        assert!(!s.set_inbox(Some(b), BY).unwrap());
        assert_eq!(s.inbox().unwrap(), Some(b));
        assert!(s.set_inbox(None, BY).unwrap());
        assert_eq!(s.inbox().unwrap(), None);
        assert_eq!(
            audit_kinds(&s),
            [
                audit::LIST_KEY_ADDED,
                audit::LIST_KEY_ADDED,
                audit::LIST_KEY_REMOVED,
                audit::INGEST_INBOX_SET,
                audit::INGEST_INBOX_SET
            ]
        );
    }

    impl IngestStore {
        fn read_key(&self, key: &str) -> Option<ListId> {
            list_for_key(&self.conn(), key).unwrap()
        }
    }
}
