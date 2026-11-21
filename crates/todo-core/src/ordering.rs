//! Fractional-index position keys.
//!
//! A [`PosKey`] is a string over the base-62 alphabet `0-9A-Za-z` (ASCII order equals digit order) read as
//! a fraction `0.d1d2d3...`. Keys compare lexicographically, never end in `0`, and there is always a key
//! strictly between any two distinct keys. Reordering an item therefore rewrites only that item's key.
//!
//! Appending and prepending increment the first digit instead of bisecting, so a long run of appends
//! grows a key by one character per ~30 items rather than one per ~5. Inserting repeatedly at the same
//! spot still grows keys, which is inherent to fractional indexing; rebalancing is not implemented.
//!
//! Keys are at most [`MAX_KEY_LEN`] characters. Validation (and therefore deserialization) rejects longer
//! keys, and the generators fail with [`OrderError::TooLong`] instead of producing one. All generators
//! are iterative, so adversarial keys cannot exhaust the stack.

use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: u32 = 62;

/// Longest accepted position key, in characters.
pub const MAX_KEY_LEN: usize = 128;

fn val(b: u8) -> u32 {
    DIGITS.iter().position(|&d| d == b).map_or(0, |p| p as u32)
}

fn digit(v: u32) -> u8 {
    DIGITS[(v as usize).min(61)]
}

/// Errors from key generation or validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrderError {
    /// The string is empty, has a character outside `0-9A-Za-z`, or ends in `0`.
    #[error("invalid position key `{0}`")]
    InvalidKey(String),
    /// `between(a, b)` was called with `a >= b`.
    #[error("lower bound `{lower}` is not before upper bound `{upper}`")]
    BadBounds {
        /// The lower bound.
        lower: String,
        /// The upper bound.
        upper: String,
    },
    /// The key (given or generated) is longer than [`MAX_KEY_LEN`] characters.
    #[error("position key is {len} characters long; at most {MAX_KEY_LEN} are allowed")]
    TooLong {
        /// Length of the offending key.
        len: usize,
    },
}

/// A fractional-index position key. Construct with [`PosKey::new`] or the generators.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PosKey(String);

impl<'de> Deserialize<'de> for PosKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for PosKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A key strictly between `a` and `b` (`b = None` means 1.0). Requires `a < b`.
fn midpoint(mut a: &[u8], mut b: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        if let Some(hi) = b {
            // Copy the common prefix (`a` is padded with zeros).
            let n = hi
                .iter()
                .zip(a.iter().copied().chain(std::iter::repeat(b'0')))
                .take_while(|(x, y)| *x == y)
                .count();
            if n > 0 {
                out.extend_from_slice(&hi[..n]);
                a = a.get(n..).unwrap_or(&[]);
                b = Some(&hi[n..]);
                continue;
            }
        }
        let da = a.first().map_or(0, |&x| val(x));
        let db = b.and_then(|b| b.first()).map_or(BASE, |&x| val(x));
        if db.saturating_sub(da) > 1 {
            out.push(digit((da + db) / 2));
            return out;
        }
        if let Some(hi) = b
            && hi.len() > 1
        {
            out.push(hi[0]);
            return out;
        }
        out.push(digit(da));
        a = a.get(1..).unwrap_or(&[]);
        b = None;
    }
}

fn after_bytes(a: &[u8]) -> Vec<u8> {
    // Skip the leading run of `z`, then bump the first digit that can be bumped.
    let n = a.iter().take_while(|&&b| b == b'z').count();
    let mut out = a[..n].to_vec();
    match a.get(n) {
        Some(&f) => out.push(digit(val(f) + 1)),
        None => out.push(digit(BASE / 2)),
    }
    out
}

fn before_bytes(b: &[u8]) -> Vec<u8> {
    // Skip the leading run of `0` (a valid key never consists of zeros only).
    let n = b.iter().take_while(|&&x| x == b'0').count();
    let mut out = b[..n].to_vec();
    let rest = &b[n..];
    match rest.first().map(|&x| val(x)) {
        Some(d) if d >= 2 => out.push(digit(d - 1)),
        Some(_) => out.extend(midpoint(&[], Some(rest))),
        None => out.push(digit(BASE / 2)),
    }
    out
}

fn check_len(len: usize) -> Result<(), OrderError> {
    if len > MAX_KEY_LEN {
        Err(OrderError::TooLong { len })
    } else {
        Ok(())
    }
}

impl PosKey {
    /// Validates a key string.
    pub fn new(s: impl Into<String>) -> Result<Self, OrderError> {
        let s = s.into();
        check_len(s.len())?;
        let ok = !s.is_empty() && s.bytes().all(|b| DIGITS.contains(&b)) && !s.ends_with('0');
        if ok {
            Ok(Self(s))
        } else {
            Err(OrderError::InvalidKey(s))
        }
    }

    fn from_bytes(v: Vec<u8>) -> Result<Self, OrderError> {
        check_len(v.len())?;
        Ok(Self(String::from_utf8(v).unwrap_or_default()))
    }

