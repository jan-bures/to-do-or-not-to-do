//! One locally configured inference destination per pairing. No transcript or proposal on disk.

use crate::{
    chatgpt::DesktopProvider,
    config::DataDir,
    platform::{dpapi, tailnet::WindowsTailnet},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{ErrorKind, Write},
    sync::Arc,
};
use todo_client::assistant::{
    AccessRules, Selection,
    local::{EndpointInfo, LocalProvider, SetupError, Support, endpoint_info, valid_model},
};
use zeroize::Zeroizing;

const FILE: &str = "assistant-profile.bin";
const PURPOSE: &[u8] = b"to-do-or-not-to-do/assistant-profile/v1";
const MAX_PROFILE: u64 = 64 * 1024;

#[derive(Default, Serialize, Deserialize)]
pub struct Profile {
    pub url: String,
    pub model: String,
    pub enabled: bool,
    #[serde(default)]
    pub cloud: bool,
    #[serde(default)]
    pub attested: bool,
    #[serde(flatten)]
    pub access: AccessRules,
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
    pub fn selection(&self) -> Option<Selection<DesktopProvider>> {
        if self.cloud || !self.enabled || !valid_model(&self.model) {
            return None;
        }
        let mut provider = self.provider().ok()?;
        if let Some(support) = self.support {
            provider = provider.with_support(self.model.clone(), support);
        }
        Some(Selection {
            provider: Arc::new(DesktopProvider::Private(provider)),
            model: self.model.clone(),
            access: self.access.clone(),
            allow_billed: false,
        })
    }

    pub fn provider(&self) -> Result<LocalProvider, SetupError> {
        Ok(
            LocalProvider::configured(&self.url, &self.key, self.attested)?
                .with_tailnet_interface(WindowsTailnet),
        )
    }

    pub fn endpoint(&self) -> Option<EndpointInfo> {
        endpoint_info(&self.url, self.attested).ok()
    }

    pub fn change_url(&mut self, url: String) {
        if self.url != url {
            self.attested = false;
            self.key = Zeroizing::default();
            self.support = None;
            self.access = AccessRules::default();
            self.url = url;
        }
    }

