//! Content fingerprints: SHA-256 over a canonical serialization.
//!
//! The journal stores a fingerprint of every applied event so that [`undo`](crate::undo::undo) can check
//! that the event it is asked to invert is exactly the one that was applied. The serialization is the
//! compact JSON form of the value; every type hashed here has a deterministic serialization (fields in
//! declaration order, ordered maps and sets, no floats). Hashing uses the pure-Rust `sha2` crate, which
//! builds for every target the shared crates support.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io;

/// SHA-256 of `value`'s canonical serialization.
#[must_use]
pub fn fingerprint<T: Serialize + ?Sized>(value: &T) -> [u8; 32] {
    let mut w = HashWriter(Sha256::new());
    // Writing into the hasher cannot fail, and none of the hashed types fail to serialize.
    if serde_json::to_writer(&mut w, value).is_err() {
        w.0.update(b"\0serialization failed");
    }
    w.0.finalize().into()
}

/// Adapts a hasher to [`io::Write`] so serialization streams straight into it.
struct HashWriter(Sha256);

impl io::Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: [u8; 32]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn fingerprint_is_sha256_of_compact_json() {
        // JSON of "abc" is "\"abc\"" (5 bytes).
        assert_eq!(
            hex(fingerprint("abc")),
            hex(Sha256::digest(b"\"abc\"").into())
        );
    }

    #[test]
    fn fingerprint_distinguishes_values() {
        assert_eq!(fingerprint(&("a", 1)), fingerprint(&("a", 1)));
        assert_ne!(fingerprint(&("a", 1)), fingerprint(&("a", 2)));
    }
}
