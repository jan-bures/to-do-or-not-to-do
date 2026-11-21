//! Random secrets and their hashes.
//!
//! Every secret the server hands out (device and agent tokens, session cookies, pairing poll tokens,
//! share links, the setup code) comes from the OS random number generator and is stored only as its
//! SHA-256 hash. Lookups hash the presented value and look the hash up, so an unknown secret costs the
//! same as a known one and nothing is compared byte by byte against stored secrets.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Prefix of device session tokens.
pub const DEVICE_TOKEN_PREFIX: &str = "tdd_";
/// Prefix of agent tokens.
pub const AGENT_TOKEN_PREFIX: &str = "tda_";
/// Letters of pairing user codes and setup codes: no `0`/`O`, `1`/`I`/`L`, so they read back
/// unambiguously.
pub const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// The OS random number generator failed (should never happen on a supported platform).
#[derive(Debug, thiserror::Error)]
#[error("the system random number generator failed")]
pub struct RandomError;

/// `N` random bytes.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], RandomError> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|_| RandomError)?;
    Ok(buf)
}

/// A random base64url string of `N` bytes of entropy.
pub fn random_token<const N: usize>() -> Result<String, RandomError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<N>()?))
}

/// A random code of `len` letters from [`CODE_ALPHABET`] (rejection sampling, so unbiased).
pub fn random_code(len: usize) -> Result<String, RandomError> {
    let n = CODE_ALPHABET.len();
    // The largest multiple of `n` that fits in a byte; bytes at or above it are rejected.
    let limit = 256 - (256 % n);
    let mut out = String::with_capacity(len);
    while out.len() < len {
        for b in random_bytes::<32>()? {
            if usize::from(b) < limit && out.len() < len {
                out.push(char::from(CODE_ALPHABET[usize::from(b) % n]));
            }
        }
    }
    Ok(out)
}

/// Normalises a typed code: upper case, without spaces and dashes.
#[must_use]
pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// SHA-256 of a secret.
#[must_use]
pub fn digest(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

/// Hex SHA-256 of a secret, as stored.
#[must_use]
pub fn digest_hex(secret: &str) -> String {
    hex::encode(digest(secret))
}

/// Constant-time equality of two byte strings.
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_random_and_distinct() {
        let a = random_token::<32>().unwrap();
        let b = random_token::<32>().unwrap();
        assert_eq!(a.len(), 43);
        assert_ne!(a, b);
        assert_eq!(random_token::<16>().unwrap().len(), 22);
    }

    #[test]
    fn codes_use_the_unambiguous_alphabet() {
        let c = random_code(500).unwrap();
        assert_eq!(c.len(), 500);
        assert!(c.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        for bad in ['0', 'O', '1', 'I', 'L'] {
            assert!(!c.contains(bad));
        }
        assert_eq!(normalize_code(" abcd-efgh "), "ABCDEFGH");
    }

    #[test]
    fn digests_compare_in_constant_time() {
        assert!(ct_eq(&digest("x"), &digest("x")));
        assert!(!ct_eq(&digest("x"), &digest("y")));
        assert!(!ct_eq(b"ab", b"abc"));
    }
}
