//! SQLite storage for authentication: owner passkeys, ceremony state, browser sessions, paired
//! devices, agent tokens, share links and the audit log (tables from migration 2 in [`crate::db`]).
//!
//! The store has its own connection to the engine's database file (WAL mode lets it write alongside
//! the engine's writer). Every method blocks: call them from the blocking pool. Secrets are passed in
//! and stored only as hex SHA-256 hashes; times are Unix milliseconds.

use crate::db::StoreError;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use todo_core::{AgentId, DeviceId, Grants};
use webauthn_rs::prelude::Passkey;

/// Result alias.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// Most audit rows kept; older ones are dropped.
pub const AUDIT_KEEP: i64 = 20_000;

/// Sentinel for a signed-in session that has not yet asserted a passkey.
pub(crate) const UNVERIFIED_AT: i64 = -1;

/// A stored owner passkey.
#[derive(Debug, Clone)]
pub struct PasskeyRow {
    /// Row ID.
    pub id: i64,
    /// A label.
    pub name: String,
    /// The credential (public key, sign counter and flags).
    pub passkey: Passkey,
    /// Registration time.
    pub created_at: i64,
    /// Last successful use.
    pub last_used_at: Option<i64>,
    /// The device or browser session that issued the invitation.
    pub issued_by: Option<String>,
}

/// A browser session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// Hex SHA-256 of the cookie value.
    pub id_hash: String,
    /// The CSRF synchronizer token.
    pub csrf: String,
    /// Sign-in time.
    pub created_at: i64,
    /// Last request.
    pub last_seen_at: i64,
    /// Last passkey assertion (sign-in or step-up), or -1 if not yet asserted.
    pub verified_at: i64,
}

/// A paired device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// The device ID (the actor of its events).
    pub id: DeviceId,
    /// The name the client gave at pairing.
    pub name: String,
    /// Hex SHA-256 of the device token.
    pub token_hash: String,
    /// Pairing time.
    pub created_at: i64,
    /// Last authenticated request (updated at most once a minute).
    pub last_used_at: Option<i64>,
    /// Revocation time.
    pub revoked_at: Option<i64>,
    /// The device or browser session that authorized the pairing.
    pub issued_by: Option<String>,
}

/// An agent token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    /// Row ID.
    pub id: i64,
    /// The agent.
    pub agent: AgentId,
    /// Hex SHA-256 of the token.
    pub token_hash: String,
    /// Access levels.
    pub grants: Grants,
    /// Write quota.
    pub writes_per_hour: u32,
    /// Creation time.
    pub created_at: i64,
    /// Expiry, if any.
    pub expires_at: Option<i64>,
    /// Last authenticated request (updated at most once a minute).
    pub last_used_at: Option<i64>,
    /// Revocation time.
    pub revoked_at: Option<i64>,
}

/// What a share link shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareTarget {
    /// One list.
    List(todo_core::ids::ListId),
    /// The lists of a collection.
    Collection(todo_core::ids::CollectionId),
}

impl ShareTarget {
    fn kind(self) -> &'static str {
        match self {
            Self::List(_) => "list",
            Self::Collection(_) => "collection",
        }
    }

    fn id(self) -> String {
        match self {
            Self::List(l) => l.to_string(),
            Self::Collection(c) => c.to_string(),
        }
    }

    fn parse(kind: &str, id: &str) -> Result<Self> {
        let bad = |e: String| StoreError::Corrupt(format!("share target: {e}"));
        match kind {
            "list" => Ok(Self::List(id.parse().map_err(|e| bad(format!("{e}")))?)),
            "collection" => Ok(Self::Collection(
                id.parse().map_err(|e| bad(format!("{e}")))?,
            )),
            other => Err(bad(format!("unknown kind {other}"))),
        }
    }
}

/// A share link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRow {
    /// Row ID.
    pub id: i64,
    /// What it shows.
    pub target: ShareTarget,
    /// Creation time.
    pub created_at: i64,
    /// Last rotation.
    pub rotated_at: Option<i64>,
    /// Revocation time.
    pub revoked_at: Option<i64>,
}

/// An audit entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRow {
    /// Row ID.
    pub id: i64,
    /// Time.
    pub at: i64,
    /// What happened (see [`super::audit`]).
    pub kind: String,
    /// Client IP, if known.
    pub ip: Option<String>,
    /// JSON details (never secrets).
    pub detail: String,
}

