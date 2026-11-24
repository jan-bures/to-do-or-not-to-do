//! Browser-scoped Web Push subscriptions and durable per-occurrence delivery claims.

use super::{AuthStore, Result};
use crate::auth::secret;
use crate::web_push::{MAX_SUBSCRIPTIONS, Subscription, Target, inactivity_cutoff};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

pub(crate) enum Registered {
    Stored,
    MissingSession,
    AtCapacity,
}

pub(crate) struct Browser {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) current: bool,
    pub(crate) last_seen_at: i64,
}

const TEST_INTERVAL_MS: i64 = 30_000;

struct Binding<'a> {
    session: &'a str,
    passkey: i64,
    step_up: Option<i64>,
    logout_csrf: String,
}

impl<'a> Binding<'a> {
    fn load(conn: &Connection, session: &'a str) -> Result<Option<Self>> {
        Ok(conn
            .query_row(
                "SELECT passkey_id, step_up_passkey_id, csrf FROM sessions
             WHERE id_hash = ?1 AND passkey_id IS NOT NULL",
                [session],
                |r| {
                    Ok(Self {
                        session,
                        passkey: r.get(0)?,
                        step_up: r.get(1)?,
                        logout_csrf: secret::digest_hex(&r.get::<_, String>(2)?),
                    })
                },
            )
            .optional()?)
    }

    fn at_capacity(&self, conn: &Connection, key: &str, endpoint: &str) -> Result<bool> {
        Ok(conn.query_row(
            "SELECT count(*) FROM web_push_browsers b JOIN web_push_subscriptions p USING (id)
             WHERE b.session = ?1 AND p.endpoint <> ?2 AND p.vapid_key = ?3",
            params![self.session, endpoint, key],
            |r| r.get::<_, i64>(0),
        )? >= MAX_SUBSCRIPTIONS)
    }

    // Confirm only an existing, unexpired subscription with matching browser keys. Rebinding it
    // at the next sign-in makes sign-out work without resurrecting a revoked subscription or
    // resetting its original registration time, test rate limit, or delivery claims.
    fn confirm(
        &self,
        conn: &Connection,
        key: &str,
        target: &Subscription,
        now: i64,
    ) -> Result<bool> {
        let id = conn
            .query_row(
                "SELECT id FROM web_push_subscriptions WHERE endpoint = ?1 AND vapid_key = ?2
             AND p256dh = ?3 AND auth = ?4 AND last_seen_at > ?5",
                params![
                    target.endpoint.as_str(),
                    key,
                    &target.p256dh[..],
                    &target.auth[..],
                    inactivity_cutoff(now)
                ],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(false) };
        conn.execute(
            "UPDATE web_push_browsers SET session = ?2, passkey = ?3, step_up_passkey = ?4,
             logout_csrf = ?5 WHERE id = ?1",
            params![
                id,
                self.session,
                self.passkey,
                self.step_up,
                self.logout_csrf
            ],
        )?;
        conn.execute(
            "UPDATE web_push_subscriptions SET last_seen_at = max(last_seen_at, ?2) WHERE id = ?1",
            params![id, now],
        )?;
        Ok(true)
    }
}

