//! Object storage: a small trait, an S3 implementation (rusty-s3 + ureq), an in-memory one for tests, and the
//! [`Metered`] wrapper that charges the budget before every request.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::sync::Mutex;
use std::time::Duration;

use rusty_s3::{Bucket, S3Action, UrlStyle};

use crate::Result;
use crate::budget::{Budget, Clock, OpClass};
use crate::config::Credentials;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{op} {key}: not found")]
    NotFound { op: &'static str, key: String },
    #[error("{op} {key}: HTTP {status}{}", code.as_ref().map(|c| format!(" ({c})")).unwrap_or_default())]
    Http {
        op: &'static str,
        key: String,
        status: u16,
        code: Option<String>,
    },
    #[error("{op} {key}: {message}")]
    Transport {
        op: &'static str,
        key: String,
        message: String,
    },
    #[error("{op}: invalid response: {message}")]
    InvalidResponse { op: &'static str, message: String },
    #[error("invalid endpoint or bucket: {0}")]
    Config(String),
}

/// One page of a listing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListPage {
    pub keys: Vec<String>,
    pub next: Option<String>,
}

/// The S3 operations this tool uses. Implementations do no accounting; wrap them in [`Metered`].
pub trait ObjectStore {
    fn put(&self, key: &str, body: &[u8]) -> Result<(), StoreError>;
    fn create_multipart(&self, key: &str) -> Result<String, StoreError>;
    /// Returns the part's ETag.
    fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: u16,
        body: &[u8],
    ) -> Result<String, StoreError>;
    fn complete_multipart(
        &self,
        key: &str,
        upload_id: &str,
        etags: &[String],
    ) -> Result<(), StoreError>;
    fn abort_multipart(&self, key: &str, upload_id: &str) -> Result<(), StoreError>;
    /// Streams the object into `out` and returns its length.
    fn get(&self, key: &str, out: &mut dyn Write) -> Result<u64, StoreError>;
    fn list_page(&self, prefix: &str, continuation: Option<&str>) -> Result<ListPage, StoreError>;
}

/// The only way the rest of the crate reaches a store: every call charges the budget first, against the UTC
/// month of that moment, and refuses without sending anything when the cap is reached. The charge is on disk
/// before the request goes out.
pub struct Metered<'a, S: ObjectStore + ?Sized> {
    store: &'a S,
    budget: &'a mut Budget,
    clock: &'a dyn Clock,
}

impl<S: ObjectStore + ?Sized> std::fmt::Debug for Metered<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metered")
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl<'a, S: ObjectStore + ?Sized> Metered<'a, S> {
    pub fn new(store: &'a S, budget: &'a mut Budget, clock: &'a dyn Clock) -> Self {
        Self {
            store,
            budget,
            clock,
        }
    }

    pub fn budget(&self) -> &Budget {
        self.budget
    }

    /// Fails unless `n` more operations of `class` fit in this month's budget.
    pub fn check(&mut self, class: OpClass, n: u64) -> Result<()> {
        self.budget.check(class, n, self.clock.now())
    }

    /// Charges one request, against the month the clock shows right now.
    ///
    /// The charge is made and saved a moment before the request is sent. If a month boundary (00:00 UTC on
    /// the 1st) falls in that gap, the request is counted in the old month and R2 may bill it to the new one,
    /// so the new month can be undercounted. Requests are sent one at a time while holding the state lock, so
    /// only one can straddle a given boundary: the undercount is at most one request per month boundary. That
    /// is bounded and negligible against the default caps (2% of the free tier), so it is accepted rather than
    /// charging both months.
    fn charge(&mut self, class: OpClass) -> Result<()> {
        self.budget.charge(class, 1, self.clock.now())
    }

    pub fn put(&mut self, key: &str, body: &[u8]) -> Result<()> {
        self.charge(OpClass::A)?;
        Ok(self.store.put(key, body)?)
    }

    pub fn create_multipart(&mut self, key: &str) -> Result<String> {
        self.charge(OpClass::A)?;
        Ok(self.store.create_multipart(key)?)
    }

    pub fn upload_part(
        &mut self,
        key: &str,
        upload_id: &str,
        part_number: u16,
        body: &[u8],
    ) -> Result<String> {
        self.charge(OpClass::A)?;
        Ok(self.store.upload_part(key, upload_id, part_number, body)?)
    }

    pub fn complete_multipart(
        &mut self,
        key: &str,
        upload_id: &str,
        etags: &[String],
    ) -> Result<()> {
        self.charge(OpClass::A)?;
        Ok(self.store.complete_multipart(key, upload_id, etags)?)
    }

    pub fn abort_multipart(&mut self, key: &str, upload_id: &str) -> Result<()> {
        // R2 may not bill aborts, but count them as Class A to stay conservative.
        self.charge(OpClass::A)?;
        Ok(self.store.abort_multipart(key, upload_id)?)
    }

    pub fn get(&mut self, key: &str, out: &mut dyn Write) -> Result<u64> {
        self.charge(OpClass::B)?;
        Ok(self.store.get(key, out)?)
    }

    pub fn list_page(&mut self, prefix: &str, continuation: Option<&str>) -> Result<ListPage> {
        self.charge(OpClass::A)?;
        Ok(self.store.list_page(prefix, continuation)?)
    }
}

