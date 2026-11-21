//! Attachment encryption: the server's master key ring and the per-object STREAM construction.
//!
//! # Construction
//!
//! Every object is sealed before it leaves the server, so the bucket (and anyone holding its credential)
//! only ever sees ciphertext.
//!
//! - **Per-object key.** A fresh random 32-byte salt per object; the object key is
//!   `HKDF-SHA256(ikm = master key, salt, info = "todo-server attachment v1")`. Each object therefore has
//!   its own XChaCha20-Poly1305 key, and the master key itself never encrypts anything.
//! - **Chunks.** The plaintext is cut into [`CHUNK`]-byte chunks, each sealed with the STREAM construction
//!   (Hoang, Reyhanitabar, Rogaway, Vizár; big-endian 32-bit counter, `aead::stream::StreamBE32`) over
//!   XChaCha20-Poly1305 with a random 19-byte nonce prefix. The last chunk carries STREAM's "last" flag,
//!   so reordering, dropping, duplicating or truncating chunks fails authentication.
//! - **Associated data.** Every chunk authenticates the header, the bucket object key and the attachment
//!   ID. An object copied or renamed to another key, or served for another attachment, fails to decrypt:
//!   swapping objects is detected.
//!
//! The object layout is:
//!
//! ```text
//! magic "TODOATT1" (8) | key id (8) | salt (32) | nonce prefix (19) | chunk 1 | ... | last chunk
//! ```
//!
//! where each chunk is its plaintext plus a 16-byte tag (an empty file is one empty last chunk).
//!
//! # Master key
//!
//! `attachments.key` in the data directory, created owner-only with `todo-fs` on first use. One hex key
//! per line: the first line is the current key (new objects), later lines are older keys kept only to
//! read objects sealed with them. Each object names its key by an 8-byte ID (a hash of the key), so the
//! right one is picked without trial decryption. The key is never logged; its `Debug` is redacted.
//!
//! **Rotation:** stop the server, add a new key as the *first* line (keep the old lines), start the server.
//! New uploads use the new key; old objects stay readable. Re-encrypting old objects is not implemented;
//! drop an old line only once no object needs it (they age out when their attachments are deleted).

use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::{KeyInit, Payload};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Plaintext bytes per chunk.
pub const CHUNK: usize = 64 * 1024;
/// Poly1305 tag size.
pub const TAG: usize = 16;
/// Format marker and version.
pub const MAGIC: &[u8; 8] = b"TODOATT1";
const KEY_ID: usize = 8;
const SALT: usize = 32;
/// XChaCha20's 24-byte nonce minus STREAM's 5 bytes of counter and flag.
const NONCE_PREFIX: usize = 19;
/// Length of the object header.
pub const HEADER: usize = MAGIC.len() + KEY_ID + SALT + NONCE_PREFIX;
const INFO: &[u8] = b"todo-server attachment v1";
/// File name of the key ring in the data directory.
pub const KEY_FILE: &str = "attachments.key";

/// Why sealing or opening failed.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// The key file could not be read or created.
    #[error("attachment key file {path}: {source}")]
    KeyFile {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// The key file holds something other than hex keys.
    #[error("attachment key file {path}: line {line} is not a 64-digit hex key")]
    BadKey {
        /// The file.
        path: PathBuf,
        /// 1-based line number (the line itself is never echoed).
        line: usize,
    },
    /// No randomness.
    #[error("the system random number generator failed")]
    Random,
    /// The object is not one of ours, was sealed with a key not in the ring, or failed authentication
    /// (tampered, truncated, or swapped with another object).
    #[error("attachment object failed authentication: {0}")]
    Corrupt(&'static str),
    /// Reading or writing the object failed.
    #[error("attachment object I/O: {0}")]
    Io(#[from] io::Error),
}

#[derive(Clone)]
struct MasterKey {
    id: [u8; KEY_ID],
    key: [u8; 32],
}

impl MasterKey {
    fn new(key: [u8; 32]) -> Self {
        let digest = Sha256::new()
            .chain_update(b"todo-server attachment key id\0")
            .chain_update(key)
            .finalize();
        let mut id = [0u8; KEY_ID];
        id.copy_from_slice(&digest[..KEY_ID]);
        Self { id, key }
    }

    fn object_cipher(&self, salt: &[u8]) -> Result<XChaCha20Poly1305, CryptoError> {
        let hk = Hkdf::<Sha256>::new(Some(salt), &self.key);
        let mut okm = [0u8; 32];
        hk.expand(INFO, &mut okm)
            .map_err(|_| CryptoError::Corrupt("key derivation"))?;
        let cipher = XChaCha20Poly1305::new((&okm).into());
        okm.fill(0);
        Ok(cipher)
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.key.fill(0);
    }
}

/// The master keys: the current one first, then older ones (read-only).
#[derive(Clone)]
pub struct KeyRing {
    keys: Vec<MasterKey>,
}

impl std::fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyRing")
            .field("keys", &self.keys.len())
            .finish_non_exhaustive()
    }
}

fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|_| CryptoError::Random)?;
    Ok(b)
}

impl KeyRing {
    /// A ring with one key (tests).
    #[must_use]
    pub fn from_key(key: [u8; 32]) -> Self {
        Self {
            keys: vec![MasterKey::new(key)],
        }
    }

    /// A ring with a fresh random key (tests).
    pub fn generate() -> Result<Self, CryptoError> {
        Ok(Self::from_key(random()?))
    }

    /// Adds an older, read-only key at the end (tests).
    #[must_use]
    pub fn with_old_key(mut self, key: [u8; 32]) -> Self {
        self.keys.push(MasterKey::new(key));
        self
    }

    /// Loads `dir/attachments.key`, creating it owner-only with a fresh key if it does not exist.
    /// An existing file must be owner-only (checked with `todo-fs`).
    pub fn load_or_create(dir: &Path) -> Result<Self, CryptoError> {
        let path = dir.join(KEY_FILE);
        let err = |source| CryptoError::KeyFile {
            path: path.clone(),
            source,
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                todo_fs::verify_private(&path, todo_fs::Private::AnyFile).map_err(err)?;
                Self::parse(&text, &path)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let key: [u8; 32] = random()?;
                let tmp = dir.join(format!("{KEY_FILE}.new"));
                let _ = std::fs::remove_file(&tmp);
                let mut f = todo_fs::create_private_file(&tmp).map_err(err)?;
                let mut line = hex::encode(key);
                line.push('\n');
                f.write_all(line.as_bytes()).map_err(err)?;
                f.sync_all().map_err(err)?;
                drop(f);
                // Never replace a key that appeared meanwhile: that would orphan every object sealed with it.
                if path.exists() {
                    let _ = std::fs::remove_file(&tmp);
                    let text = std::fs::read_to_string(&path).map_err(err)?;
                    return Self::parse(&text, &path);
                }
                todo_fs::replace_durably(&tmp, &path).map_err(err)?;
                tracing::info!("created a new attachment encryption key; back it up");
                Ok(Self::from_key(key))
            }
            Err(e) => Err(err(e)),
        }
    }

    fn parse(text: &str, path: &Path) -> Result<Self, CryptoError> {
        let mut keys = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let bytes: [u8; 32] = hex::decode(line)
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| CryptoError::BadKey {
                    path: path.to_owned(),
                    line: n + 1,
                })?;
            keys.push(MasterKey::new(bytes));
        }
        if keys.is_empty() {
            return Err(CryptoError::BadKey {
                path: path.to_owned(),
                line: 1,
            });
        }
        Ok(Self { keys })
    }

    /// A UUID (version 8) derived from the current key and `context`: the same for the same key and
    /// context, and unpredictable without the key. For server-made IDs that must not be guessable.
    pub fn derive_uuid(&self, context: &str) -> Result<uuid::Uuid, CryptoError> {
        let hk = Hkdf::<Sha256>::new(Some(b"todo-server derived id"), &self.current().key);
        let mut b = [0u8; 16];
        hk.expand(context.as_bytes(), &mut b)
            .map_err(|_| CryptoError::Corrupt("key derivation"))?;
        Ok(uuid::Uuid::new_v8(b))
    }

    fn current(&self) -> &MasterKey {
        // Never empty: every constructor adds a key.
        &self.keys[0]
    }

    fn by_id(&self, id: &[u8]) -> Option<&MasterKey> {
        self.keys.iter().find(|k| k.id == id)
    }
}

