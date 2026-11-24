//! Authenticated encryption for the single owner's assistant credentials and sharing settings.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    path::{Path, PathBuf},
};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use todo_core::ids::ListId;
use todo_tools::assistant::{access::AccessRules, relay::Account};
use uuid::Uuid;
use zeroize::Zeroizing;

const FORMAT: &[u8] = b"todo-assistant-v1\0";
const NONCE_LEN: usize = 24;
const LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("assistant vault I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("assistant key file must contain exactly 32 bytes in hex")]
    BadKey,
    #[error("assistant vault authentication or decoding failed")]
    Invalid,
    #[error("assistant vault exceeds its size limit")]
    TooLarge,
    #[error("system randomness is unavailable")]
    Random,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub(super) enum Flow {
    Device,
    Loopback,
}

/// No Debug impl: serde is used only inside the encrypted vault.
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Credential {
    pub flow: Flow,
    pub client_id: String,
    pub subject: String,
    pub workspace: Option<String>,
    pub label: String,
    pub access: String,
    pub refresh: String,
    pub expires_at: u64,
    pub scope: Option<String>,
    #[serde(default)]
    pub approved: bool,
    pub approved_lists: BTreeSet<ListId>,
}

impl Drop for Credential {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access.zeroize();
        self.refresh.zeroize();
    }
}

impl Credential {
    pub fn public(&self, id: Uuid) -> Account {
        Account {
            id,
            label: self.label.clone(),
            approved: self.approved,
            approved_lists: self.approved_lists.clone(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Records {
    pub host: Uuid,
    pub version: Uuid,
    pub access: AccessRules,
    pub accounts: BTreeMap<Uuid, Credential>,
}

impl Default for Records {
    fn default() -> Self {
        Self {
            host: Uuid::new_v4(),
            version: Uuid::new_v4(),
            access: AccessRules::default(),
            accounts: BTreeMap::new(),
        }
    }
}

pub(super) struct Vault {
    path: PathBuf,
    cipher: XChaCha20Poly1305,
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut value = [0; N];
    getrandom::fill(&mut value).map_err(|_| Error::Random)?;
    Ok(value)
}

fn load_key(path: &Path, may_create: bool) -> Result<Zeroizing<[u8; 32]>, Error> {
    let bytes = match todo_fs::read_private_file(path, 128) {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound && may_create => {
            let temp = path.with_extension(format!("{}.new", Uuid::new_v4()));
            let key = Zeroizing::new(random::<32>()?);
            let encoded = Zeroizing::new(hex::encode(*key));
            let result = (|| -> Result<(), io::Error> {
                let mut file = todo_fs::create_private_file(&temp)?;
                file.write_all(encoded.as_bytes())?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                drop(file);
                // Publish the complete key without ever replacing a key another process created.
                std::fs::hard_link(&temp, path)
            })();
            let _ = std::fs::remove_file(&temp);
            match result {
                Ok(()) => {
                    #[cfg(unix)]
                    std::fs::File::open(
                        path.parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or_else(|| Path::new(".")),
                    )?
                    .sync_all()?;
                    return Ok(key);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return load_key(path, false),
                Err(e) => return Err(e.into()),
            }
        }
        Err(e) => return Err(e.into()),
    };
    let mut key = Zeroizing::new([0; 32]);
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::BadKey)?;
    hex::decode_to_slice(text.trim(), &mut *key).map_err(|_| Error::BadKey)?;
    Ok(key)
}

impl Vault {
    pub fn open(dir: &Path, key_path: &Path) -> Result<(Self, Records), Error> {
        let path = dir.join("assistant.vault");
        let sealed = match todo_fs::read_private_file(&path, LIMIT) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let key = load_key(key_path, sealed.is_none())?;
        let vault = Self {
            path,
            cipher: XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| Error::BadKey)?,
        };
        let records = match sealed {
            Some(bytes) => vault.unseal(&bytes)?,
            None => {
                let records = Records::default();
                vault.save(&records)?;
                records
            }
        };
        Ok((vault, records))
    }

    pub fn save(&self, records: &Records) -> Result<(), Error> {
        let nonce = random::<NONCE_LEN>()?;
        let json = Zeroizing::new(serde_json::to_vec(records).map_err(|_| Error::Invalid)?);
        let sealed = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: json.as_ref(),
                    aad: FORMAT,
                },
            )
            .map_err(|_| Error::Invalid)?;
        if (FORMAT.len() + NONCE_LEN + sealed.len()) as u64 > LIMIT {
            return Err(Error::TooLarge);
        }
        let temp = self.path.with_extension(format!("{}.new", Uuid::new_v4()));
        let result = (|| -> Result<(), io::Error> {
            let mut file = todo_fs::create_private_file(&temp)?;
            file.write_all(FORMAT)?;
            file.write_all(&nonce)?;
            file.write_all(&sealed)?;
            file.sync_all()?;
            drop(file);
            todo_fs::replace_durably(&temp, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        Ok(result?)
    }

    fn unseal(&self, bytes: &[u8]) -> Result<Records, Error> {
        let rest = bytes.strip_prefix(FORMAT).ok_or(Error::Invalid)?;
        let (nonce, sealed) = rest.split_at_checked(NONCE_LEN).ok_or(Error::Invalid)?;
        let json = Zeroizing::new(
            self.cipher
                .decrypt(
                    XNonce::from_slice(nonce),
                    Payload {
                        msg: sealed,
                        aad: FORMAT,
                    },
                )
                .map_err(|_| Error::Invalid)?,
        );
        serde_json::from_slice(&json).map_err(|_| Error::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_round_trips_and_refuses_tampering_or_a_missing_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("assistant.key");
        let (vault, mut data) = Vault::open(dir.path(), &key_path).unwrap();
        let id = Uuid::new_v4();
        data.accounts.insert(
            id,
            Credential {
                flow: Flow::Device,
                client_id: "example".into(),
                subject: "subject".into(),
                workspace: None,
                label: "Example account".into(),
                access: "fixture-access-value".into(),
                refresh: "fixture-refresh-value".into(),
                expires_at: 123,
                scope: None,
                approved: false,
                approved_lists: BTreeSet::new(),
            },
        );
        vault.save(&data).unwrap();
        let mut bytes = std::fs::read(&vault.path).unwrap();
        assert!(!bytes.windows(20).any(|b| b == b"fixture-access-value"));
        let (_, reloaded) = Vault::open(dir.path(), &key_path).unwrap();
        assert_eq!(reloaded.accounts[&id].refresh, "fixture-refresh-value");
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&vault.path, bytes).unwrap();
        assert!(matches!(
            Vault::open(dir.path(), &key_path),
            Err(Error::Invalid)
        ));
        std::fs::remove_file(&key_path).unwrap();
        assert!(Vault::open(dir.path(), &key_path).is_err());
        assert!(!key_path.exists());
    }
}