// ---------------------------------------------------------------------------------------------------------
// S3

/// Presigned URLs are used immediately; an hour leaves room for a slow multipart part.
const SIGN_TTL: Duration = Duration::from_secs(60 * 60);
/// Error bodies are small XML documents; read no more than this.
const ERROR_BODY_LIMIT: u64 = 16 * 1024;
const LIST_BODY_LIMIT: u64 = 8 * 1024 * 1024;

/// An S3-compatible bucket, addressed path-style (what R2 expects), with presigned SigV4 requests.
pub struct S3Store {
    bucket: Bucket,
    credentials: rusty_s3::Credentials,
    agent: ureq::Agent,
}

impl std::fmt::Debug for S3Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Store")
            .field("bucket", &self.bucket.name())
            .finish_non_exhaustive()
    }
}

impl S3Store {
    pub fn new(creds: &Credentials) -> Result<Self, StoreError> {
        let endpoint: url::Url = creds
            .endpoint
            .parse()
            .map_err(|e| StoreError::Config(format!("endpoint is not a URL: {e}")))?;
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
            // Every request is paid for before it is sent; a followed redirect would be a second, unpaid
            // request (and could send a presigned URL elsewhere). A 3xx is an error instead.
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(30 * 60)))
            .build();
        Ok(Self {
            bucket,
            credentials,
            agent: ureq::Agent::new_with_config(config),
        })
    }

    fn check(
        op: &'static str,
        key: &str,
        resp: ureq::http::Response<ureq::Body>,
    ) -> Result<ureq::http::Response<ureq::Body>, StoreError> {
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(resp);
        }
        if status == 404 {
            return Err(StoreError::NotFound {
                op,
                key: key.to_owned(),
            });
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
            key: key.to_owned(),
            status,
            code: xml_element(&body, "Code"),
        })
    }
}