/// The associated data of every chunk: header, object key and attachment ID, length-prefixed.
fn associated_data(header: &[u8], object_key: &str, attachment: &uuid::Uuid) -> Vec<u8> {
    let mut aad = Vec::with_capacity(header.len() + object_key.len() + 32);
    aad.extend_from_slice(header);
    aad.extend_from_slice(
        &u32::try_from(object_key.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    aad.extend_from_slice(object_key.as_bytes());
    aad.extend_from_slice(attachment.as_bytes());
    aad
}

/// The size of the sealed object for `plain` bytes of content.
#[must_use]
pub fn sealed_len(plain: u64) -> u64 {
    let chunk = CHUNK as u64;
    let chunks = plain.div_ceil(chunk).max(1);
    HEADER as u64 + plain + chunks * TAG as u64
}

/// Seals a stream of plaintext into `out`, chunk by chunk. Memory use is one chunk.
pub struct Sealer<W: Write> {
    out: W,
    enc: Option<EncryptorBE32<XChaCha20Poly1305>>,
    aad: Vec<u8>,
    buf: Vec<u8>,
    plain: u64,
    written: u64,
}

impl<W: Write> std::fmt::Debug for Sealer<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sealer")
            .field("plain", &self.plain)
            .finish_non_exhaustive()
    }
}

impl<W: Write> Sealer<W> {
    /// Starts an object for `attachment` stored under `object_key`, writing the header to `out`.
    pub fn new(
        ring: &KeyRing,
        object_key: &str,
        attachment: &uuid::Uuid,
        mut out: W,
    ) -> Result<Self, CryptoError> {
        let key = ring.current();
        let salt: [u8; SALT] = random()?;
        let prefix: [u8; NONCE_PREFIX] = random()?;
        let mut header = Vec::with_capacity(HEADER);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&key.id);
        header.extend_from_slice(&salt);
        header.extend_from_slice(&prefix);
        out.write_all(&header)?;
        let cipher = key.object_cipher(&salt)?;
        Ok(Self {
            out,
            enc: Some(EncryptorBE32::from_aead(cipher, (&prefix).into())),
            aad: associated_data(&header, object_key, attachment),
            buf: Vec::with_capacity(CHUNK),
            plain: 0,
            written: HEADER as u64,
        })
    }

    /// Plaintext bytes accepted so far.
    #[must_use]
    pub fn plain_len(&self) -> u64 {
        self.plain
    }

    /// Adds plaintext. A chunk is sealed once it is full *and* more data follows, so the last chunk
    /// always gets the "last" flag.
    pub fn push(&mut self, mut data: &[u8]) -> Result<(), CryptoError> {
        self.plain += data.len() as u64;
        while !data.is_empty() {
            if self.buf.len() == CHUNK {
                let enc = self.enc.as_mut().ok_or(CryptoError::Corrupt("finished"))?;
                let ct = enc
                    .encrypt_next(Payload {
                        msg: &self.buf,
                        aad: &self.aad,
                    })
                    .map_err(|_| CryptoError::Corrupt("too many chunks"))?;
                self.out.write_all(&ct)?;
                self.written += ct.len() as u64;
                self.buf.clear();
            }
            let take = (CHUNK - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
        }
        Ok(())
    }

    /// Seals the last chunk and returns the writer and the sealed length.
    pub fn finish(mut self) -> Result<(W, u64), CryptoError> {
        let enc = self.enc.take().ok_or(CryptoError::Corrupt("finished"))?;
        let ct = enc
            .encrypt_last(Payload {
                msg: &self.buf,
                aad: &self.aad,
            })
            .map_err(|_| CryptoError::Corrupt("last chunk"))?;
        self.out.write_all(&ct)?;
        self.written += ct.len() as u64;
        self.out.flush()?;
        Ok((self.out, self.written))
    }
}

/// Opens a sealed object chunk by chunk. Memory use is one chunk.
pub struct Opener<R: Read> {
    src: R,
    dec: Option<DecryptorBE32<XChaCha20Poly1305>>,
    aad: Vec<u8>,
    /// Ciphertext read ahead: up to one chunk plus one byte, to tell the last chunk.
    ahead: Vec<u8>,
    eof: bool,
}

impl<R: Read> std::fmt::Debug for Opener<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opener").finish_non_exhaustive()
    }
}

