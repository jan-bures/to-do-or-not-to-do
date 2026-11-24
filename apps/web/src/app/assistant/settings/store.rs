//! Per-owner settings and a separately scoped, optional tab credential.

use crate::platform;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use todo_client::assistant::{AccessRules, endpoint::bearer_key};
use uuid::Uuid;

const ACTIVE: &str = "todo.assistant.owner";
const PROFILE: &str = "todo.assistant.settings";
const API_KEY: &str = "todo.assistant.api-key";
const REVOKED: &str = "todo.assistant.revoked";

#[derive(Clone, PartialEq, Eq)]
pub(in crate::app::assistant) struct Scope(String);

impl Scope {
    fn parse(value: String) -> Option<Self> {
        (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())).then_some(Self(value))
    }

    pub(in crate::app::assistant) fn activate() -> Option<Self> {
        let mine = Self(platform::owner_scope()?);
        let storage = platform::local_storage()?;
        let previous = storage.get_item(ACTIVE).ok()?.and_then(Self::parse);
        if previous.as_ref() != Some(&mine) {
            if let Some(previous) = previous {
                let _ = previous.revoke();
            }
            storage.set_item(ACTIVE, &mine.0).ok()?;
        }
        // The unscoped preview build's profile had no authenticated owner. Never migrate it.
        let _ = storage.remove_item(PROFILE);
        Some(mine)
    }

    pub(in crate::app::assistant) fn active(&self) -> bool {
        platform::local_storage()
            .and_then(|s| s.get_item(ACTIVE).ok().flatten())
            .as_ref()
            == Some(&self.0)
    }

    fn key(&self, prefix: &str) -> String {
        format!("{prefix}:{}", self.0)
    }

    pub(super) fn epoch(&self) -> Option<Uuid> {
        platform::local_storage()?
            .get_item(&self.key(REVOKED))
            .ok()
            .flatten()?
            .parse()
            .ok()
    }

    pub(in crate::app::assistant) fn revoke(&self) -> Option<Uuid> {
        self.forget();
        let epoch = crate::backend::new_uuid();
        platform::local_storage()?
            .set_item(&self.key(REVOKED), &epoch.to_string())
            .ok()?;
        Some(epoch)
    }

    pub(super) fn forget(&self) {
        if let Some(storage) = session_storage() {
            let _ = storage.remove_item(&self.key(API_KEY));
        }
    }

    pub(super) fn remembered(&self, address: &str, epoch: Option<Uuid>) -> Credential {
        let stored = session_storage()
            .and_then(|s| s.get_item(&self.key(API_KEY)).ok().flatten())
            .and_then(|s| serde_json::from_str::<Credential>(&s).ok());
        match stored {
            Some(key)
                if key.matches(address, epoch)
                    && bearer_key(&key.value).is_some_and(|s| !s.is_empty()) =>
            {
                key
            }
            Some(_) => {
                self.forget();
                Credential::default()
            }
            None => Credential::default(),
        }
    }

    pub(super) fn remember(&self, key: &Credential) -> bool {
        serde_json::to_string(key).ok().is_some_and(|value| {
            session_storage().is_some_and(|s| s.set_item(&self.key(API_KEY), &value).is_ok())
        })
    }

    pub(in crate::app::assistant) fn relevant(&self, key: Option<&str>) -> bool {
        key.is_none_or(|key| key == ACTIVE || key == self.key(PROFILE) || key == self.key(REVOKED))
    }
}

/// An opaque login binding; never store the CSRF token itself alongside the credential.
pub(super) fn login_fingerprint() -> Option<[u8; 32]> {
    crate::backend::csrf().map(|csrf| {
        let mut digest = Sha256::new();
        digest.update(b"todo-assistant-login-v1\0");
        digest.update(csrf.as_bytes());
        digest.finalize().into()
    })
}

fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(in crate::app::assistant) struct Profile {
    pub(super) address: String,
    pub(super) model: String,
    pub(super) access: AccessRules,
}

impl Profile {
    pub(in crate::app::assistant) fn load(scope: &Scope) -> Self {
        platform::local_storage()
            .and_then(|s| s.get_item(&scope.key(PROFILE)).ok().flatten())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub(super) fn save(&self, scope: &Scope) -> bool {
        scope.active()
            && serde_json::to_string(self).ok().is_some_and(|value| {
                platform::local_storage()
                    .is_some_and(|s| s.set_item(&scope.key(PROFILE), &value).is_ok())
            })
    }
}

/// Never serializes into the profile, logs, or a model request body. When opted in, this exact
/// address and owner-bound value is written only to this tab's sessionStorage.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app::assistant) struct Credential {
    pub(super) address: String,
    pub(super) epoch: Option<Uuid>,
    pub(super) login: Option<[u8; 32]>,
    pub(super) value: String,
    pub(super) remember: bool,
}

impl Credential {
    pub(super) fn matches(&self, address: &str, epoch: Option<Uuid>) -> bool {
        self.address == address && self.epoch == epoch && self.login == login_fingerprint()
    }
}
