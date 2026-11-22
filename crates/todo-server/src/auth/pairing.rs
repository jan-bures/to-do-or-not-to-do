//! Device pairing requests, in the style of `gh auth login`.
//!
//! A client starts a pairing and gets a short user code plus a long secret poll token. The owner
//! opens the verification page, signs in and approves or denies; the client polls with its token and,
//! once approved, receives its device token exactly once.
//!
//! Requests live in memory only: they last [`PAIRING_TTL_MS`], and a restart simply cancels them. At
//! most [`MAX_PENDING`] are pending at once, and at most a configured number per client address
//! (`auth.pairings_per_ip`), so one address cannot fill the table and block real pairings. The
//! poll token is stored as its hash. The device and its token are created when the approved client
//! polls, so the plain token never sits in memory or on disk waiting to be fetched.

use super::secret::{self, RandomError};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// How long a pairing request stays valid.
pub const PAIRING_TTL_MS: i64 = 5 * 60 * 1000;
/// Length of a user code.
pub const USER_CODE_LEN: usize = todo_core::pairing::APPROVAL_CODE_LEN;
/// Most pending requests at once.
pub const MAX_PENDING: usize = 64;
/// Longest accepted device name, in characters.
pub const MAX_DEVICE_NAME: usize = 64;
/// Seconds a client should wait between polls.
pub const POLL_INTERVAL_SECS: u32 = 5;

/// Where a request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Waiting for the owner.
    Pending,
    /// Approved; the next poll creates the device.
    Approved,
    /// Denied; the next poll says so.
    Denied,
}

/// A pairing request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    /// The user code, without the dash.
    pub user_code: String,
    /// The device name the client supplied.
    pub device_name: String,
    /// When it was started.
    pub created_at: i64,
    /// When it expires.
    pub expires_at: i64,
    /// Where it stands.
    pub status: Status,
    /// The client address that started it.
    pub source: Option<IpAddr>,
    /// The issuer that approved it, if any.
    pub issued_by: Option<String>,
}

/// A newly started pairing, as returned to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The user code, formatted `XXXX-XXXX`.
    pub user_code: String,
    /// The secret poll token.
    pub poll_token: String,
    /// Expiry.
    pub expires_at: i64,
}

/// What a poll found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Still waiting.
    Pending,
    /// Approved: the caller now creates the device. The request is gone.
    Approved {
        /// The device name.
        device_name: String,
        /// The device or session that approved the pairing.
        issued_by: Option<String>,
    },
    /// Denied. The request is gone.
    Denied,
    /// Unknown, expired or already finished.
    Gone,
}

/// Why a pairing could not be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The device name is empty, too long or has control characters.
    #[error("the device name must be 1 to {MAX_DEVICE_NAME} printable characters")]
    BadName,
    /// Too many requests are pending.
    #[error("too many pairing requests are pending; try again in a few minutes")]
    Full,
    /// Too many requests from this address are pending.
    #[error("this address has too many pending pairing requests; finish or wait for them")]
    TooManyFromSource,
    /// No randomness.
    #[error(transparent)]
    Random(#[from] RandomError),
}

#[derive(Debug, Default)]
struct Inner {
    /// Poll-token hash -> request.
    by_poll: HashMap<[u8; 32], Pairing>,
    /// User code -> poll-token hash.
    by_code: HashMap<String, [u8; 32]>,
}

impl Inner {
    fn code_hash(&self, code: &str) -> Option<[u8; 32]> {
        let mut found = None;
        for (candidate, hash) in &self.by_code {
            if secret::ct_eq(candidate.as_bytes(), code.as_bytes()) {
                found = Some(*hash);
            }
        }
        found
    }

    fn poll_hash(&self, hash: &[u8; 32]) -> Option<[u8; 32]> {
        let mut found = None;
        for candidate in self.by_poll.keys() {
            if secret::ct_eq(candidate, hash) {
                found = Some(*candidate);
            }
        }
        found
    }

    fn purge(&mut self, now: i64) {
        let expired: Vec<[u8; 32]> = self
            .by_poll
            .iter()
            .filter(|(_, p)| p.expires_at <= now)
            .map(|(k, _)| *k)
            .collect();
        for k in expired {
            if let Some(p) = self.by_poll.remove(&k) {
                self.by_code.remove(&p.user_code);
            }
        }
    }
}