fn read_full(src: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match src.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

impl<R: Read> Opener<R> {
    /// Reads the header and picks the key. Fails for objects that are not ours or whose key is not in
    /// the ring.
    pub fn new(
        ring: &KeyRing,
        object_key: &str,
        attachment: &uuid::Uuid,
        mut src: R,
    ) -> Result<Self, CryptoError> {
        let mut header = [0u8; HEADER];
        if read_full(&mut src, &mut header)? != HEADER || &header[..MAGIC.len()] != MAGIC {
            return Err(CryptoError::Corrupt("not an attachment object"));
        }
        let id = &header[MAGIC.len()..MAGIC.len() + KEY_ID];
        let salt = &header[MAGIC.len() + KEY_ID..MAGIC.len() + KEY_ID + SALT];
        let prefix = &header[MAGIC.len() + KEY_ID + SALT..];
        let key = ring.by_id(id).ok_or(CryptoError::Corrupt(
            "sealed with a key that is not in the key file",
        ))?;
        let cipher = key.object_cipher(salt)?;
        Ok(Self {
            src,
            dec: Some(DecryptorBE32::from_aead(cipher, prefix.into())),
            aad: associated_data(&header, object_key, attachment),
            ahead: Vec::with_capacity(CHUNK + TAG + 1),
            eof: false,
        })
    }

    /// The next plaintext chunk, or `None` after the last one. Fails if any chunk was changed, the
    /// object was cut short or extended, or it belongs to another key or attachment.
    pub fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, CryptoError> {
        if self.dec.is_none() {
            return Ok(None);
        }
        let want = CHUNK + TAG + 1;
        if !self.eof && self.ahead.len() < want {
            let have = self.ahead.len();
            self.ahead.resize(want, 0);
            let n = read_full(&mut self.src, &mut self.ahead[have..])?;
            self.ahead.truncate(have + n);
            if have + n < want {
                self.eof = true;
            }
        }
        if self.ahead.len() > CHUNK + TAG {
            let rest = self.ahead.split_off(CHUNK + TAG);
            let ct = std::mem::replace(&mut self.ahead, rest);
            let dec = self.dec.as_mut().ok_or(CryptoError::Corrupt("finished"))?;
            let pt = dec
                .decrypt_next(Payload {
                    msg: &ct,
                    aad: &self.aad,
                })
                .map_err(|_| CryptoError::Corrupt("chunk authentication failed"))?;
            Ok(Some(pt))
        } else {
            let dec = self.dec.take().ok_or(CryptoError::Corrupt("finished"))?;
            let ct = std::mem::take(&mut self.ahead);
            let pt = dec
                .decrypt_last(Payload {
                    msg: &ct,
                    aad: &self.aad,
                })
                .map_err(|_| CryptoError::Corrupt("last chunk authentication failed"))?;
            Ok(Some(pt))
        }
    }

    /// Reads and checks the whole object into `out` (tests and small objects).
    pub fn copy_to(mut self, out: &mut impl Write) -> Result<u64, CryptoError> {
        let mut n = 0;
        while let Some(chunk) = self.next_chunk()? {
            out.write_all(&chunk)?;
            n += chunk.len() as u64;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn seal(ring: &KeyRing, key: &str, att: &Uuid, data: &[u8]) -> Vec<u8> {
        let mut s = Sealer::new(ring, key, att, Vec::new()).unwrap();
        // Uneven pushes exercise the chunk boundary logic.
        for part in data.chunks(7_777) {
            s.push(part).unwrap();
        }
        let (out, len) = s.finish().unwrap();
        assert_eq!(len, out.len() as u64);
        assert_eq!(len, sealed_len(data.len() as u64));
        out
    }

    fn open(ring: &KeyRing, key: &str, att: &Uuid, sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut out = Vec::new();
        Opener::new(ring, key, att, sealed)?.copy_to(&mut out)?;
        Ok(out)
    }

    #[test]
    fn derived_ids_are_stable_per_key_and_unpredictable_without_it() {
        let a = KeyRing::from_key([1; 32]);
        let b = KeyRing::from_key([2; 32]);
        let id = a.derive_uuid("budget warning task 2026-11").unwrap();
        assert_eq!(id, a.derive_uuid("budget warning task 2026-11").unwrap());
        assert_ne!(id, a.derive_uuid("budget warning task 2026-12").unwrap());
        assert_ne!(id, b.derive_uuid("budget warning task 2026-11").unwrap());
        assert_eq!(id.get_version_num(), 8);
    }

    #[test]
    fn round_trips_at_every_chunk_boundary() {
        let ring = KeyRing::generate().unwrap();
        let att = Uuid::new_v4();
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK, 3 * CHUNK + 5] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let sealed = seal(&ring, "a1/k", &att, &data);
            assert!(!sealed.windows(16).any(|w| len > 64 && w == &data[32..48]));
            assert_eq!(
                open(&ring, "a1/k", &att, &sealed).unwrap(),
                data,
                "len {len}"
            );
        }
    }

    #[test]
    fn each_object_gets_its_own_key_and_nonce() {
        let ring = KeyRing::generate().unwrap();
        let att = Uuid::new_v4();
        let a = seal(&ring, "a1/k", &att, b"same content");
        let b = seal(&ring, "a1/k", &att, b"same content");
        assert_ne!(a, b);
        assert_ne!(a[HEADER..], b[HEADER..]);
    }

    #[test]
    fn tampering_truncation_and_extension_are_detected() {
        let ring = KeyRing::generate().unwrap();
        let att = Uuid::new_v4();
        let data = vec![7u8; 2 * CHUNK + 10];
        let sealed = seal(&ring, "a1/k", &att, &data);
        for i in [0, 9, HEADER - 1, HEADER, HEADER + CHUNK, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[i] ^= 1;
            assert!(open(&ring, "a1/k", &att, &bad).is_err(), "flip at {i}");
        }
        // Cut at a chunk boundary: the remaining last chunk lacks the "last" flag.
        let cut = HEADER + 2 * (CHUNK + TAG);
        assert!(open(&ring, "a1/k", &att, &sealed[..cut]).is_err());
        assert!(open(&ring, "a1/k", &att, &sealed[..sealed.len() - 1]).is_err());
        let mut longer = sealed.clone();
        longer.extend_from_slice(&[0; TAG]);
        assert!(open(&ring, "a1/k", &att, &longer).is_err());
        // Two chunks swapped.
        let mut swapped = sealed.clone();
        let (a, b) = (HEADER, HEADER + CHUNK + TAG);
        let first = sealed[a..b].to_vec();
        swapped[a..b].copy_from_slice(&sealed[b..b + CHUNK + TAG]);
        swapped[b..b + CHUNK + TAG].copy_from_slice(&first);
        assert!(open(&ring, "a1/k", &att, &swapped).is_err());
    }

    #[test]
    fn swapping_objects_between_keys_or_attachments_is_detected() {
        let ring = KeyRing::generate().unwrap();
        let (att, other) = (Uuid::new_v4(), Uuid::new_v4());
        let sealed = seal(&ring, "a1/one", &att, b"secret");
        assert!(
            open(&ring, "a1/two", &att, &sealed).is_err(),
            "other object key"
        );
        assert!(
            open(&ring, "a1/one", &other, &sealed).is_err(),
            "other attachment"
        );
        assert!(open(&KeyRing::generate().unwrap(), "a1/one", &att, &sealed).is_err());
        assert_eq!(open(&ring, "a1/one", &att, &sealed).unwrap(), b"secret");
    }

    #[test]
    fn rotated_rings_read_old_objects() {
        let (old, new) = ([1u8; 32], [2u8; 32]);
        let att = Uuid::new_v4();
        let sealed = seal(&KeyRing::from_key(old), "a1/k", &att, b"x");
        let ring = KeyRing::from_key(new).with_old_key(old);
        assert_eq!(open(&ring, "a1/k", &att, &sealed).unwrap(), b"x");
        assert!(open(&KeyRing::from_key(new), "a1/k", &att, &sealed).is_err());
    }

    #[test]
    fn key_file_is_created_once_owner_only_and_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let att = Uuid::new_v4();
        let ring = KeyRing::load_or_create(dir.path()).unwrap();
        let sealed = seal(&ring, "a1/k", &att, b"hello");
        todo_fs::verify_private(&dir.path().join(KEY_FILE), todo_fs::Private::AnyFile).unwrap();
        let again = KeyRing::load_or_create(dir.path()).unwrap();
        assert_eq!(open(&again, "a1/k", &att, &sealed).unwrap(), b"hello");
        assert!(!format!("{again:?}").contains(&hex::encode(again.current().key)));
    }

    #[test]
    fn bad_key_files_are_refused_without_echoing_them() {
        let path = Path::new("k");
        let err = KeyRing::parse("not-a-key-secretvalue\n", path).unwrap_err();
        assert!(!err.to_string().contains("secretvalue"), "{err}");
        assert!(KeyRing::parse("# only a comment\n", path).is_err());
    }
}