/// An audit entry to write in the same transaction as the change it records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAudit {
    /// Time.
    pub at: i64,
    /// What happened (see [`super::audit`]).
    pub kind: &'static str,
    /// Client IP, if known.
    pub ip: Option<String>,
    /// JSON details (never secrets).
    pub detail: String,
}

impl NewAudit {
    fn insert(&self, conn: &Connection) -> Result<()> {
        insert_audit(conn, self.at, self.kind, self.ip.as_deref(), &self.detail)
    }
}

/// Appends an audit entry through any connection to the database (so a change and its audit entry
/// can share one transaction), and drops the oldest beyond [`AUDIT_KEEP`].
pub fn insert_audit(
    conn: &Connection,
    at: i64,
    kind: &str,
    ip: Option<&str>,
    detail: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audit (at, kind, ip, detail) VALUES (?1, ?2, ?3, ?4)",
        params![at, kind, ip, detail],
    )?;
    let id = conn.last_insert_rowid();
    if id % 512 == 0 {
        conn.execute("DELETE FROM audit WHERE id <= ?1", [id - AUDIT_KEEP])?;
    }
    Ok(())
}

/// The authentication store.
pub struct AuthStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for AuthStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthStore")
    }
}

const DESCENDANTS: &str = "WITH RECURSIVE descendants(id) AS (
    SELECT ?1
    UNION
    SELECT d.id FROM devices d JOIN descendants a
        ON d.issued_by = 'Device ' || a.id OR d.issued_via_device = a.id
)";
const ISSUED_PASSKEY: &str = "issued_via_device IN descendants OR EXISTS (
    SELECT 1 FROM descendants a WHERE passkeys.issued_by = 'Device ' || a.id
)";

fn insert_session(conn: &Connection, row: &SessionRow, passkey_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (id_hash, csrf, created_at, last_seen_at, verified_at, passkey_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            row.id_hash,
            row.csrf,
            row.created_at,
            row.last_seen_at,
            row.verified_at,
            passkey_id
        ],
    )?;
    Ok(())
}

fn corrupt(what: &str) -> impl FnOnce(String) -> StoreError + '_ {
    move |e| StoreError::Corrupt(format!("{what}: {e}"))
}

type RawDevice = (
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawDevice> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}

type RawAgent = (
    i64,
    String,
    String,
    String,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

fn agent_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawAgent> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
    ))
}

fn decode_agent(raw: RawAgent) -> Result<AgentRow> {
    let (id, agent, token_hash, grants, wph, created_at, expires_at, last_used_at, revoked_at) =
        raw;
    Ok(AgentRow {
        id,
        agent: AgentId::new(agent).map_err(|e| corrupt("agent id")(e.to_string()))?,
        token_hash,
        grants: serde_json::from_str(&grants)?,
        writes_per_hour: u32::try_from(wph).unwrap_or(u32::MAX),
        created_at,
        expires_at,
        last_used_at,
        revoked_at,
    })
}

const AGENT_COLUMNS: &str = "id, agent_id, token_hash, grants, writes_per_hour, created_at, expires_at, last_used_at, revoked_at";

impl AuthStore {
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

    /// Runs one statement and, if it changed a row, appends `audit`, in one transaction: the change is
    /// stored only with its audit entry. Returns whether a row changed.
    fn update_audited(
        &self,
        sql: &str,
        p: impl rusqlite::Params,
        audit: &NewAudit,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let n = tx.execute(sql, p)?;
        if n == 1 {
            audit.insert(&tx)?;
        }
        tx.commit()?;
        Ok(n == 1)
    }

    // ---- meta -------------------------------------------------------------------------------------

    /// A meta value.
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM auth_meta WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Sets a meta value.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO auth_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- passkeys ---------------------------------------------------------------------------------

