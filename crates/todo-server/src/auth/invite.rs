//! Bounded, in-memory invitations. The link secret, typed code and browser ceremony handle are
//! independent credentials, stored only as hashes. A browser ceremony does not reserve the invite:
//! cancellation and native redemption can still win until verified registration claims it.

use super::pairing::{PAIRING_TTL_MS, format_code};
use super::secret::{self, RandomError};
use serde::Serialize;
use std::sync::{Mutex, MutexGuard, PoisonError};
use todo_core::DeviceId;
use todo_core::pairing::{INVITE_CODE_LEN, INVITE_SECRET_LEN};
use webauthn_rs::prelude::PasskeyRegistration;

const CAPACITY: usize = 32;

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub(crate) enum Issuer {
    Device(DeviceId),
    Session(String),
}

pub(crate) struct Created {
    pub id: String,
    pub code: String,
    pub secret: String,
    pub expires_in: u32,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Status {
    Pending,
    Used { device_name: String },
    Cancelled,
}

struct Entry {
    id: String,
    issuer: Issuer,
    expires: i64,
    link_hash: [u8; 32],
    code_hash: [u8; 32],
    claimed: bool,
    status: Status,
    browser: Option<Browser>,
}

struct Browser {
    handle: [u8; 32],
    registration: PasskeyRegistration,
    name: String,
}

pub(crate) struct Claim {
    pub id: String,
    pub issuer: Issuer,
}

#[derive(Default)]
pub(crate) struct Invites(Mutex<Vec<Entry>>);

impl Invites {
    fn entries(&self, now: i64) -> MutexGuard<'_, Vec<Entry>> {
        let mut entries = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|e| e.expires > now);
        entries
    }

    pub fn create(&self, issuer: Issuer, now: i64) -> Result<Option<Created>, RandomError> {
        let mut entries = self.entries(now);
        if entries.len() >= CAPACITY {
            return Ok(None);
        }
        let id = secret::random_token::<16>()?;
        let link = secret::random_token::<32>()?;
        let code = loop {
            let code = secret::random_code(INVITE_CODE_LEN)?;
            let hash = secret::digest(&code);
            if !entries.iter().any(|e| secret::ct_eq(&e.code_hash, &hash)) {
                break code;
            }
        };
        entries.push(Entry {
            id: id.clone(),
            issuer,
            expires: now + PAIRING_TTL_MS,
            link_hash: secret::digest(&link),
            code_hash: secret::digest(&code),
            claimed: false,
            status: Status::Pending,
            browser: None,
        });
        Ok(Some(Created {
            id,
            code: format_code(&code),
            secret: link,
            expires_in: (PAIRING_TTL_MS / 1000) as u32,
        }))
    }

    pub fn status(&self, issuer: &Issuer, id: &str, now: i64) -> Option<Status> {
        self.entries(now)
            .iter()
            .find(|e| e.id == id && &e.issuer == issuer)
            .map(|e| e.status.clone())
    }

    pub fn cancel(&self, issuer: &Issuer, id: &str, now: i64) -> bool {
        let mut entries = self.entries(now);
        let Some(entry) = entries
            .iter_mut()
            .find(|e| e.id == id && &e.issuer == issuer && e.available())
        else {
            return false;
        };
        entry.status = Status::Cancelled;
        entry.browser = None;
        true
    }

    pub fn redeem(&self, input: &str, now: i64) -> Option<Claim> {
        let mut entries = self.entries(now);
        let entry = find_secret(&mut entries, input, true)?;
        Some(entry.claim())
    }

    pub fn browser_issuer(&self, input: &str, now: i64) -> Option<Issuer> {
        find_secret(&mut self.entries(now), input, false).map(|entry| entry.issuer.clone())
    }

    pub fn start_browser(
        &self,
        input: &str,
        registration: PasskeyRegistration,
        name: String,
        now: i64,
    ) -> Result<Option<String>, RandomError> {
        let mut entries = self.entries(now);
        let Some(e) = find_secret(&mut entries, input, false) else {
            return Ok(None);
        };
        let handle = secret::random_token::<32>()?;
        e.browser = Some(Browser {
            handle: secret::digest(&handle),
            registration,
            name,
        });
        Ok(Some(handle))
    }

    pub fn take_browser(
        &self,
        handle: &str,
        now: i64,
    ) -> Option<(String, PasskeyRegistration, String)> {
        if handle.len() != INVITE_SECRET_LEN {
            return None;
        }
        let hash = secret::digest(handle);
        let mut entries = self.entries(now);
        let mut found = None;
        for e in entries.iter_mut() {
            if e.available()
                && e.browser
                    .as_ref()
                    .is_some_and(|b| secret::ct_eq(&hash, &b.handle))
            {
                found = Some(e);
            }
        }
        let e = found?;
        let browser = e.browser.take()?;
        Some((e.id.clone(), browser.registration, browser.name))
    }

    pub fn claim_browser(&self, id: &str, now: i64) -> Option<Claim> {
        self.entries(now)
            .iter_mut()
            .find(|e| e.id == id && e.available())
            .map(Entry::claim)
    }

    pub fn finish(&self, claim: &Claim, used_name: Option<String>, now: i64) {
        if let Some(e) = self.entries(now).iter_mut().find(|e| e.id == claim.id) {
            e.status = used_name.map_or(Status::Cancelled, |device_name| Status::Used {
                device_name,
            });
        }
    }
}

