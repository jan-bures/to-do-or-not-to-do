//! One locally configured inference destination per pairing. No transcript or proposal on disk.

use crate::{config::DataDir, platform::dpapi};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{ErrorKind, Write},
    sync::Arc,
};
use todo_client::assistant::{
    ListAccess, Selection,
    local::{LocalProvider, SetupError, Support, valid_model},
};
use todo_core::ids::ListId;
use zeroize::Zeroizing;

const FILE: &str = "assistant-profile.bin";
const PURPOSE: &[u8] = b"to-do-or-not-to-do/assistant-profile/v1";
const MAX_PROFILE: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    Private,
    All,
}

impl Access {
    pub fn name(value: Option<Self>) -> &'static str {
        match value {
            None => "Hidden",
            Some(Self::Private) => "Private only",
            Some(Self::All) => "All models",
        }
    }
    pub fn next(value: Option<Self>) -> Option<Self> {
        match value {
            None => Some(Self::Private),
            Some(Self::Private) => Some(Self::All),
            Some(Self::All) => None,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Profile {
    pub url: String,
    pub model: String,
    pub enabled: bool,
    pub lists: BTreeMap<ListId, Access>,
    pub support: Option<Support>,
    #[serde(with = "secret")]
    pub key: Zeroizing<String>,
}

mod secret {
    use super::*;
    pub fn serialize<S: serde::Serializer>(s: &str, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Zeroizing<String>, D::Error> {
        String::deserialize(d).map(Zeroizing::new)
    }
}

impl Profile {
    pub fn scope(&self) -> BTreeMap<ListId, ListAccess> {
        self.lists
            .iter()
            .map(|(&id, &access)| {
                (
                    id,
                    match access {
                        Access::All => ListAccess::AllModels,
                        Access::Private => ListAccess::PrivateOnly,
                    },
                )
            })
            .collect()
    }

    pub fn selection(&self) -> Option<Selection<LocalProvider>> {
        if !self.enabled || !valid_model(&self.model) {
            return None;
        }
        let mut provider = LocalProvider::new(&self.url, &self.key).ok()?;
        if let Some(support) = self.support {
            provider = provider.with_support(self.model.clone(), support);
        }
        Some(Selection {
            provider: Arc::new(provider),
            model: self.model.clone(),
            lists: self.scope(),
            allow_billed: false,
        })
    }

    pub fn normalize(&mut self) -> Result<(), SetupError> {
        self.url = LocalProvider::new(&self.url, &self.key)?.base_url().into();
        Ok(())
    }

    pub fn load(data: &DataDir) -> anyhow::Result<Self> {
        data.check()?;
        let sealed = match todo_fs::read_private_file(&data.path().join(FILE), MAX_PROFILE) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            other => other?,
        };
        let plain = Zeroizing::new(dpapi::open(&sealed, PURPOSE)?);
        // A malformed secret record is neither logged nor returned in a message.
        serde_json::from_slice(&plain)
            .map_err(|_| anyhow::anyhow!("Couldn't read assistant settings."))
    }

    pub fn save(&self, data: &DataDir) -> anyhow::Result<()> {
        data.check()?;
        let plain = Zeroizing::new(serde_json::to_vec(self)?);
        let sealed = dpapi::seal(&plain, PURPOSE)?;
        anyhow::ensure!(
            sealed.len() as u64 <= MAX_PROFILE,
            "assistant settings are too large"
        );
        let temporary = data
            .path()
            .join(format!("assistant-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = todo_fs::create_private_file(&temporary)?;
        let result = file.write_all(&sealed).and_then(|()| file.sync_all());
        drop(file);
        let result =
            result.and_then(|()| todo_fs::replace_durably(&temporary, &data.path().join(FILE)));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        Ok(result?)
    }

    pub fn forget(data: &DataDir) -> anyhow::Result<()> {
        data.check()?;
        match std::fs::remove_file(data.path().join(FILE)) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PairingId, Paths};

    #[test]
    fn endpoint_model_and_key_are_sealed_and_disabled_profiles_cannot_send() {
        let dir = tempfile::tempdir_in("target").expect("fixture profile directory");
        let paths = Paths::in_dir(dir.path().to_owned());
        let data = paths.data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("create fixture pairing");
        let mut profile = Profile {
            url: "https://mac.tailnet.example".into(),
            model: "sample-model".into(),
            key: Zeroizing::new("synthetic-secret".into()),
            ..Profile::default()
        };
        profile.normalize().expect("normalize fixture URL");
        profile.save(&data).expect("seal fixture profile");
        let sealed = std::fs::read(data.path().join(FILE)).expect("read ciphertext");
        for text in [&profile.url, &profile.model, &*profile.key] {
            assert!(!sealed.windows(text.len()).any(|w| w == text.as_bytes()));
        }
        assert!(dpapi::unprotect(&sealed).is_err());
        let loaded = Profile::load(&data).expect("open fixture profile");
        assert_eq!(*loaded.key, *profile.key);
        assert_eq!(loaded.url, profile.url);
        assert!(loaded.selection().is_none());
        Profile::forget(&data).expect("forget assistant profile");
        assert!(
            Profile::load(&data)
                .expect("default profile")
                .url
                .is_empty()
        );
    }
}