/// ureq errors can quote the request URL, whose query string carries the credential scope and signature.
fn transport(op: &'static str, key: &str, e: ureq::Error) -> StoreError {
    StoreError::Transport {
        op,
        key: key.to_owned(),
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

impl ObjectStore for S3Store {
    fn put(&self, key: &str, body: &[u8]) -> Result<(), StoreError> {
        let url = self
            .bucket
            .put_object(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let resp = self
            .agent
            .put(url.as_str())
            .send(body)
            .map_err(|e| transport("PUT", key, e))?;
        Self::check("PUT", key, resp).map(drop)
    }

    fn create_multipart(&self, key: &str) -> Result<String, StoreError> {
        let url = self
            .bucket
            .create_multipart_upload(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let resp = self
            .agent
            .post(url.as_str())
            .send_empty()
            .map_err(|e| transport("CreateMultipartUpload", key, e))?;
        let mut resp = Self::check("CreateMultipartUpload", key, resp)?;
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| transport("CreateMultipartUpload", key, e))?;
        let parsed =
            rusty_s3::actions::CreateMultipartUpload::parse_response(&body).map_err(|e| {
                StoreError::InvalidResponse {
                    op: "CreateMultipartUpload",
                    message: e.to_string(),
                }
            })?;
        Ok(parsed.upload_id().to_owned())
    }

    fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: u16,
        body: &[u8],
    ) -> Result<String, StoreError> {
        let url = self
            .bucket
            .upload_part(Some(&self.credentials), key, part_number, upload_id)
            .sign(SIGN_TTL);
        let resp = self
            .agent
            .put(url.as_str())
            .send(body)
            .map_err(|e| transport("UploadPart", key, e))?;
        let resp = Self::check("UploadPart", key, resp)?;
        resp.headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or(StoreError::InvalidResponse {
                op: "UploadPart",
                message: "no ETag header".into(),
            })
    }

    fn complete_multipart(
        &self,
        key: &str,
        upload_id: &str,
        etags: &[String],
    ) -> Result<(), StoreError> {
        let action = self.bucket.complete_multipart_upload(
            Some(&self.credentials),
            key,
            upload_id,
            etags.iter().map(String::as_str),
        );
        let url = action.sign(SIGN_TTL);
        let body = action.body();
        let resp = self
            .agent
            .post(url.as_str())
            .send(body.as_bytes())
            .map_err(|e| transport("CompleteMultipartUpload", key, e))?;
        let mut resp = Self::check("CompleteMultipartUpload", key, resp)?;
        // S3 can answer 200 and still fail, with an error document in the body, and a cut-off body means we
        // don't know the outcome. Only a complete, parsed CompleteMultipartUploadResult counts as success.
        let text = resp
            .body_mut()
            .with_config()
            .limit(ERROR_BODY_LIMIT)
            .read_to_string()
            .map_err(|e| transport("CompleteMultipartUpload", key, e))?;
        if text.contains("<Error>") {
            return Err(StoreError::Http {
                op: "CompleteMultipartUpload",
                key: key.to_owned(),
                status: 200,
                code: xml_element(&text, "Code"),
            });
        }
        let parsed =
            rusty_s3::actions::CompleteMultipartUploadResponse::parse(&text).map_err(|e| {
                StoreError::InvalidResponse {
                    op: "CompleteMultipartUpload",
                    message: format!("no CompleteMultipartUploadResult: {e}"),
                }
            })?;
        if parsed.etag().is_empty() {
            return Err(StoreError::InvalidResponse {
                op: "CompleteMultipartUpload",
                message: "empty ETag".into(),
            });
        }
        Ok(())
    }

    fn abort_multipart(&self, key: &str, upload_id: &str) -> Result<(), StoreError> {
        let url = self
            .bucket
            .abort_multipart_upload(Some(&self.credentials), key, upload_id)
            .sign(SIGN_TTL);
        let resp = self
            .agent
            .delete(url.as_str())
            .call()
            .map_err(|e| transport("AbortMultipartUpload", key, e))?;
        Self::check("AbortMultipartUpload", key, resp).map(drop)
    }

    fn get(&self, key: &str, out: &mut dyn Write) -> Result<u64, StoreError> {
        let url = self
            .bucket
            .get_object(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let resp = self
            .agent
            .get(url.as_str())
            .call()
            .map_err(|e| transport("GET", key, e))?;
        let resp = Self::check("GET", key, resp)?;
        let mut reader = resp.into_body().into_reader();
        io::copy(&mut reader, out).map_err(|e| StoreError::Transport {
            op: "GET",
            key: key.to_owned(),
            message: strip_queries(&e.to_string()),
        })
    }

    fn list_page(&self, prefix: &str, continuation: Option<&str>) -> Result<ListPage, StoreError> {
        let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
        action.with_prefix(prefix);
        if let Some(token) = continuation {
            action.with_continuation_token(token);
        }
        let url = action.sign(SIGN_TTL);
        let resp = self
            .agent
            .get(url.as_str())
            .call()
            .map_err(|e| transport("LIST", prefix, e))?;
        let mut resp = Self::check("LIST", prefix, resp)?;
        let body = resp
            .body_mut()
            .with_config()
            .limit(LIST_BODY_LIMIT)
            .read_to_string()
            .map_err(|e| transport("LIST", prefix, e))?;
        let parsed = rusty_s3::actions::ListObjectsV2::parse_response(&body).map_err(|e| {
            StoreError::InvalidResponse {
                op: "LIST",
                message: e.to_string(),
            }
        })?;
        Ok(ListPage {
            keys: parsed.contents.into_iter().map(|c| c.key).collect(),
            next: parsed.next_continuation_token,
        })
    }
}

// ---------------------------------------------------------------------------------------------------------
// In memory

/// An in-memory store for tests. It records every operation it serves.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<MemoryInner>,
}

#[derive(Debug, Default)]
struct MemoryInner {
    objects: BTreeMap<String, Vec<u8>>,
    uploads: BTreeMap<String, (String, BTreeMap<u16, Vec<u8>>)>,
    next_upload: u64,
    ops: Vec<&'static str>,
    page_size: Option<usize>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes listings return at most `n` keys per page.
    pub fn with_page_size(n: usize) -> Self {
        let s = Self::default();
        s.lock().page_size = Some(n);
        s
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryInner> {
        // A panic in another test thread must not cascade.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn keys(&self) -> Vec<String> {
        self.lock().objects.keys().cloned().collect()
    }

    pub fn object(&self, key: &str) -> Option<Vec<u8>> {
        self.lock().objects.get(key).cloned()
    }

    pub fn insert(&self, key: &str, body: Vec<u8>) {
        self.lock().objects.insert(key.to_owned(), body);
    }

    /// Operation names in the order they were served: `PUT`, `GET`, `LIST`, `CREATE`, `PART`, `COMPLETE`,
    /// `ABORT`.
    pub fn ops(&self) -> Vec<&'static str> {
        self.lock().ops.clone()
    }

    pub fn open_uploads(&self) -> usize {
        self.lock().uploads.len()
    }
}

impl ObjectStore for MemoryStore {
    fn put(&self, key: &str, body: &[u8]) -> Result<(), StoreError> {
        let mut s = self.lock();
        s.ops.push("PUT");
        s.objects.insert(key.to_owned(), body.to_vec());
        Ok(())
    }

    fn create_multipart(&self, key: &str) -> Result<String, StoreError> {
        let mut s = self.lock();
        s.ops.push("CREATE");
        s.next_upload += 1;
        let id = format!("upload-{}", s.next_upload);
        s.uploads
            .insert(id.clone(), (key.to_owned(), BTreeMap::new()));
        Ok(id)
    }

    fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: u16,
        body: &[u8],
    ) -> Result<String, StoreError> {
        let mut s = self.lock();
        s.ops.push("PART");
        let (k, parts) = s.uploads.get_mut(upload_id).ok_or(StoreError::NotFound {
            op: "PART",
            key: key.to_owned(),
        })?;
        if k != key {
            return Err(StoreError::InvalidResponse {
                op: "PART",
                message: "key mismatch".into(),
            });
        }
        parts.insert(part_number, body.to_vec());
        Ok(format!("\"etag-{part_number}\""))
    }

    fn complete_multipart(
        &self,
        key: &str,
        upload_id: &str,
        etags: &[String],
    ) -> Result<(), StoreError> {
        let mut s = self.lock();
        s.ops.push("COMPLETE");
        let (k, parts) = s.uploads.remove(upload_id).ok_or(StoreError::NotFound {
            op: "COMPLETE",
            key: key.to_owned(),
        })?;
        let expected: Vec<String> = (1..=parts.len()).map(|n| format!("\"etag-{n}\"")).collect();
        if k != key || etags != expected.as_slice() {
            return Err(StoreError::InvalidResponse {
                op: "COMPLETE",
                message: "parts do not match".into(),
            });
        }
        let body = parts.into_values().flatten().collect();
        s.objects.insert(key.to_owned(), body);
        Ok(())
    }

    fn abort_multipart(&self, _key: &str, upload_id: &str) -> Result<(), StoreError> {
        let mut s = self.lock();
        s.ops.push("ABORT");
        s.uploads.remove(upload_id);
        Ok(())
    }

    fn get(&self, key: &str, out: &mut dyn Write) -> Result<u64, StoreError> {
        let body = {
            let mut s = self.lock();
            s.ops.push("GET");
            s.objects.get(key).cloned()
        };
        let body = body.ok_or(StoreError::NotFound {
            op: "GET",
            key: key.to_owned(),
        })?;
        out.write_all(&body).map_err(|e| StoreError::Transport {
            op: "GET",
            key: key.to_owned(),
            message: e.to_string(),
        })?;
        Ok(body.len() as u64)
    }

    fn list_page(&self, prefix: &str, continuation: Option<&str>) -> Result<ListPage, StoreError> {
        let mut s = self.lock();
        s.ops.push("LIST");
        let page_size = s.page_size.unwrap_or(1000);
        let mut keys: Vec<String> = s
            .objects
            .keys()
            .filter(|k| k.starts_with(prefix))
            .filter(|k| continuation.is_none_or(|after| k.as_str() > after))
            .cloned()
            .collect();
        let next = if keys.len() > page_size {
            keys.truncate(page_size);
            keys.last().cloned()
        } else {
            None
        };
        Ok(ListPage { keys, next })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_queries_hides_signatures() {
        let msg =
            "io: failed https://h.invalid/b/k?X-Amz-Credential=AKID%2F&X-Amz-Signature=abc reset";
        let s = strip_queries(msg);
        assert!(!s.contains("AKID") && !s.contains("abc"), "{s}");
        assert!(s.contains("https://h.invalid/b/k?<redacted>"));
    }

    #[test]
    fn xml_code_is_extracted() {
        let body = "<Error><Code>AccessDenied</Code><Message>no</Message></Error>";
        assert_eq!(xml_element(body, "Code").as_deref(), Some("AccessDenied"));
        assert_eq!(xml_element("garbage", "Code"), None);
    }
}
