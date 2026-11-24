//! Session-scoped Web Push subscriptions and durable per-occurrence delivery claims.

use super::{AuthStore, Result};
use crate::web_push::{MAX_SUBSCRIPTIONS, Subscription, Target};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

pub(crate) enum Registered {
    Stored,
    MissingSession,
    AtCapacity,
}

const TEST_INTERVAL_MS: i64 = 30_000;

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
        let live: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM sessions WHERE id_hash = ?1)",
            [session],
            |r| r.get(0),
        )?;
        if !live {
            return Ok(Registered::MissingSession);
        }
        let endpoint = target.endpoint.as_str();
        let same: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM web_push_subscriptions
             WHERE endpoint = ?1 AND session = ?2 AND vapid_key = ?3 AND p256dh = ?4 AND auth = ?5)",
            params![endpoint, session, key, &target.p256dh[..], &target.auth[..]], |r| r.get(0),
        )?;
        if same {
            return Ok(Registered::Stored);
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM web_push_subscriptions WHERE session = ?1 AND endpoint <> ?2 AND vapid_key = ?3",
            params![session, endpoint, key], |r| r.get(0),
        )?;
        if count >= MAX_SUBSCRIPTIONS {
            return Ok(Registered::AtCapacity);
        }
        // The endpoint belongs to a browser registration, not to a cookie. An explicit opt-in can
        // move it to a new session. Give it a fresh row so an in-flight 410 can't delete it.
        tx.execute(
            "DELETE FROM web_push_subscriptions WHERE endpoint = ?1 OR vapid_key <> ?2",
            params![endpoint, key],
        )?;
        tx.execute(
            "INSERT INTO web_push_subscriptions(session, endpoint, p256dh, auth, vapid_key, registered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![session, endpoint, &target.p256dh[..], &target.auth[..], key, now],
        )?;
        tx.commit()?;
        Ok(Registered::Stored)
    }

    pub(crate) fn has_web_push(&self, session: &str, endpoint: &str, key: &str) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM web_push_subscriptions WHERE session = ?1 AND endpoint = ?2 AND vapid_key = ?3)",
            params![session, endpoint, key], |r| r.get(0),
        )?)
    }

    pub(crate) fn unregister_web_push(&self, session: &str, endpoint: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM web_push_subscriptions WHERE session = ?1 AND endpoint = ?2",
            params![session, endpoint],
        )?;
        Ok(())
    }

    pub(crate) fn remove_web_push(&self, id: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM web_push_subscriptions WHERE id = ?1", [id])?;
        Ok(())
    }

    pub(crate) fn prune_web_push(&self, idle: i64, absolute: i64, old_alarm: i64) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM web_push_subscriptions WHERE session IN
             (SELECT id_hash FROM sessions WHERE last_seen_at <= ?1 OR created_at <= ?2)",
            params![idle, absolute],
        )?;
        conn.execute("DELETE FROM web_push_claims WHERE at < ?1", [old_alarm])?;
        Ok(())
    }

    pub(crate) fn unclaimed_web_push(&self, key: &str, task: &str, at: i64) -> Result<Vec<i64>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM web_push_subscriptions s WHERE vapid_key = ?1 AND registered_at <= ?3
             AND NOT EXISTS (SELECT 1 FROM web_push_claims WHERE subscription = s.id AND task = ?2 AND at = ?3)",
        )?;
        Ok(stmt
            .query_map(params![key, task, at], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn claim_web_push(
        &self,
        id: i64,
        task: &str,
        at: i64,
        idle: i64,
        absolute: i64,
    ) -> Result<Option<Target>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let target = tx
            .query_row(
                "SELECT p.id, p.endpoint, p.p256dh, p.auth FROM web_push_subscriptions p
             JOIN sessions s ON p.session = s.id_hash
             WHERE p.id = ?1 AND s.last_seen_at > ?2 AND s.created_at > ?3",
                params![id, idle, absolute],
                target,
            )
            .optional()?;
        if target.is_some()
            && tx.execute(
                "INSERT INTO web_push_claims(subscription, task, at) VALUES (?1, ?2, ?3)
             ON CONFLICT(subscription, task, at) DO NOTHING",
                params![id, task, at],
            )? == 1
        {
            tx.commit()?;
            Ok(target)
        } else {
            Ok(None)
        }
    }

    pub(crate) fn claim_web_push_test(
        &self,
        session: &str,
        endpoint: &str,
        key: &str,
        now: i64,
    ) -> Result<Option<Target>> {
        Ok(self.conn().query_row(
            "UPDATE web_push_subscriptions SET tested_at = ?4 WHERE session = ?1 AND endpoint = ?2 AND vapid_key = ?3
             AND (tested_at IS NULL OR tested_at <= ?5) RETURNING id, endpoint, p256dh, auth",
            params![session, endpoint, key, now, now.saturating_sub(TEST_INTERVAL_MS)], target,
        ).optional()?)
    }
}

fn target(r: &rusqlite::Row<'_>) -> rusqlite::Result<Target> {
    Ok(Target {
        id: r.get(0)?,
        endpoint: r.get(1)?,
        p256dh: r.get(2)?,
        auth: r.get(3)?,
    })
}