    pub fn normalize(&mut self) -> Result<(), SetupError> {
        self.url = self.provider()?.base_url().into();
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

    pub fn exists(data: &DataDir) -> bool {
        data.check().is_ok()
            && (std::fs::symlink_metadata(data.path().join(FILE)).is_ok()
                || std::fs::read_dir(data.path()).ok().is_some_and(|entries| {
                    entries
                        .flatten()
                        .any(|entry| is_temporary(&entry.file_name()))
                }))
    }

    pub fn forget(data: &DataDir) -> anyhow::Result<()> {
        data.check()?;
        let mut failure = std::fs::remove_file(data.path().join(FILE))
            .err()
            .filter(|error| error.kind() != ErrorKind::NotFound);
        for entry in std::fs::read_dir(data.path())? {
            let entry = entry?;
            if is_temporary(&entry.file_name())
                && let Err(error) = std::fs::remove_file(entry.path())
                && error.kind() != ErrorKind::NotFound
            {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), |error| Err(error.into()))
    }
}

fn is_temporary(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.starts_with("assistant-")
            && !name.starts_with("assistant-chatgpt-")
            && name.ends_with(".tmp")
    })
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
            attested: true,
            key: Zeroizing::new("synthetic-secret".into()),
            ..Profile::default()
        };
        use todo_client::assistant::ListAccess;
        use todo_core::ids::{CollectionId, ListId};
        let collection = CollectionId::from_uuid(uuid::Uuid::from_u128(7));
        let list = ListId::from_uuid(uuid::Uuid::from_u128(8));
        profile
            .access
            .collections
            .insert(collection, ListAccess::PrivateOnly);
        profile.access.lists.insert(list, ListAccess::Hidden);
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
        assert_eq!(loaded.access, profile.access);
        assert!(loaded.attested);
        assert!(loaded.selection().is_none());
        Profile::forget(&data).expect("forget assistant profile");
        assert!(
            Profile::load(&data)
                .expect("default profile")
                .url
                .is_empty()
        );
    }

    #[test]
    fn forget_works_for_bad_or_empty_settings_and_orphaned_temporary_files() {
        let dir = tempfile::tempdir_in("target").expect("fixture profile directory");
        let data = Paths::in_dir(dir.path().to_owned()).data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("create fixture pairing");
        Profile {
            key: Zeroizing::new("fixture-key".into()),
            ..Profile::default()
        }
        .save(&data)
        .expect("save a key without URL");
        assert!(Profile::exists(&data));
        assert!(Profile::load(&data).expect("load fixture").url.is_empty());
        std::fs::write(data.path().join(FILE), b"not a protected record")
            .expect("fixture invalid profile");
        assert!(Profile::load(&data).is_err());
        let orphan = data.path().join("assistant-orphan.tmp");
        let other = data.path().join("unrelated.tmp");
        std::fs::write(&orphan, b"orphan").expect("fixture temporary file");
        std::fs::write(&other, b"unrelated").expect("fixture unrelated file");
        assert!(Profile::exists(&data));
        Profile::forget(&data).expect("forget corrupt profile and orphan");
        assert!(!Profile::exists(&data));
        assert!(!orphan.exists());
        assert!(other.exists());
        std::fs::write(&orphan, b"orphan only").expect("fixture temporary file without profile");
        assert!(Profile::exists(&data));
        Profile::forget(&data).expect("forget only the orphan");
        assert!(!Profile::exists(&data));
    }

    #[test]
    fn legacy_profiles_do_not_inherit_attestation() {
        use todo_client::assistant::local::Destination;
        let old = r#"{"url":"https://mac.tailnet.example/v1","model":"fixture-model","enabled":true,"lists":{"00000000-0000-0000-0000-000000000001":"Private","00000000-0000-0000-0000-000000000002":"All"},"support":null,"key":""}"#;
        let profile: Profile = serde_json::from_str(old).expect("legacy profile");
        assert!(profile.access.collections.is_empty());
        assert_eq!(
            profile.access.lists.values().copied().collect::<Vec<_>>(),
            [
                todo_client::assistant::ListAccess::PrivateOnly,
                todo_client::assistant::ListAccess::AllModels
            ]
        );
        assert!(!profile.attested);
        assert_eq!(
            profile.endpoint().expect("legacy URL").destination,
            Destination::NotPrivate
        );
    }

    #[tokio::test]
    async fn changing_url_cannot_send_the_saved_key_to_the_new_host() {
        use todo_client::assistant::provider::Provider;
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        async fn server(host: &str) -> (String, tokio::task::JoinHandle<String>) {
            let listener = TcpListener::bind((host, 0))
                .await
                .expect("bind fixture server");
            let address = format!(
                "http://{host}:{}/v1",
                listener
                    .local_addr()
                    .expect("fixture listener address")
                    .port()
            );
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept fixture request");
                let mut data = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let n = stream.read(&mut buf).await.expect("read fixture request");
                    assert!(n > 0 && data.len() < 8192);
                    data.extend_from_slice(&buf[..n]);
                    if data.windows(4).any(|chunk| chunk == b"\r\n\r\n") {
                        break;
                    }
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}").await.expect("send fixture catalog");
                String::from_utf8(data)
                    .expect("fixture HTTP header")
                    .to_ascii_lowercase()
            });
            (address, task)
        }
        let (old, old_request) = server("127.0.0.1").await;
        let (new, new_request) = server("localhost").await;
        let mut profile = Profile {
            url: old,
            attested: true,
            key: Zeroizing::new("fixture-key".into()),
            ..Profile::default()
        };
        profile
            .provider()
            .expect("old provider")
            .list_models()
            .await
            .expect("old catalog");
        assert!(
            old_request
                .await
                .expect("old request")
                .contains("authorization: bearer fixture-key")
        );
        profile.change_url(new);
        assert!(!profile.attested && profile.key.is_empty());
        profile
            .provider()
            .expect("new provider")
            .list_models()
            .await
            .expect("new catalog");
        assert!(
            !new_request
                .await
                .expect("new request")
                .contains("authorization:")
        );
    }
}
