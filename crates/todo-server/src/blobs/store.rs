//! Where sealed attachment objects live: the [`BlobStore`] trait, a local-filesystem store (tests, and
//! running without R2) and an R2 store (S3 API, presigned SigV4 requests with `rusty-s3` and `ureq`, the
//! same approach as `todo-backup`).
//!
//! Stores only move ciphertext (see [`super::crypto`]) and do no accounting: every call is charged to the
//! budget by [`super::Attachments`] *before* it is made.

use rusty_s3::{Bucket, S3Action, UrlStyle};
use serde::Deserialize;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Prefix of every object key. The rest is 128 random bits in hex: opaque and unguessable, never derived
/// from a file name.
pub const KEY_PREFIX: &str = "a1/";

/// A fresh random object key.
pub fn new_object_key() -> Result<String, StoreError> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|_| StoreError::Config("no randomness".into()))?;
    Ok(format!("{KEY_PREFIX}{}", hex::encode(b)))
}

/// True for a key [`new_object_key`] could have made. Stores refuse anything else, so a key can never
/// name a path or another prefix.
#[must_use]
pub fn valid_object_key(key: &str) -> bool {
    key.strip_prefix(KEY_PREFIX).is_some_and(|h| {
        h.len() == 32
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Store failures. Messages never contain credentials or presigned URLs.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No such object.
    #[error("{op}: object not found")]
    NotFound {
        /// The operation.
        op: &'static str,
    },
    /// The store answered with an error status.
    #[error("{op}: HTTP {status}{}", code.as_ref().map(|c| format!(" ({c})")).unwrap_or_default())]
    Http {
        /// The operation.
        op: &'static str,
        /// The status.
        status: u16,
        /// The S3 error code, if any.
        code: Option<String>,
    },
    /// The request did not complete.
    #[error("{op}: {message}")]
    Transport {
        /// The operation.
        op: &'static str,
        /// What happened (URLs stripped of their query strings).
        message: String,
    },
    /// Local I/O failed.
    #[error("{op}: {source}")]
    Io {
        /// The operation.
        op: &'static str,
        /// The cause.
        source: io::Error,
    },
    /// Bad configuration or key.
    #[error("attachment store: {0}")]
    Config(String),
}

/// A bucket of sealed objects. Blocking: call from the blocking pool.
pub trait BlobStore: Send + Sync + std::fmt::Debug + 'static {
    /// Stores the whole of `body` (positioned at its start, exactly `len` bytes long) under `key`.
    fn put(&self, key: &str, body: &File, len: u64) -> Result<(), StoreError>;
    /// Opens the object for streaming.
    fn get(&self, key: &str) -> Result<Box<dyn Read + Send>, StoreError>;
    /// Deletes the object. Deleting a missing object succeeds.
    fn delete(&self, key: &str) -> Result<(), StoreError>;
    /// A short name for logs (`local` or `r2`).
    fn kind(&self) -> &'static str;
}

fn checked(key: &str) -> Result<(), StoreError> {
    if valid_object_key(key) {
        Ok(())
    } else {
        Err(StoreError::Config("malformed object key".into()))
    }
}

// ---- local ----------------------------------------------------------------------------------------

/// Objects as files in one owner-only directory (`<data_dir>/blobs` by default).
#[derive(Debug)]
pub struct LocalStore {
    dir: PathBuf,
}

impl LocalStore {
    /// Opens (creating, owner-only) the directory.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        let io = |source| StoreError::Io {
            op: "open local store",
            source,
        };
        match std::fs::metadata(dir) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(StoreError::Config(format!(
                    "{} is not a directory",
                    dir.display()
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                todo_fs::create_private_dir(dir).map_err(io)?;
            }
            Err(e) => return Err(io(e)),
        }
        Ok(Self {
            dir: dir.to_owned(),
        })
    }

    fn path(&self, key: &str) -> Result<PathBuf, StoreError> {
        checked(key)?;
        // Validated: `a1/` plus 32 hex digits.
        Ok(self.dir.join(&key[KEY_PREFIX.len()..]))
    }
}