/// The pending pairing requests.
#[derive(Debug, Default)]
pub struct Pairings {
    inner: Mutex<Inner>,
}

/// Formats a user code as `XXXX-XXXX`.
#[must_use]
pub fn format_code(code: &str) -> String {
    let (a, b) = code.split_at(code.len() / 2);
    format!("{a}-{b}")
}

/// Checks a client-supplied device name.
pub fn valid_device_name(name: &str) -> Result<String, StartError> {
    let name = name.trim();
    let n = name.chars().count();
    if n == 0 || n > MAX_DEVICE_NAME || name.chars().any(char::is_control) {
        return Err(StartError::BadName);
    }
    Ok(name.to_owned())
}

impl Pairings {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a request from `source`, which may have at most `per_source` pending.
    pub fn start(
        &self,
        device_name: &str,
        source: Option<IpAddr>,
        per_source: u32,
        now: i64,
    ) -> Result<Started, StartError> {
        let device_name = valid_device_name(device_name)?;
        let mut inner = self.lock();
        inner.purge(now);
        if let Some(ip) = source {
            let mine = inner
                .by_poll
                .values()
                .filter(|p| p.source == Some(ip) && p.status == Status::Pending)
                .count();
            if mine >= usize::try_from(per_source).unwrap_or(usize::MAX) {
                return Err(StartError::TooManyFromSource);
            }
        }
        if inner.by_poll.len() >= MAX_PENDING {
            return Err(StartError::Full);
        }
        let poll_token = secret::random_token::<32>()?;
        let mut user_code = secret::random_code(USER_CODE_LEN)?;
        while inner.by_code.contains_key(&user_code) {
            user_code = secret::random_code(USER_CODE_LEN)?;
        }
        let hash = secret::digest(&poll_token);
        let expires_at = now + PAIRING_TTL_MS;
        inner.by_code.insert(user_code.clone(), hash);
        inner.by_poll.insert(
            hash,
            Pairing {
                user_code: user_code.clone(),
                device_name,
                created_at: now,
                expires_at,
                status: Status::Pending,
                source,
                issued_by: None,
            },
        );
        Ok(Started {
            user_code: format_code(&user_code),
            poll_token,
            expires_at,
        })
    }

    /// The pending request with this user code (as typed: dashes and case do not matter).
    #[must_use]
    pub fn pending(&self, user_code: &str, now: i64) -> Option<Pairing> {
        let code = secret::normalize_code(user_code);
        let mut inner = self.lock();
        inner.purge(now);
        let hash = inner.code_hash(&code)?;
        inner
            .by_poll
            .get(&hash)
            .filter(|p| p.status == Status::Pending)
            .cloned()
    }

    /// Approves or denies a pending request. Returns it, or `None` if there is no such pending request.
    /// A user code can be decided once.
    pub fn decide(&self, user_code: &str, approve: bool, now: i64) -> Option<Pairing> {
        self.decide_inner(user_code, approve, None, None, now)
    }

    /// Approves the request matching the code and creation time reviewed by the owner.
    pub(crate) fn approve_reviewed(
        &self,
        code: &str,
        created_at: Option<i64>,
        issuer: &super::invite::Issuer,
        now: i64,
    ) -> Option<Pairing> {
        self.decide_inner(code, true, created_at, Some(issuer), now)
    }

    fn decide_inner(
        &self,
        user_code: &str,
        approve: bool,
        created_at: Option<i64>,
        issuer: Option<&super::invite::Issuer>,
        now: i64,
    ) -> Option<Pairing> {
        let code = secret::normalize_code(user_code);
        let mut inner = self.lock();
        inner.purge(now);
        let hash = inner.code_hash(&code)?;
        let p = inner.by_poll.get_mut(&hash)?;
        if p.status != Status::Pending || created_at.is_some_and(|at| p.created_at != at) {
            return None;
        }
        p.issued_by = issuer.map(super::invite::Issuer::label);
        p.status = if approve {
            Status::Approved
        } else {
            Status::Denied
        };
        // Keep the code reserved until the request is purged or collected, so its cleanup
        // cannot remove a newer pairing that happened to get the same code.
        Some(p.clone())
    }