impl AuthStore {
    pub(crate) fn register_web_push(
        &self,
        session: &str,
        key: &str,
        target: &Subscription,
        now: i64,
    ) -> Result<Registered> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(binding) = Binding::load(&tx, session)? else {
            return Ok(Registered::MissingSession);
        };
        let endpoint = target.endpoint.as_str();
        if binding.at_capacity(&tx, key, endpoint)? {
            return Ok(Registered::AtCapacity);
        }
        if !binding.confirm(&tx, key, target, now)? {
            // Give changed keys a fresh row so an in-flight 410 cannot delete the new subscription.
            tx.execute(
                "DELETE FROM web_push_browsers WHERE id IN
                 (SELECT id FROM web_push_subscriptions WHERE endpoint = ?1 OR vapid_key <> ?2)",
                params![endpoint, key],
            )?;
            tx.execute(
                "INSERT INTO web_push_browsers(session, passkey, step_up_passkey, logout_csrf)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    session,
                    binding.passkey,
                    binding.step_up,
                    binding.logout_csrf
                ],
            )?;
            tx.execute(
                "INSERT INTO web_push_subscriptions(id, endpoint, p256dh, auth, vapid_key, registered_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![tx.last_insert_rowid(), endpoint, &target.p256dh[..], &target.auth[..], key, now],
            )?;
        }
        tx.commit()?;
        Ok(Registered::Stored)
    }

    pub(crate) fn confirm_web_push(
        &self,
        session: &str,
        key: &str,
        target: &Subscription,
        now: i64,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(binding) = Binding::load(&tx, session)? else {
            return Ok(false);
        };
        if binding.at_capacity(&tx, key, target.endpoint.as_str())? {
            return Ok(false);
        }
        let active = binding.confirm(&tx, key, target, now)?;
        tx.commit()?;
        Ok(active)
    }

    pub(crate) fn has_web_push(
        &self,
        session: &str,
        endpoint: &str,
        key: &str,
        now: i64,
    ) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM web_push_browsers b JOIN web_push_subscriptions p USING (id)
             WHERE b.session = ?1 AND p.endpoint = ?2 AND p.vapid_key = ?3 AND p.last_seen_at > ?4)",
            params![session, endpoint, key, inactivity_cutoff(now)], |r| r.get(0),
        )?)
    }

    pub(crate) fn unregister_web_push(&self, session: &str, endpoint: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM web_push_browsers WHERE session = ?1 AND id IN
             (SELECT id FROM web_push_subscriptions WHERE endpoint = ?2)",
            params![session, endpoint],
        )?;
        Ok(())
    }

    pub(crate) fn web_push_browsers(&self, session: &str, now: i64) -> Result<Vec<Browser>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT b.id, k.name, b.session = ?1, p.last_seen_at FROM web_push_browsers b
             JOIN web_push_subscriptions p USING (id) JOIN passkeys k ON k.id = b.passkey
             WHERE p.last_seen_at > ?2 ORDER BY p.last_seen_at DESC, b.id DESC",
        )?;
        Ok(stmt
            .query_map(params![session, inactivity_cutoff(now)], |r| {
                Ok(Browser {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    current: r.get(2)?,
                    last_seen_at: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn revoke_web_push_browser(&self, id: i64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(session) = tx
            .query_row(
                "SELECT session FROM web_push_browsers WHERE id = ?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        else {
            return Ok(false);
        };
        delete_browser_session(&tx, &session)?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn remove_web_push(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM web_push_browsers WHERE id = ?1", [id])?;
        Ok(())
    }

    pub(crate) fn prune_web_push(&self, now: i64, old_alarm: i64) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM web_push_browsers WHERE id IN
             (SELECT id FROM web_push_subscriptions WHERE last_seen_at <= ?1)",
            [inactivity_cutoff(now)],
        )?;
        conn.execute("DELETE FROM web_push_claims WHERE at < ?1", [old_alarm])?;
        Ok(())
    }

    /// Includes delayed retries so the scheduler knows when an alarm is truly finished.
    pub(crate) fn unclaimed_web_push(
        &self,
        key: &str,
        task: &str,
        at: i64,
        now: i64,
    ) -> Result<Vec<(i64, Option<i64>)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT p.id, c.retry_at FROM web_push_subscriptions p LEFT JOIN web_push_claims c
                ON c.subscription = p.id AND c.task = ?2 AND c.at = ?3
             WHERE p.vapid_key = ?1 AND p.registered_at <= ?3 AND p.last_seen_at > ?4
               AND (c.subscription IS NULL OR c.retry_at IS NOT NULL)",
        )?;
        Ok(stmt
            .query_map(params![key, task, at, inactivity_cutoff(now)], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn claim_web_push(
        &self,
        id: i64,
        task: &str,
        at: i64,
        now: i64,
    ) -> Result<Option<(Target, u32)>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(target) = tx
            .query_row(
                "SELECT id, endpoint, p256dh, auth FROM web_push_subscriptions
             WHERE id = ?1 AND last_seen_at > ?2",
                params![id, inactivity_cutoff(now)],
                target,
            )
            .optional()?
        else {
            return Ok(None);
        };
        let attempt = tx.query_row(
            "INSERT INTO web_push_claims(subscription, task, at) VALUES (?1, ?2, ?3)
             ON CONFLICT(subscription, task, at) DO UPDATE SET attempt = attempt + 1, retry_at = NULL
             WHERE retry_at <= ?4 RETURNING attempt",
            params![id, task, at, now], |r| r.get(0),
        ).optional()?;
        tx.commit()?;
        Ok(attempt.map(|attempt| (target, attempt)))
    }

    pub(crate) fn retry_web_push(&self, id: i64, task: &str, at: i64, retry_at: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE web_push_claims SET retry_at = ?4 WHERE subscription = ?1 AND task = ?2 AND at = ?3",
            params![id, task, at, retry_at],
        )?;
        Ok(())
    }

    pub(crate) fn claim_web_push_test(
        &self,
        session: &str,
        endpoint: &str,
        key: &str,
        now: i64,
    ) -> Result<Option<Target>> {
        Ok(self.conn().query_row(
            "UPDATE web_push_subscriptions SET tested_at = ?4 WHERE id IN (SELECT id FROM web_push_browsers WHERE session = ?1)
             AND endpoint = ?2 AND vapid_key = ?3 AND (tested_at IS NULL OR tested_at <= ?5)
             AND last_seen_at > ?6 RETURNING id, endpoint, p256dh, auth",
            params![session, endpoint, key, now, now.saturating_sub(TEST_INTERVAL_MS), inactivity_cutoff(now)], target,
        ).optional()?)
    }
}

pub(super) fn delete_browser_session(conn: &Connection, session: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM web_push_browsers WHERE session = ?1",
        [session],
    )?;
    conn.execute("DELETE FROM sessions WHERE id_hash = ?1", [session])?;
    Ok(())
}

fn target(r: &rusqlite::Row<'_>) -> rusqlite::Result<Target> {
    Ok(Target {
        id: r.get(0)?,
        endpoint: r.get(1)?,
        p256dh: r.get(2)?,
        auth: r.get(3)?,
    })
}