    /// All owner passkeys.
    pub fn passkeys(&self) -> Result<Vec<PasskeyRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT id, name, passkey, created_at, last_used_at, issued_by FROM passkeys ORDER BY id",
        )?;
        let raws = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter()
            .map(|(id, name, pk, created_at, last_used_at, issued_by)| {
                Ok(PasskeyRow {
                    id,
                    name,
                    passkey: serde_json::from_str(&pk)?,
                    created_at,
                    last_used_at,
                    issued_by,
                })
            })
            .collect()
    }

    /// Stores a new passkey. Returns its ID unless it was already registered.
    pub fn add_passkey(
        &self,
        name: &str,
        passkey: &Passkey,
        now: i64,
        issued_via_device: Option<&str>,
    ) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "INSERT INTO passkeys (cred_id, name, passkey, created_at, issued_via_device) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (cred_id) DO NOTHING RETURNING id",
                params![
                    hex::encode(passkey.cred_id()),
                    name,
                    serde_json::to_string(passkey)?,
                    now,
                    issued_via_device,
                ],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Deletes a passkey and its sessions atomically, only after an older credential asserted.
    /// Passkey IDs are monotonically assigned, even for registrations on the same clock tick.
    pub(crate) fn revoke_passkey(
        &self,
        id: i64,
        verified_by: i64,
        audit: &NewAudit,
    ) -> Result<bool> {
        self.update_audited(
            "DELETE FROM passkeys WHERE id = ?1 AND ?2 < id AND EXISTS (SELECT 1 FROM passkeys WHERE id = ?2)",
            params![id, verified_by],
            audit,
        )
    }

    /// Saves a passkey and its initial session together, so neither can be left half-created.
    pub(crate) fn add_passkey_with_session(
        &self,
        name: &str,
        passkey: &Passkey,
        session: &SessionRow,
        issued_by: &str,
        issued_via_device: Option<&str>,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let id: Option<i64> = tx.query_row(
            "INSERT INTO passkeys (cred_id, name, passkey, created_at, issued_by, issued_via_device) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (cred_id) DO NOTHING RETURNING id",
            params![
                hex::encode(passkey.cred_id()),
                name,
                serde_json::to_string(passkey)?,
                session.created_at,
                issued_by,
                issued_via_device,
            ],
            |r| r.get(0),
        ).optional()?;
        let Some(id) = id else { return Ok(false) };
        insert_session(&tx, session, id)?;
        tx.commit()?;
        Ok(true)
    }

    /// Saves a passkey after use (new sign counter) and records the use.
    pub fn update_passkey(&self, id: i64, passkey: &Passkey, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE passkeys SET passkey = ?2, last_used_at = ?3 WHERE id = ?1",
            params![id, serde_json::to_string(passkey)?, now],
        )?;
        Ok(())
    }

    // ---- ceremonies -------------------------------------------------------------------------------

    /// Stores the server half of a WebAuthn ceremony. `session` binds it to a browser session.
    pub fn put_ceremony(
        &self,
        id_hash: &str,
        kind: &str,
        session: Option<&str>,
        state: &str,
        expires_at: i64,
        now: i64,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM ceremonies WHERE expires_at <= ?1", [now])?;
        conn.execute(
            "INSERT INTO ceremonies (id, kind, session, state, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id_hash, kind, session, state, expires_at],
        )?;
        Ok(())
    }

    /// Takes (deletes and returns) a ceremony's state if it exists, has this kind and session and has
    /// not expired. A ceremony can be finished at most once.
    pub fn take_ceremony(
        &self,
        id_hash: &str,
        kind: &str,
        session: Option<&str>,
        now: i64,
    ) -> Result<Option<String>> {
        let conn = self.conn();
        let row: Option<(String, Option<String>, String, i64)> = conn
            .query_row(
                "DELETE FROM ceremonies WHERE id = ?1 RETURNING kind, session, state, expires_at",
                [id_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        Ok(row.and_then(|(k, s, state, exp)| {
            (k == kind && s.as_deref() == session && exp > now).then_some(state)
        }))
    }

    // ---- sessions ---------------------------------------------------------------------------------

    /// Creates a session.
    pub fn create_session(&self, session: &SessionRow, passkey_id: i64) -> Result<()> {
        insert_session(&self.conn(), session, passkey_id)
    }

    /// A session by its hash.
    pub fn session(&self, id_hash: &str) -> Result<Option<SessionRow>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id_hash, csrf, created_at, last_seen_at, verified_at FROM sessions WHERE id_hash = ?1",
                [id_hash],
                |r| {
                    Ok(SessionRow {
                        id_hash: r.get(0)?,
                        csrf: r.get(1)?,
                        created_at: r.get(2)?,
                        last_seen_at: r.get(3)?,
                        verified_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// The credential asserted at the session's last explicit step-up, if any.
    pub(crate) fn step_up_passkey(&self, id_hash: &str) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT step_up_passkey_id FROM sessions WHERE id_hash = ?1",
                [id_hash],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// The nearest device ancestor of the credential through which the session exists.
    pub(crate) fn session_device_origin(&self, id_hash: &str) -> Result<Option<String>> {
        Ok(self.conn().query_row(
            "SELECT COALESCE(p.issued_via_device, CASE WHEN p.issued_by LIKE 'Device %' THEN substr(p.issued_by, 8) END)
             FROM sessions s JOIN passkeys p ON p.id = s.passkey_id WHERE s.id_hash = ?1",
            [id_hash],
            |row| row.get::<_, Option<String>>(0),
        ).optional()?.flatten())
    }

    /// Records activity on a session.
    pub fn touch_session(&self, id_hash: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET last_seen_at = ?2 WHERE id_hash = ?1",
            params![id_hash, now],
        )?;
        Ok(())
    }

    /// Records a fresh passkey verification (step-up) on a session.
    pub fn set_verified(&self, id_hash: &str, now: i64, passkey_id: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET verified_at = ?2, last_seen_at = ?2, step_up_passkey_id = ?3 WHERE id_hash = ?1",
            params![id_hash, now, passkey_id],
        )?;
        Ok(())
    }

    /// Ends a session.
    pub fn delete_session(&self, id_hash: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM sessions WHERE id_hash = ?1", [id_hash])?;
        Ok(())
    }

    /// Deletes sessions idle since `idle_before` or created before `created_before`.
    pub fn purge_sessions(&self, idle_before: i64, created_before: i64) -> Result<()> {
        self.conn().execute(
            "DELETE FROM sessions WHERE last_seen_at < ?1 OR created_at < ?2",
            params![idle_before, created_before],
        )?;
        Ok(())
    }

    // ---- devices ----------------------------------------------------------------------------------

    /// Stores a newly paired device.
    pub fn add_device(
        &self,
        id: &DeviceId,
        name: &str,
        token_hash: &str,
        now: i64,
        issued_by: Option<&str>,
        issued_via_device: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO devices (id, name, token_hash, created_at, issued_by, issued_via_device) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id.as_str(), name, token_hash, now, issued_by, issued_via_device],
        )?;
        Ok(())
    }

    /// All devices, newest first.
    pub fn devices(&self) -> Result<Vec<DeviceRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT id, name, token_hash, created_at, last_used_at, revoked_at, issued_by FROM devices
             ORDER BY created_at DESC, id",
        )?;
        let raws = stmt
            .query_map([], device_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter()
            .map(
                |(id, name, token_hash, created_at, last_used_at, revoked_at, issued_by)| {
                    Ok(DeviceRow {
                        id: DeviceId::new(id).map_err(|e| corrupt("device id")(e.to_string()))?,
                        name,
                        token_hash,
                        created_at,
                        last_used_at,
                        revoked_at,
                        issued_by,
                    })
                },
            )
            .collect()
    }

    /// Counts other native and browser sign-ins that would be revoked with this device.
    pub(crate) fn issued_credential_count(&self, id: &str) -> Result<i64> {
        Ok(self.conn().query_row(
            &format!("{DESCENDANTS}
                SELECT (SELECT count(*) FROM devices WHERE id IN descendants AND id <> ?1 AND revoked_at IS NULL)
                     + (SELECT count(*) FROM passkeys WHERE {ISSUED_PASSKEY})"),
            [id],
            |row| row.get(0),
        )?)
    }

    /// Revokes a device, all its descendants and their sessions, with one audit entry and transaction.
    /// Returns false if there is no such active device.
    pub fn revoke_device(&self, id: &str, now: i64, audit: &NewAudit) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let active: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM devices WHERE id = ?1 AND revoked_at IS NULL)",
            [id],
            |row| row.get(0),
        )?;
        if !active {
            return Ok(false);
        }
        tx.execute(
            &format!("{DESCENDANTS} UPDATE devices SET revoked_at = ?2 WHERE id IN descendants AND revoked_at IS NULL"),
            params![id, now],
        )?;
        // Both the sign-in and step-up foreign keys delete sessions of these passkeys.
        tx.execute(
            &format!("{DESCENDANTS} DELETE FROM passkeys WHERE {ISSUED_PASSKEY}"),
            [id],
        )?;
        audit.insert(&tx)?;
        tx.commit()?;
        Ok(true)
    }

    /// Records use of a device token.
    pub fn touch_device(&self, id: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE devices SET last_used_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    // ---- agent tokens -----------------------------------------------------------------------------

    /// Stores a new agent token. Returns `None` if the agent already has an active token.
    pub fn add_agent(
        &self,
        agent: &AgentId,
        token_hash: &str,
        grants: &Grants,
        writes_per_hour: u32,
        expires_at: Option<i64>,
        now: i64,
    ) -> Result<Option<i64>> {
        let conn = self.conn();
        let r = conn.execute(
            "INSERT INTO agent_tokens (agent_id, token_hash, grants, writes_per_hour, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                agent.as_str(),
                token_hash,
                serde_json::to_string(grants)?,
                i64::from(writes_per_hour),
                now,
                expires_at
            ],
        );
        match r {
            Ok(_) => Ok(Some(conn.last_insert_rowid())),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// All agent tokens, newest first.
    pub fn agents(&self) -> Result<Vec<AgentRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {AGENT_COLUMNS} FROM agent_tokens ORDER BY id DESC"
        ))?;
        let raws = stmt
            .query_map([], agent_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter().map(decode_agent).collect()
    }

    /// One agent token.
    pub fn agent(&self, id: i64) -> Result<Option<AgentRow>> {
        let raw = self
            .conn()
            .query_row(
                &format!("SELECT {AGENT_COLUMNS} FROM agent_tokens WHERE id = ?1"),
                [id],
                agent_row,
            )
            .optional()?;
        raw.map(decode_agent).transpose()
    }

    /// Revokes an agent token, with its audit entry. Returns false if it is not active.
    pub fn revoke_agent(&self, id: i64, now: i64, audit: &NewAudit) -> Result<bool> {
        self.update_audited(
            "UPDATE agent_tokens SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now],
            audit,
        )
    }

    /// Replaces an active agent token's grants, with the audit entry. Returns false if it is not
    /// active.
    pub fn set_agent_grants(&self, id: i64, grants: &Grants, audit: &NewAudit) -> Result<bool> {
        self.update_audited(
            "UPDATE agent_tokens SET grants = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, serde_json::to_string(grants)?],
            audit,
        )
    }

    /// Replaces an active agent token's grants and write quota, with the audit entry. Returns false
    /// if it is not active.
    pub fn set_agent_access(
        &self,
        id: i64,
        grants: &Grants,
        writes_per_hour: u32,
        audit: &NewAudit,
    ) -> Result<bool> {
        self.update_audited(
            "UPDATE agent_tokens SET grants = ?2, writes_per_hour = ?3 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, serde_json::to_string(grants)?, i64::from(writes_per_hour)],
            audit,
        )
    }

    /// Records use of an agent token.
    pub fn touch_agent(&self, id: i64, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE agent_tokens SET last_used_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    // ---- share links ------------------------------------------------------------------------------

    /// Stores a new share link.
    pub fn add_share(&self, token_hash: &str, target: ShareTarget, now: i64) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO share_links (token_hash, target_kind, target_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![token_hash, target.kind(), target.id(), now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn shares_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<ShareRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT id, target_kind, target_id, created_at, rotated_at, revoked_at FROM share_links {clause}"
        ))?;
        let raws = stmt
            .query_map(p, |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter()
            .map(|(id, kind, target, created_at, rotated_at, revoked_at)| {
                Ok(ShareRow {
                    id,
                    target: ShareTarget::parse(&kind, &target)?,
                    created_at,
                    rotated_at,
                    revoked_at,
                })
            })
            .collect()
    }

    /// All share links, newest first.
    pub fn shares(&self) -> Result<Vec<ShareRow>> {
        self.shares_where("ORDER BY id DESC", [])
    }

    /// The active share link with this token hash.
    pub fn share_by_hash(&self, token_hash: &str) -> Result<Option<ShareRow>> {
        Ok(self
            .shares_where("WHERE token_hash = ?1 AND revoked_at IS NULL", [token_hash])?
            .into_iter()
            .next())
    }

    /// Revokes a share link, with its audit entry. Returns false if it is not active.
    pub fn revoke_share(&self, id: i64, now: i64, audit: &NewAudit) -> Result<bool> {
        self.update_audited(
            "UPDATE share_links SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now],
            audit,
        )
    }

    /// Gives an active share link a new token. Returns false if it is not active.
    pub fn rotate_share(&self, id: i64, token_hash: &str, now: i64) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE share_links SET token_hash = ?2, rotated_at = ?3 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, token_hash, now],
        )?;
        Ok(n == 1)
    }

    // ---- audit ------------------------------------------------------------------------------------

    /// Appends an audit entry and drops the oldest beyond [`AUDIT_KEEP`].
    pub fn audit(&self, at: i64, kind: &str, ip: Option<&str>, detail: &str) -> Result<()> {
        insert_audit(&self.conn(), at, kind, ip, detail)
    }

    /// The newest audit entries, newest first.
    pub fn audit_page(&self, limit: usize) -> Result<Vec<AuditRow>> {
        self.audit_before(None, limit)
    }

    /// The newest audit entries older than the entry `before` (all entries when `None`), newest
    /// first.
    pub fn audit_before(&self, before: Option<i64>, limit: usize) -> Result<Vec<AuditRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT id, at, kind, ip, detail FROM audit WHERE id < ?2 ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(
                params![
                    i64::try_from(limit).unwrap_or(i64::MAX),
                    before.unwrap_or(i64::MAX)
                ],
                |r| {
                    Ok(AuditRow {
                        id: r.get(0)?,
                        at: r.get(1)?,
                        kind: r.get(2)?,
                        ip: r.get(3)?,
                        detail: r.get(4)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, AuthStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite3");
        drop(crate::db::open_writer(&path).unwrap());
        let s = AuthStore::open(&path).unwrap();
        (dir, s)
    }

    #[test]
    fn ceremonies_are_single_use_bound_and_expire() {
        let (_d, s) = store();
        s.put_ceremony("a", "login", None, "{}", 100, 0).unwrap();
        assert_eq!(
            s.take_ceremony("a", "login", None, 50).unwrap().as_deref(),
            Some("{}")
        );
        assert_eq!(
            s.take_ceremony("a", "login", None, 50).unwrap(),
            None,
            "used"
        );
        s.put_ceremony("b", "login", None, "{}", 100, 0).unwrap();
        assert_eq!(
            s.take_ceremony("b", "login", None, 100).unwrap(),
            None,
            "expired"
        );
        s.put_ceremony("c", "step_up", Some("s1"), "{}", 100, 0)
            .unwrap();
        assert_eq!(
            s.take_ceremony("c", "step_up", Some("s2"), 1).unwrap(),
            None,
            "other session"
        );
        s.put_ceremony("d", "step_up", Some("s1"), "{}", 100, 0)
            .unwrap();
        assert_eq!(
            s.take_ceremony("d", "login", Some("s1"), 1).unwrap(),
            None,
            "other kind"
        );
    }

    #[test]
    fn one_active_token_per_agent() {
        let (_d, s) = store();
        let a = AgentId::new("bot").unwrap();
        let g = Grants::default();
        let id = s.add_agent(&a, "h1", &g, 10, None, 0).unwrap().unwrap();
        assert_eq!(s.add_agent(&a, "h2", &g, 10, None, 0).unwrap(), None);
        let audit = NewAudit {
            at: 1,
            kind: "agent_token_revoked",
            ip: None,
            detail: "{}".into(),
        };
        assert!(s.revoke_agent(id, 1, &audit).unwrap());
        assert!(!s.revoke_agent(id, 1, &audit).unwrap());
        assert_eq!(
            s.audit_page(10).unwrap().len(),
            1,
            "audited once, with the change"
        );
        assert!(s.add_agent(&a, "h3", &g, 10, None, 2).unwrap().is_some());
        assert_eq!(s.agents().unwrap().len(), 2);
    }

    #[test]
    fn audit_rows_are_kept_newest_first() {
        let (_d, s) = store();
        s.audit(1, "sign_in", Some("127.0.0.1"), "{}").unwrap();
        s.audit(2, "sign_out", None, "{}").unwrap();
        let rows = s.audit_page(10).unwrap();
        assert_eq!(rows[0].kind, "sign_out");
        assert_eq!(rows[1].ip.as_deref(), Some("127.0.0.1"));
        let older = s.audit_before(Some(rows[0].id), 10).unwrap();
        assert_eq!(older, rows[1..].to_vec());
        assert!(s.audit_before(Some(rows[1].id), 10).unwrap().is_empty());
        assert_eq!(s.audit_before(None, 1).unwrap(), rows[..1].to_vec());
    }
}