    /// Polls with a poll token. A decided request is removed by the poll that reports it, so the
    /// outcome (and the device token) is delivered exactly once.
    pub fn poll(&self, poll_token: &str, now: i64) -> Poll {
        let hash = secret::digest(poll_token);
        let mut inner = self.lock();
        inner.purge(now);
        let Some(hash) = inner.poll_hash(&hash) else {
            return Poll::Gone;
        };
        let Some(p) = inner.by_poll.get(&hash) else {
            return Poll::Gone;
        };
        match p.status {
            Status::Pending => Poll::Pending,
            Status::Approved | Status::Denied => {
                let Some(p) = inner.by_poll.remove(&hash) else {
                    return Poll::Gone;
                };
                inner.by_code.remove(&p.user_code);
                if p.status == Status::Approved {
                    Poll::Approved {
                        device_name: p.device_name,
                        issued_by: p.issued_by,
                    }
                } else {
                    Poll::Denied
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Pairings {
        fn start_any(&self, name: &str, now: i64) -> Result<Started, StartError> {
            self.start(name, None, u32::MAX, now)
        }
    }

    #[test]
    fn one_address_cannot_fill_the_table() {
        let p = Pairings::default();
        let a: IpAddr = "203.0.113.7".parse().unwrap();
        let b: IpAddr = "203.0.113.9".parse().unwrap();
        let mut first = None;
        for _ in 0..3 {
            first.get_or_insert(p.start("x", Some(a), 3, 0).unwrap());
        }
        assert!(matches!(
            p.start("x", Some(a), 3, 0),
            Err(StartError::TooManyFromSource)
        ));
        p.start("x", Some(b), 3, 0).unwrap();
        // A decided request no longer counts; an expired one is purged.
        let first = first.unwrap();
        p.decide(&first.user_code, false, 1).unwrap();
        p.start("x", Some(a), 3, 1).unwrap();
        p.start("x", Some(a), 3, PAIRING_TTL_MS + 1).unwrap();
    }

    #[test]
    fn approve_then_poll_once() {
        let p = Pairings::default();
        let s = p.start_any("Laptop", 0).unwrap();
        assert_eq!(s.user_code.len(), USER_CODE_LEN + 1);
        assert_eq!(p.poll(&s.poll_token, 1), Poll::Pending);
        let lower = s.user_code.to_lowercase();
        assert_eq!(p.pending(&lower, 1).unwrap().device_name, "Laptop");
        assert!(p.decide(&s.user_code, true, 2).is_some());
        assert!(p.decide(&s.user_code, true, 2).is_none(), "single use");
        assert!(p.pending(&s.user_code, 2).is_none());
        assert_eq!(
            p.poll(&s.poll_token, 3),
            Poll::Approved {
                device_name: "Laptop".into(),
                issued_by: None,
            }
        );
        assert_eq!(p.poll(&s.poll_token, 4), Poll::Gone, "delivered once");
    }

    #[test]
    fn requests_expire() {
        let p = Pairings::default();
        let s = p.start_any("Phone", 0).unwrap();
        assert!(p.decide(&s.user_code, true, PAIRING_TTL_MS).is_none());
        assert_eq!(p.poll(&s.poll_token, PAIRING_TTL_MS), Poll::Gone);
        let s = p.start_any("Phone", 0).unwrap();
        p.decide(&s.user_code, false, 1).unwrap();
        assert_eq!(p.poll(&s.poll_token, 2), Poll::Denied);
    }

    #[test]
    fn names_are_checked_and_pending_requests_are_bounded() {
        let p = Pairings::default();
        assert!(p.start_any("", 0).is_err());
        assert!(p.start_any("a\nb", 0).is_err());
        assert!(p.start_any(&"x".repeat(MAX_DEVICE_NAME + 1), 0).is_err());
        for _ in 0..MAX_PENDING {
            p.start_any("d", 0).unwrap();
        }
        assert!(matches!(p.start_any("d", 0), Err(StartError::Full)));
        p.start_any("d", PAIRING_TTL_MS).unwrap();
    }
}