impl Entry {
    fn available(&self) -> bool {
        !self.claimed && matches!(self.status, Status::Pending)
    }

    fn claim(&mut self) -> Claim {
        self.claimed = true;
        self.browser = None;
        Claim {
            id: self.id.clone(),
            issuer: self.issuer.clone(),
        }
    }
}

fn find_secret<'a>(entries: &'a mut [Entry], input: &str, typed: bool) -> Option<&'a mut Entry> {
    if input.len() > 64 {
        return None;
    }
    let link_hash = secret::digest(input);
    let code = secret::normalize_code(input);
    let code_hash = secret::digest(&code);
    let mut found = None;
    // Compare both fixed-size digests against the whole bounded table; no early match exit.
    for entry in entries {
        let link = secret::ct_eq(&entry.link_hash, &link_hash) & (input.len() == INVITE_SECRET_LEN);
        let code_match =
            secret::ct_eq(&entry.code_hash, &code_hash) & typed & (code.len() == INVITE_CODE_LEN);
        if (link | code_match) && entry.available() {
            found = Some(entry);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer() -> Issuer {
        Issuer::Device(DeviceId::new("issuer").unwrap())
    }

    #[test]
    fn expires_at_the_boundary_and_both_credentials_share_one_use() {
        let invites = Invites::default();
        let invite = invites.create(issuer(), 0).unwrap().unwrap();
        assert_eq!(invite.secret.len(), 43);
        let claim = invites.redeem(&invite.code.to_lowercase(), 1).unwrap();
        assert!(invites.redeem(&invite.secret, 2).is_none());
        assert!(!invites.cancel(&issuer(), &invite.id, 2));
        invites.finish(&claim, Some("Laptop".into()), 3);
        assert!(
            matches!(invites.status(&issuer(), &invite.id, 3), Some(Status::Used { device_name }) if device_name == "Laptop")
        );
        assert!(
            invites
                .status(&issuer(), &invite.id, PAIRING_TTL_MS)
                .is_none()
        );
        let fresh = invites.create(issuer(), 0).unwrap().unwrap();
        assert!(invites.redeem(&fresh.secret, PAIRING_TTL_MS).is_none());
    }

    #[test]
    fn only_issuer_can_cancel_and_the_table_is_bounded() {
        let invites = Invites::default();
        let invite = invites.create(issuer(), 0).unwrap().unwrap();
        let other = Issuer::Session("different".into());
        assert!(!invites.cancel(&other, &invite.id, 1));
        assert!(invites.status(&other, &invite.id, 1).is_none());
        assert!(invites.cancel(&issuer(), &invite.id, 1));
        assert!(invites.redeem(&invite.secret, 1).is_none());
        assert!(invites.redeem(&invite.code, 1).is_none());
        for _ in 1..CAPACITY {
            assert!(invites.create(issuer(), 1).unwrap().is_some());
        }
        assert!(invites.create(issuer(), 1).unwrap().is_none());
        assert!(invites.create(issuer(), PAIRING_TTL_MS).unwrap().is_some());
    }
}
