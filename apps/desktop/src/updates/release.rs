//! Signed desktop release metadata and the integrity check shared by staging and apply.

use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use velopack::{VelopackAsset, VelopackAssetFeed};

pub(super) const APP_ID: &str = "todo-desktop";
pub(super) const CHANNEL: &str = "win";
pub(super) const MAX_FEED: u64 = 1024 * 1024;
pub(super) const MAX_PROOF: u64 = 1024;
const MAX_PACKAGE: u64 = 256 * 1024 * 1024;
const DOMAIN: &str = "todo-desktop-update-v1\n";
const KEYS: &str = env!("TODO_DESKTOP_VERIFY_KEYS");

#[derive(Debug, thiserror::Error)]
#[error("desktop update verification failed")]
pub(super) struct Invalid;

/// Untyped bytes from the release host. No paths or package data may be used until verify succeeds.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Receipt {
    pub manifest: String,
    pub proof: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    tag: String,
    signature: String,
}

#[derive(Debug)]
pub(super) struct Release {
    pub version: Version,
    pub full: VelopackAsset,
}

pub(super) fn read_bounded(source: impl Read, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    source.take(limit + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn package_version(raw: &str) -> Option<Version> {
    let version = Version::parse(raw).ok()?;
    let height = version.pre.as_str().strip_prefix("build.")?;
    (version.build.is_empty() && height.parse::<u32>().is_ok_and(|n| n > 0)).then_some(version)
}

fn valid_asset(asset: &VelopackAsset) -> bool {
    let kind = match asset.Type.as_str() {
        "Full" => "full",
        "Delta" => "delta",
        _ => return false,
    };
    package_version(&asset.Version).is_some()
        && asset.PackageId == APP_ID
        && asset.FileName == format!("{APP_ID}-{}-{kind}.nupkg", asset.Version)
        && (1..=MAX_PACKAGE).contains(&asset.Size)
        && asset.SHA256.len() == 64
        && asset.SHA256.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Only used to route the signature request and deduplicate verification failures. This version
/// alone is not trusted and never authorizes a package download or an apply.
pub(super) fn advertised_version(manifest: &str) -> Option<Version> {
    let feed: VelopackAssetFeed = serde_json::from_str(manifest).ok()?;
    feed.Assets
        .iter()
        .filter(|a| a.Type == "Full")
        .filter_map(|asset| package_version(&asset.Version))
        .max()
}

fn signed_bytes(tag: &str, manifest: &str) -> Vec<u8> {
    format!("{DOMAIN}{tag}\n{manifest}").into_bytes()
}

fn keys() -> impl Iterator<Item = VerifyingKey> {
    KEYS.split_whitespace()
        .filter_map(|hex| VerifyingKey::from_bytes(&hex::decode(hex).ok()?.try_into().ok()?).ok())
}

impl Receipt {
    pub fn verify(&self) -> Result<Release, Invalid> {
        self.verify_with(keys())
    }

    pub(super) fn verify_with(
        &self,
        keys: impl IntoIterator<Item = VerifyingKey>,
    ) -> Result<Release, Invalid> {
        if self.manifest.len() as u64 > MAX_FEED || self.proof.len() as u64 > MAX_PROOF {
            return Err(Invalid);
        }
        let proof: Proof = serde_json::from_str(&self.proof).map_err(|_| Invalid)?;
        let version = proof
            .tag
            .strip_prefix('v')
            .and_then(package_version)
            .ok_or(Invalid)?;
        let sig = hex::decode(&proof.signature).map_err(|_| Invalid)?;
        let sig = Signature::from_slice(&sig).map_err(|_| Invalid)?;
        let bytes = signed_bytes(&proof.tag, &self.manifest);
        if !keys
            .into_iter()
            .any(|key| key.verify_strict(&bytes, &sig).is_ok())
        {
            return Err(Invalid);
        }
        let feed: VelopackAssetFeed = serde_json::from_str(&self.manifest).map_err(|_| Invalid)?;
        let (mut full, mut previous, mut delta) = (None, None, None);
        for asset in feed.Assets {
            if !valid_asset(&asset) {
                return Err(Invalid);
            }
            let asset_version = package_version(&asset.Version).ok_or(Invalid)?;
            match asset.Type.as_str() {
                "Full" if asset_version == version && full.is_none() => full = Some(asset),
                "Full" if asset_version < version && previous.is_none() => previous = Some(asset),
                "Delta" if asset_version == version && delta.is_none() => delta = Some(asset),
                _ => return Err(Invalid),
            }
        }
        if previous.is_some() != delta.is_some() {
            return Err(Invalid);
        }
        Ok(Release {
            version,
            full: full.ok_or(Invalid)?,
        })
    }
}

impl Release {
    pub fn newer_than(&self, installed: &Version) -> bool {
        // A valid signature does not authorize rollback or reinstall of an older/equal version.
        self.version > *installed
    }
}

pub(super) fn matches_bytes(asset: &VelopackAsset, size: u64, digest: &[u8]) -> bool {
    let mut expected = [0; 32];
    size == asset.Size
        && hex::decode_to_slice(&asset.SHA256, &mut expected).is_ok()
        && digest == expected
}

pub(super) fn matches_file(asset: &VelopackAsset, path: &Path) -> io::Result<bool> {
    let file = File::open(path)?;
    if file.metadata()?.len() != asset.Size {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let read = io::copy(&mut file.take(asset.Size + 1), &mut hash)?;
    Ok(matches_bytes(asset, read, &hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn test_manifest() -> Value {
        let entry = |version: &str, kind: &str| {
            json!({
                "PackageId": APP_ID, "Version": version, "Type": kind,
                "FileName": format!("{APP_ID}-{version}-{}.nupkg", kind.to_ascii_lowercase()),
                "SHA256": hex::encode(Sha256::digest(b"sample")), "Size": 6,
            })
        };
        json!({"Assets": [entry("0.1.0-build.9", "Full"), entry("0.1.0-build.10", "Full"), entry("0.1.0-build.10", "Delta")]})
    }

    fn signed(key: &SigningKey, tag: &str, manifest: &Value) -> Receipt {
        let manifest = manifest.to_string();
        let signature = key.sign(&signed_bytes(tag, &manifest));
        Receipt {
            manifest,
            proof: json!({"tag": tag, "signature": hex::encode(signature.to_bytes())}).to_string(),
        }
    }

    #[test]
    fn trusted_signature_binds_full_delta_version_and_tag() {
        let signing = key(1);
        let tag = "v0.1.0-build.10";
        let receipt = signed(&signing, tag, &test_manifest());
        let release = receipt
            .verify_with([key(2).verifying_key(), signing.verifying_key()])
            .expect("one trusted key");
        assert_eq!(release.version.to_string(), "0.1.0-build.10");
        assert_eq!(release.full.Type, "Full");
        assert!(receipt.verify_with([key(2).verifying_key()]).is_err());
        assert!(
            signed(&signing, "v0.1.0-build.11", &test_manifest())
                .verify_with([signing.verifying_key()])
                .is_err()
        );

        for (index, field, value) in [
            (0, "Size", json!(7)),
            (1, "Version", json!("0.1.0-build.11")),
            (2, "SHA256", json!("f".repeat(64))),
        ] {
            let mut tampered = test_manifest();
            tampered["Assets"][index][field] = value;
            let mut receipt = signed(&signing, tag, &test_manifest());
            receipt.manifest = tampered.to_string();
            assert!(receipt.verify_with([signing.verifying_key()]).is_err());
        }
        let mut no_proof = signed(&signing, tag, &test_manifest());
        no_proof.proof.clear();
        assert!(no_proof.verify_with([signing.verifying_key()]).is_err());
    }

    #[test]
    fn package_hash_size_and_downgrade_are_checked() {
        let signing = key(3);
        let release = signed(&signing, "v0.1.0-build.10", &test_manifest())
            .verify_with([signing.verifying_key()])
            .expect("trusted signature");
        for installed in ["0.1.0-build.10", "0.1.0-build.11", "0.2.0-build.9"] {
            assert!(!release.newer_than(&Version::parse(installed).expect("valid version")));
        }
        assert!(release.newer_than(&Version::parse("0.1.0-build.9").expect("valid version")));
        let file = tempfile::NamedTempFile::new().expect("temp file");
        for (bytes, expected) in [
            (b"sample".as_slice(), true),
            (b"Sample", false),
            (b"sample!", false),
        ] {
            std::fs::write(file.path(), bytes).expect("write fixture");
            assert_eq!(
                matches_file(&release.full, file.path()).expect("hash fixture"),
                expected
            );
        }
    }

    #[test]
    fn compiled_update_keys_are_valid() {
        let keys: Vec<_> = keys().collect();
        assert_eq!(keys.len(), KEYS.split_whitespace().count());
        assert!(!keys.is_empty() && keys.iter().all(|key| !key.is_weak()));
    }
}