impl BlobStore for LocalStore {
    fn put(&self, key: &str, body: &File, len: u64) -> Result<(), StoreError> {
        let path = self.path(key)?;
        let io = |source| StoreError::Io { op: "PUT", source };
        let tmp = path.with_extension("part");
        let _ = std::fs::remove_file(&tmp);
        let mut out = todo_fs::create_private_file(&tmp).map_err(io)?;
        let copied = io::copy(&mut body.take(len), &mut out).map_err(io)?;
        if copied != len {
            let _ = std::fs::remove_file(&tmp);
            return Err(io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short body",
            )));
        }
        out.sync_all().map_err(io)?;
        drop(out);
        todo_fs::replace_durably(&tmp, &path).map_err(io)
    }

    fn get(&self, key: &str) -> Result<Box<dyn Read + Send>, StoreError> {
        match File::open(self.path(key)?) {
            Ok(f) => Ok(Box::new(io::BufReader::new(f))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(StoreError::NotFound { op: "GET" })
            }
            Err(source) => Err(StoreError::Io { op: "GET", source }),
        }
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        match std::fs::remove_file(self.path(key)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StoreError::Io {
                op: "DELETE",
                source,
            }),
        }
    }

    fn kind(&self) -> &'static str {
        "local"
    }
}

// ---- R2 -------------------------------------------------------------------------------------------

/// The credentials file: a bucket-scoped R2 token. `Debug` never prints the keys.
///
/// ```json
/// {
///   "endpoint": "https://<account-id>.r2.cloudflarestorage.com",
///   "bucket": "<bucket>",
///   "access_key_id": "<access-key-id>",
///   "secret_access_key": "<secret-access-key>"
/// }
/// ```
#[derive(Clone, Deserialize)]
pub struct Credentials {
    /// The S3 endpoint.
    pub endpoint: String,
    /// The bucket (private: no public access, no custom domain).
    pub bucket: String,
    /// Access key ID.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: String,
    /// Region; R2 uses `auto`.
    #[serde(default = "auto")]
    pub region: String,
}

fn auto() -> String {
    "auto".into()
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("bucket", &self.bucket)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl Credentials {
    /// Reads the JSON credentials file. Parse errors give only the position, never file content.
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            StoreError::Config(format!(
                "cannot read credentials file {}: {e}",
                path.display()
            ))
        })?;
        serde_json::from_str(&text).map_err(|e| {
            StoreError::Config(format!(
                "credentials file {} is not valid (line {}, column {}); expected endpoint, bucket, \
                 access_key_id and secret_access_key",
                path.display(),
                e.line(),
                e.column()
            ))
        })
    }
}

/// Presigned URLs are used at once; the TTL only has to cover a slow upload.
const SIGN_TTL: Duration = Duration::from_secs(15 * 60);
const ERROR_BODY_LIMIT: u64 = 16 * 1024;

/// An S3-compatible bucket addressed path-style (what R2 expects).
pub struct R2Store {
    bucket: Bucket,
    credentials: rusty_s3::Credentials,
    agent: ureq::Agent,
}

impl std::fmt::Debug for R2Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2Store")
            .field("bucket", &self.bucket.name())
            .finish_non_exhaustive()
    }
}

impl R2Store {
    /// A store for the bucket in `creds`.
    pub fn new(creds: &Credentials) -> Result<Self, StoreError> {
        let endpoint: url::Url = creds
            .endpoint
            .parse()
            .map_err(|e| StoreError::Config(format!("endpoint is not a URL: {e}")))?;
        if endpoint.scheme() != "https" {
            return Err(StoreError::Config("endpoint must be https".into()));
        }
        let bucket = Bucket::new(
            endpoint,
            UrlStyle::Path,
            creds.bucket.clone(),
            creds.region.clone(),
        )
        .map_err(|e| StoreError::Config(e.to_string()))?;
        let credentials = rusty_s3::Credentials::new(
            creds.access_key_id.clone(),
            creds.secret_access_key.clone(),
        );
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // Every request is paid for before it is sent; a redirect would be a second, unpaid request
            // (and could carry a presigned URL elsewhere).
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(15 * 60)))
            .build();
        Ok(Self {
            bucket,
            credentials,
            agent: ureq::Agent::new_with_config(config),
        })
    }

    fn check(
        op: &'static str,
        resp: ureq::http::Response<ureq::Body>,
    ) -> Result<ureq::http::Response<ureq::Body>, StoreError> {
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(resp);
        }
        if status == 404 {
            return Err(StoreError::NotFound { op });
        }
        let mut body = String::new();
        let _ = resp
            .into_body()
            .into_with_config()
            .limit(ERROR_BODY_LIMIT)
            .reader()
            .read_to_string(&mut body);
        Err(StoreError::Http {
            op,
            status,
            code: xml_element(&body, "Code"),
        })
    }
}

