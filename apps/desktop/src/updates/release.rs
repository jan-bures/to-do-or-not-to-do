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
const DOMAIN: &str = "todo-desktop-update-v2\n";
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
    setup_sha256: String,
    setup_size: u64,
    signature: String,
}

#[derive(Debug)]
pub(super) struct Release {
    pub version: Version,
    pub full: VelopackAsset,
    pub rejection: Rejection,
}

/// Created only after successful signature verification. Canonical signed bytes and the decoded
/// signature keep formatting changes to the unsigned JSON envelope from evading a rejection.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct Rejection {
    pub version: Version,
    pub fingerprint: [u8; 32],
}

pub(super) fn read_bounded(source: impl Read, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    source.take(limit + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn package_version(raw: &str) -> Option<(Version, u32)> {
    let version = Version::parse(raw).ok()?;
    let height = version
        .pre
        .as_str()
        .strip_prefix("build.")?
        .parse::<u32>()
        .ok()?;
    (version.build.is_empty() && height > 0).then_some((version, height))
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

fn signed_bytes(proof: &Proof, manifest: &str) -> Vec<u8> {
    format!(
        "{DOMAIN}{}\n{}\n{}\n{manifest}",
        proof.tag, proof.setup_sha256, proof.setup_size
    )
    .into_bytes()
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
        let (version, _) = proof
            .tag
            .strip_prefix('v')
            .and_then(package_version)
            .ok_or(Invalid)?;
        if proof.setup_size == 0
            || proof.setup_sha256.len() != 64
            || !proof
                .setup_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Invalid);
        }
        let sig = hex::decode(&proof.signature).map_err(|_| Invalid)?;
        let sig = Signature::from_slice(&sig).map_err(|_| Invalid)?;
        let bytes = signed_bytes(&proof, &self.manifest);
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
            let (asset_version, _) = package_version(&asset.Version).ok_or(Invalid)?;
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
        let mut fingerprint = Sha256::new();
        fingerprint.update(bytes);
        fingerprint.update(sig.to_bytes());
        let rejection = Rejection {
            version: version.clone(),
            fingerprint: fingerprint.finalize().into(),
        };
        Ok(Release {
            version,
            full: full.ok_or(Invalid)?,
            rejection,
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
pub(super) fn test_receipt(
    key: &ed25519_dalek::SigningKey,
    tag: &str,
    manifest: String,
) -> Receipt {
    use ed25519_dalek::Signer;
    use serde_json::json;
    let proof = Proof {
        tag: tag.into(),
        setup_sha256: "1".repeat(64),
        setup_size: 100,
        signature: String::new(),
    };
    let signature = key.sign(&signed_bytes(&proof, &manifest));
    Receipt { manifest, proof: json!({"tag":tag,"setup_sha256":proof.setup_sha256,"setup_size":proof.setup_size,"signature":hex::encode(signature.to_bytes())}).to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
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
        test_receipt(key, tag, manifest.to_string())
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
        let reformatted = Receipt {
            manifest: receipt.manifest.clone(),
            proof: serde_json::to_string_pretty(
                &serde_json::from_str::<Value>(&receipt.proof).expect("valid proof"),
            )
            .expect("serializable proof"),
        };
        assert_eq!(
            reformatted
                .verify_with([signing.verifying_key()])
                .expect("proof formatting is not signed")
                .rejection,
            release.rejection
        );
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
        for field in ["setup_sha256", "setup_size"] {
            let mut receipt = signed(&signing, tag, &test_manifest());
            let mut proof: Value = serde_json::from_str(&receipt.proof).expect("proof object");
            proof[field] = if field == "setup_size" {
                json!(1)
            } else {
                json!("0".repeat(64))
            };
            receipt.proof = proof.to_string();
            assert!(receipt.verify_with([signing.verifying_key()]).is_err());
        }
        let mut no_proof = signed(&signing, tag, &test_manifest());
        no_proof.proof.clear();
        assert!(no_proof.verify_with([signing.verifying_key()]).is_err());
    }

    #[test]
    fn python_signer_fixture_verifies_and_binds_manifest_setup_and_tag() {
        const MANIFEST: &str =
            include_str!("../../tests/fixtures/update-signing/releases.win.json");
        const PROOF: &str =
            include_str!("../../tests/fixtures/update-signing/desktop-update.sig.json");
        const SETUP: &[u8] = include_bytes!("../../tests/fixtures/update-signing/Setup.exe");
        const PUBLIC_KEY: &str = include_str!("../../tests/fixtures/update-signing/public-key.txt");
        let mut pin = [0; 32];
        hex::decode_to_slice(PUBLIC_KEY.trim(), &mut pin).expect("fixture key hex");
        let key = VerifyingKey::from_bytes(&pin).expect("fixture key");
        assert!(
            keys().all(|configured| configured != key),
            "the test key must never be trusted in production"
        );
        let verify = |manifest: &str, proof: &str| {
            Receipt {
                manifest: manifest.into(),
                proof: proof.into(),
            }
            .verify_with([key])
        };
        let release = verify(MANIFEST, PROOF).expect("proof produced by the Python signer");
        assert_eq!(release.version.to_string(), "0.1.0-build.9");
        let mut proof: Value = serde_json::from_str(PROOF).expect("proof object");
        assert_eq!(
            proof["setup_sha256"],
            json!(hex::encode(Sha256::digest(SETUP)))
        );
        assert_eq!(proof["setup_size"], json!(SETUP.len()));

        for index in 0..MANIFEST.len() {
            let mut changed = MANIFEST.as_bytes().to_vec();
            changed[index] ^= 1;
            assert!(
                verify(std::str::from_utf8(&changed).expect("ASCII fixture"), PROOF).is_err(),
                "manifest byte {index}"
            );
        }
        for index in 0..SETUP.len() {
            let mut changed = SETUP.to_vec();
            changed[index] ^= 1;
            // Rust receives the Setup identity in the proof, not the installer. Substituting
            // the digest of different Setup bytes must invalidate the signature.
            proof["setup_sha256"] = json!(hex::encode(Sha256::digest(changed)));
            assert!(
                verify(MANIFEST, &proof.to_string()).is_err(),
                "Setup byte {index}"
            );
        }
        let mut changed: Proof = serde_json::from_str(PROOF).expect("proof object");
        changed.tag = "v0.1.0-build.8".into();
        let signature =
            Signature::from_slice(&hex::decode(&changed.signature).expect("signature hex"))
                .expect("signature");
        // Check the signature independently of the semantic tag/package version comparison.
        assert!(
            key.verify_strict(&signed_bytes(&changed, MANIFEST), &signature)
                .is_err()
        );
        assert!(verify(MANIFEST, &PROOF.replace("v0.1.0-build.9", &changed.tag)).is_err());
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