    /// The key as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A key for the first item of an empty sequence.
    #[must_use]
    pub fn first() -> Self {
        Self(char::from(digit(BASE / 2)).to_string())
    }

    /// A key strictly after `self`. Fails only if the result would exceed [`MAX_KEY_LEN`].
    pub fn after(&self) -> Result<Self, OrderError> {
        Self::from_bytes(after_bytes(self.0.as_bytes()))
    }

    /// A key strictly before `self`. Fails only if the result would exceed [`MAX_KEY_LEN`].
    pub fn before(&self) -> Result<Self, OrderError> {
        Self::from_bytes(before_bytes(self.0.as_bytes()))
    }

    /// A key strictly between `lower` and `upper`. Either bound may be open (`None`).
    ///
    /// Fails if both bounds are present and `lower >= upper`, or if the result would exceed
    /// [`MAX_KEY_LEN`].
    pub fn between(lower: Option<&PosKey>, upper: Option<&PosKey>) -> Result<Self, OrderError> {
        match (lower, upper) {
            (None, None) => Ok(Self::first()),
            (Some(a), None) => a.after(),
            (None, Some(b)) => b.before(),
            (Some(a), Some(b)) => {
                if a >= b {
                    return Err(OrderError::BadBounds {
                        lower: a.0.clone(),
                        upper: b.0.clone(),
                    });
                }
                Self::from_bytes(midpoint(a.0.as_bytes(), Some(b.0.as_bytes())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> PosKey {
        PosKey::new(s).unwrap()
    }

    #[test]
    fn validation() {
        assert!(PosKey::new("").is_err());
        assert!(PosKey::new("a0").is_err());
        assert!(PosKey::new("a-b").is_err());
        assert!(PosKey::new("a1").is_ok());
    }

    #[test]
    fn between_basic() {
        let m = PosKey::between(Some(&k("a")), Some(&k("b"))).unwrap();
        assert!(k("a") < m && m < k("b"));
        let m = PosKey::between(Some(&k("a")), Some(&k("a1"))).unwrap();
        assert!(k("a") < m && m < k("a1"));
        let m = PosKey::between(Some(&k("1")), Some(&k("2"))).unwrap();
        assert!(k("1") < m && m < k("2"));
        assert!(PosKey::between(Some(&k("b")), Some(&k("a"))).is_err());
        assert!(PosKey::between(Some(&k("a")), Some(&k("a"))).is_err());
    }

    #[test]
    fn before_and_after() {
        for s in ["1", "0V", "z", "zz", "V", "a1", "01"] {
            let key = k(s);
            assert!(key.after().unwrap() > key, "after {s}");
            assert!(key.before().unwrap() < key, "before {s}");
        }
    }

    #[test]
    fn appends_stay_short() {
        let mut key = PosKey::first();
        for _ in 0..1000 {
            let next = key.after().unwrap();
            assert!(next > key);
            key = next;
        }
        assert!(key.as_str().len() <= 40, "len {}", key.as_str().len());
    }

    #[test]
    fn prepends_stay_short() {
        let mut key = PosKey::first();
        for _ in 0..1000 {
            let next = key.before().unwrap();
            assert!(next < key);
            key = next;
        }
        assert!(key.as_str().len() <= 70, "len {}", key.as_str().len());
    }

    #[test]
    fn adversarial_keys_are_bounded_and_do_not_recurse() {
        // Far too long: rejected by validation and by deserialization, without walking the string.
        let huge = "z".repeat(100_000);
        assert_eq!(
            PosKey::new(huge.clone()),
            Err(OrderError::TooLong { len: 100_000 })
        );
        let json = format!("\"{huge}\"");
        assert!(serde_json::from_str::<PosKey>(&json).is_err());
        // The longest valid keys: the generators fail with a typed error instead of overflowing.
        let max_z = k(&"z".repeat(MAX_KEY_LEN));
        assert!(matches!(max_z.after(), Err(OrderError::TooLong { .. })));
        let zeros = k(&format!("{}1", "0".repeat(MAX_KEY_LEN - 1)));
        assert!(matches!(zeros.before(), Err(OrderError::TooLong { .. })));
        // Long runs that used to recurse once per character.
        let run = k(&format!("y{}", "z".repeat(MAX_KEY_LEN - 2)));
        let mid = PosKey::between(Some(&run), Some(&k("z"))).unwrap();
        assert!(run < mid && mid < k("z"));
        let low = k(&format!("{}2", "0".repeat(MAX_KEY_LEN - 2)));
        let b = low.before().unwrap();
        assert!(b < low && b.as_str().len() <= MAX_KEY_LEN);
        let a = k(&"z".repeat(MAX_KEY_LEN - 1)).after().unwrap();
        assert_eq!(a.as_str().len(), MAX_KEY_LEN);
    }

    #[test]
    fn serde_validates() {
        assert!(serde_json::from_str::<PosKey>("\"a0\"").is_err());
        assert_eq!(serde_json::from_str::<PosKey>("\"a1\"").unwrap(), k("a1"));
    }
}