/// ureq errors can quote the request URL, whose query string carries the signature.
fn transport(op: &'static str, e: &dyn std::fmt::Display) -> StoreError {
    StoreError::Transport {
        op,
        message: strip_queries(&e.to_string()),
    }
}

fn strip_queries(s: &str) -> String {
    s.split_whitespace()
        .map(|w| match w.find('?') {
            Some(i) if w.contains("://") => format!("{}?<redacted>", &w[..i]),
            _ => w.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&format!("</{name}>"))? + start;
    Some(body[start..end].chars().take(100).collect())
}

/// A response body reader whose errors never carry URLs.
struct Redacted(Box<dyn Read + Send>);

impl Read for Redacted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0
            .read(buf)
            .map_err(|e| io::Error::new(e.kind(), strip_queries(&e.to_string())))
    }
}

impl BlobStore for R2Store {
    fn put(&self, key: &str, body: &File, len: u64) -> Result<(), StoreError> {
        checked(key)?;
        let actual = body
            .metadata()
            .map_err(|source| StoreError::Io { op: "PUT", source })?
            .len();
        if actual != len {
            return Err(StoreError::Config("spool file has the wrong length".into()));
        }
        let url = self
            .bucket
            .put_object(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let resp = self
            .agent
            .put(url.as_str())
            .header("content-type", "application/octet-stream")
            // A file body is sent with its length (S3 does not take chunked uploads).
            .send(body)
            .map_err(|e| transport("PUT", &e))?;
        Self::check("PUT", resp).map(drop)
    }

    fn get(&self, key: &str) -> Result<Box<dyn Read + Send>, StoreError> {
        checked(key)?;
        let url = self
            .bucket
            .get_object(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let resp = self
            .agent
            .get(url.as_str())
            .call()
            .map_err(|e| transport("GET", &e))?;
        let resp = Self::check("GET", resp)?;
        Ok(Box::new(Redacted(Box::new(resp.into_body().into_reader()))))
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        checked(key)?;
        let url = self
            .bucket
            .delete_object(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let resp = self
            .agent
            .delete(url.as_str())
            .call()
            .map_err(|e| transport("DELETE", &e))?;
        match Self::check("DELETE", resp) {
            Ok(_) | Err(StoreError::NotFound { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn kind(&self) -> &'static str {
        "r2"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, Write};

    #[test]
    fn object_keys_are_random_and_strictly_checked() {
        let (a, b) = (new_object_key().unwrap(), new_object_key().unwrap());
        assert_ne!(a, b);
        assert!(valid_object_key(&a));
        for bad in [
            "a1/../../etc/passwd",
            "a1/ABCDEF00000000000000000000000000",
            "import/sha256/00",
            "a1/0",
            "",
            "report.pdf",
        ] {
            assert!(!valid_object_key(bad), "{bad}");
        }
    }

    #[test]
    fn local_store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalStore::open(&dir.path().join("blobs")).unwrap();
        let key = new_object_key().unwrap();
        let mut f = tempfile::tempfile().unwrap();
        f.write_all(b"sealed bytes").unwrap();
        f.rewind().unwrap();
        store.put(&key, &f, 12).unwrap();
        let mut got = Vec::new();
        store.get(&key).unwrap().read_to_end(&mut got).unwrap();
        assert_eq!(got, b"sealed bytes");
        store.delete(&key).unwrap();
        store.delete(&key).unwrap();
        assert!(matches!(store.get(&key), Err(StoreError::NotFound { .. })));
        assert!(store.get("a1/../x").is_err());
    }

    #[test]
    fn credentials_debug_is_redacted_and_errors_hide_signatures() {
        let c: Credentials = serde_json::from_str(
            r#"{"endpoint":"https://r2.example.invalid","bucket":"b","access_key_id":"AKID","secret_access_key":"SECRET"}"#,
        )
        .unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("AKID") && !dbg.contains("SECRET"));
        assert_eq!(c.region, "auto");
        let s = strip_queries("failed https://h.invalid/b/k?X-Amz-Signature=abc reset");
        assert!(!s.contains("abc"), "{s}");
        R2Store::new(&c).unwrap();
        let plain = Credentials {
            endpoint: "http://r2.example.invalid".into(),
            ..c
        };
        assert!(R2Store::new(&plain).is_err(), "plain http");
    }
}
