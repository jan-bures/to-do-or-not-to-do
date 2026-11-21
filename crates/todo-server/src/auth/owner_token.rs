//! Static owner bearer tokens from the config: a development and test fallback, accepted only with
//! `auth.allow_config_owner_tokens`. The configuration stores only SHA-256 hashes, compared in
//! constant time.

use super::{AuthError, Authenticated, Authenticator, bearer};
use crate::config::{OwnerToken, decode_hash};
use axum::http::HeaderMap;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use todo_core::DeviceId;

/// Authenticates `Authorization: Bearer <token>` against configured owner token hashes.
#[derive(Clone)]
pub struct OwnerTokens {
    entries: Vec<([u8; 32], DeviceId)>,
}

impl std::fmt::Debug for OwnerTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerTokens")
            .field("count", &self.entries.len())
            .finish()
    }
}

impl OwnerTokens {
    /// Builds the authenticator. Entries with an invalid hash are skipped (config validation rejects
    /// them earlier).
    #[must_use]
    pub fn new(tokens: &[OwnerToken]) -> Self {
        Self {
            entries: tokens
                .iter()
                .filter_map(|t| decode_hash(&t.sha256).map(|h| (h, t.device.clone())))
                .collect(),
        }
    }

    /// Number of configured tokens.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if no token is configured (every request is then rejected).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// True if a token is configured for `device`.
    #[must_use]
    pub fn has_device(&self, device: &DeviceId) -> bool {
        self.entries.iter().any(|(_, d)| d == device)
    }
}

/// Hex SHA-256 of a token, as stored in the config.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

impl Authenticator for OwnerTokens {
    fn authenticate(&self, headers: &HeaderMap) -> Result<Authenticated, AuthError> {
        let token = bearer(headers)?;
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        // Compare against every entry without stopping early.
        let mut found: Option<&DeviceId> = None;
        for (hash, device) in &self.entries {
            let eq: bool = hash.ct_eq(&digest).into();
            if eq && found.is_none() {
                found = Some(device);
            }
        }
        found
            .map(|device| Authenticated::owner(device.clone()))
            .ok_or(AuthError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{MAX_TOKEN_LEN, Principal};
    use super::*;
    use axum::http::HeaderValue;
    use axum::http::header::AUTHORIZATION;

    fn tokens() -> OwnerTokens {
        OwnerTokens::new(&[OwnerToken {
            device: DeviceId::new("desk").unwrap(),
            sha256: hash_token("s3cret"),
        }])
    }

    fn headers(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn accepts_the_configured_token() {
        let p = tokens()
            .authenticate(&headers("Bearer s3cret"))
            .unwrap()
            .principal;
        assert_eq!(
            p,
            Principal::Owner {
                device: DeviceId::new("desk").unwrap()
            }
        );
        assert!(tokens().authenticate(&headers("bearer s3cret")).is_ok());
    }

    #[test]
    fn rejects_everything_else() {
        let t = tokens();
        assert!(matches!(
            t.authenticate(&HeaderMap::new()),
            Err(AuthError::Missing)
        ));
        for bad in [
            "Bearer wrong",
            "Basic s3cret",
            "Bearer",
            "s3cret",
            "Bearer  ",
        ] {
            assert!(t.authenticate(&headers(bad)).is_err(), "{bad}");
        }
        let long = format!("Bearer {}", "a".repeat(MAX_TOKEN_LEN + 1));
        assert!(t.authenticate(&headers(&long)).is_err());
    }
}
