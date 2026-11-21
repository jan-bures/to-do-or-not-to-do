//! zstd + age: sealing a snapshot for upload, opening a downloaded object, and key generation.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;

use age::secrecy::ExposeSecret;
use age::x25519::{Identity, Recipient};

use crate::{Error, Result};

/// Compresses `input` with zstd and encrypts it to `recipient`, writing the age file to `output`. Returns the
/// size of the sealed file.
pub fn seal(input: &Path, output: &Path, recipient: &Recipient, zstd_level: i32) -> Result<u64> {
    let ctx = || format!("sealing {}", input.display());
    let mut src = BufReader::new(File::open(input).map_err(Error::io(ctx()))?);
    let out = File::create_new(output).map_err(Error::io(ctx()))?;

    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
            .map_err(|e| Error::Crypto(e.to_string()))?;
    let age_writer = encryptor
        .wrap_output(BufWriter::new(out))
        .map_err(Error::io(ctx()))?;
    let mut zstd_writer = zstd::Encoder::new(age_writer, zstd_level).map_err(Error::io(ctx()))?;
    io::copy(&mut src, &mut zstd_writer).map_err(Error::io(ctx()))?;
    let age_writer = zstd_writer.finish().map_err(Error::io(ctx()))?;
    let mut buffered = age_writer.finish().map_err(Error::io(ctx()))?;
    buffered.flush().map_err(Error::io(ctx()))?;
    let file = buffered
        .into_inner()
        .map_err(|e| Error::io(ctx())(e.into_error()))?;
    file.sync_all().map_err(Error::io(ctx()))?;
    Ok(file.metadata().map_err(Error::io(ctx()))?.len())
}

/// Decrypts and decompresses the age file `input` into `out`. Fails if no identity matches, or if any byte of
/// the object was changed (age authenticates every chunk).
pub fn open(input: &Path, out: &mut File, identities: &[Identity]) -> Result<u64> {
    let ctx = || format!("decrypting {}", input.display());
    let src = BufReader::new(File::open(input).map_err(Error::io(ctx()))?);
    let decryptor = age::Decryptor::new_buffered(src)
        .map_err(|e| Error::Crypto(format!("not a valid age file: {e}")))?;
    let plaintext = decryptor
        .decrypt(identities.iter().map(|i| i as &dyn age::Identity))
        .map_err(|e| Error::Crypto(format!("cannot decrypt: {e}")))?;
    let mut decoder = zstd::Decoder::new(plaintext).map_err(Error::io(ctx()))?;
    let mut writer = BufWriter::new(&mut *out);
    let n = io::copy(&mut decoder, &mut writer)
        .map_err(|e| Error::Crypto(format!("object is corrupt or truncated: {e}")))?;
    writer.flush().map_err(Error::io(ctx()))?;
    drop(writer);
    out.sync_all().map_err(Error::io(ctx()))?;
    Ok(n)
}

/// Reads X25519 identities from an age identity file (`AGE-SECRET-KEY-1...` lines; `#` comments and blank
/// lines are skipped).
pub fn load_identities(path: &Path) -> Result<Vec<Identity>> {
    let text = std::fs::read_to_string(path).map_err(Error::io(format!(
        "reading identity file {}",
        path.display()
    )))?;
    let mut ids = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Never echo the line: it is a secret key.
        let id: Identity = line.parse().map_err(|_| {
            Error::Crypto(format!(
                "line {} of {} is not an age X25519 identity",
                n + 1,
                path.display()
            ))
        })?;
        ids.push(id);
    }
    if ids.is_empty() {
        return Err(Error::Crypto(format!(
            "{} contains no identity",
            path.display()
        )));
    }
    Ok(ids)
}

/// Generates a new identity and writes it to `path`, which must not exist. Returns the public recipient.
pub fn keygen(path: &Path, created: jiff::Timestamp) -> Result<Recipient> {
    let identity = Identity::generate();
    let recipient = identity.to_public();
    let ctx = || format!("writing identity file {}", path.display());
    let mut file = File::create_new(path).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Error::OutputExists(path.to_path_buf())
        } else {
            Error::io(ctx())(e)
        }
    })?;
    let secret = identity.to_string();
    let body = format!(
        "# todo-backup identity. Store it in a password manager, then delete this file from the backup host.\n\
         # created: {}\n# public key: {recipient}\n{}\n",
        created.strftime("%Y-%m-%dT%H:%M:%SZ"),
        secret.expose_secret()
    );
    file.write_all(body.as_bytes()).map_err(Error::io(ctx()))?;
    file.sync_all().map_err(Error::io(ctx()))?;
    Ok(recipient)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("plain");
        let data: Vec<u8> = (0..300_000u32)
            .flat_map(|i| (i % 251).to_le_bytes())
            .collect();
        std::fs::write(&plain, &data).expect("write");

        let id = Identity::generate();
        let sealed = dir.path().join("sealed.age");
        let sealed_len = seal(&plain, &sealed, &id.to_public(), 3).expect("seal");
        assert!(sealed_len > 0);
        // The sealed file must not contain the plaintext.
        let sealed_bytes = std::fs::read(&sealed).expect("read");
        assert!(sealed_bytes.starts_with(b"age-encryption.org/v1"));
        assert!(!sealed_bytes.windows(64).any(|w| w == &data[1000..1064]));

        let out = dir.path().join("out");
        let mut file = File::create_new(&out).expect("create");
        assert_eq!(
            open(&sealed, &mut file, &[id]).expect("open"),
            data.len() as u64
        );
        drop(file);
        assert_eq!(std::fs::read(&out).expect("read"), data);
    }

    #[test]
    fn wrong_identity_and_tampering_are_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("plain");
        std::fs::write(&plain, vec![7u8; 200_000]).expect("write");
        let id = Identity::generate();
        let sealed = dir.path().join("sealed.age");
        seal(&plain, &sealed, &id.to_public(), 3).expect("seal");

        let other = Identity::generate();
        assert!(matches!(
            open(&sealed, &mut tempfile::tempfile().expect("tmp"), &[other]),
            Err(Error::Crypto(_))
        ));

        let mut bytes = std::fs::read(&sealed).expect("read");
        let last = bytes.len() - 5;
        bytes[last] ^= 0x01;
        std::fs::write(&sealed, &bytes).expect("write");
        assert!(open(&sealed, &mut tempfile::tempfile().expect("tmp"), &[id]).is_err());
    }

    #[test]
    fn keygen_writes_a_loadable_identity_and_never_overwrites() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("id.txt");
        let now = jiff::Timestamp::UNIX_EPOCH;
        let recipient = keygen(&path, now).expect("keygen");
        let ids = load_identities(&path).expect("load");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].to_public().to_string(), recipient.to_string());
        assert!(matches!(keygen(&path, now), Err(Error::OutputExists(_))));
    }
}
