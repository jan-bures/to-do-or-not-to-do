//! Owner bearer tokens: the configuration stores only SHA-256 hashes, compared in constant time.

use super::{AuthError, Authenticator, Principal};
use crate::config::{OwnerToken, decode_hash};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use todo_core::DeviceId;

/// Longest bearer token accepted; anything longer is rejected before hashing.
const MAX_TOKEN_LEN: usize = 512;

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
}

/// Hex SHA-256 of a token, as stored in the config.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn bearer(headers: &HeaderMap) -> Result<&str, AuthError> {
    let value = headers.get(AUTHORIZATION).ok_or(AuthError::Missing)?;
    let value = value.to_str().map_err(|_| AuthError::Invalid)?;
    let (scheme, token) = value.split_once(' ').ok_or(AuthError::Invalid)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(AuthError::Invalid);
    }
    let token = token.trim();
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return Err(AuthError::Invalid);
    }
    Ok(token)
}

impl Authenticator for OwnerTokens {
    fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthError> {
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
            .map(|device| Principal::Owner {
                device: device.clone(),
            })
            .ok_or(AuthError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

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
        let p = tokens().authenticate(&headers("Bearer s3cret")).unwrap();
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
