//! A tiny S3 imitation on 127.0.0.1, enough for PUT, GET, ListObjectsV2 and multipart uploads. It checks the
//! presigned SigV4 query parameters loosely (algorithm, credential scope, signature shape) but does not
//! recompute signatures.

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

pub const BUCKET: &str = "backups";
pub const ACCESS_KEY_ID: &str = "test-access-key-id";
pub const SECRET_ACCESS_KEY: &str = "test-secret-access-key-do-not-log";

#[derive(Default)]
struct Inner {
    objects: BTreeMap<String, Vec<u8>>,
    uploads: BTreeMap<String, (String, BTreeMap<u16, Vec<u8>>)>,
    next_upload: u64,
    log: Vec<String>,
    page_size: usize,
}

#[derive(Clone)]
pub struct FakeS3 {
    pub endpoint: String,
    inner: Arc<Mutex<Inner>>,
}

impl FakeS3 {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let inner = Arc::new(Mutex::new(Inner {
            page_size: 1000,
            ..Inner::default()
        }));
        let shared = inner.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = shared.clone();
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        Self { endpoint, inner }
    }

    pub fn credentials_json(&self) -> String {
        format!(
            r#"{{"endpoint":"{}","bucket":"{BUCKET}","access_key_id":"{ACCESS_KEY_ID}","secret_access_key":"{SECRET_ACCESS_KEY}"}}"#,
            self.endpoint
        )
    }

    /// `"<METHOD> <operation> <key>"` per request served.
    pub fn log(&self) -> Vec<String> {
        self.inner.lock().unwrap().log.clone()
    }

    pub fn keys(&self) -> Vec<String> {
        self.inner.lock().unwrap().objects.keys().cloned().collect()
    }

    pub fn object(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().objects.get(key).cloned()
    }

    pub fn insert(&self, key: &str, body: Vec<u8>) {
        self.inner.lock().unwrap().objects.insert(key.into(), body);
    }

    pub fn set_page_size(&self, n: usize) {
        self.inner.lock().unwrap().page_size = n;
    }

    pub fn open_uploads(&self) -> usize {
        self.inner.lock().unwrap().uploads.len()
    }
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    fn error(status: u16, code: &str) -> Self {
        Self::new(
            status,
            format!(
                "<?xml version=\"1.0\"?><Error><Code>{code}</Code><Message>fake</Message></Error>"
            ),
        )
    }

    fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

fn serve(stream: TcpStream, inner: &Mutex<Inner>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
            return;
        };
        let (method, target) = (method.to_owned(), target.to_owned());
        let mut content_length = 0usize;
        let mut chunked = false;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let k = k.trim().to_ascii_lowercase();
                if k == "content-length" {
                    content_length = v.trim().parse().unwrap_or(0);
                } else if k == "transfer-encoding" {
                    chunked = true;
                }
            }
        }
        let resp = if chunked {
            // S3 needs a Content-Length on uploads; so does this fake.
            Response::error(411, "MissingContentLength")
        } else {
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            handle(&method, &target, body, inner)
        };
        let mut head = format!(
            "HTTP/1.1 {} X\r\nContent-Length: {}\r\n",
            resp.status,
            resp.body.len()
        );
        for (k, v) in &resp.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        if writer.write_all(head.as_bytes()).is_err() || writer.write_all(&resp.body).is_err() {
            return;
        }
    }
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

fn signature_ok(q: &BTreeMap<String, String>) -> bool {
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let credential = get("X-Amz-Credential");
    let signature = get("X-Amz-Signature");
    get("X-Amz-Algorithm") == "AWS4-HMAC-SHA256"
        && credential.starts_with(&format!("{ACCESS_KEY_ID}/"))
        && credential.ends_with("/auto/s3/aws4_request")
        && signature.len() == 64
        && signature.bytes().all(|b| b.is_ascii_hexdigit())
        && get("X-Amz-SignedHeaders").split(';').any(|h| h == "host")
        && get("X-Amz-Expires")
            .parse::<u64>()
            .is_ok_and(|e| (1..=604_800).contains(&e))
        && get("X-Amz-Date").len() == 16
}

fn handle(method: &str, target: &str, body: Vec<u8>, inner: &Mutex<Inner>) -> Response {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let q: BTreeMap<String, String> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect();
    if !signature_ok(&q) {
        return Response::error(403, "SignatureDoesNotMatch");
    }
    let Some(rest) = path.strip_prefix(&format!("/{BUCKET}")) else {
        return Response::error(404, "NoSuchBucket");
    };
    let key = decode(rest.trim_start_matches('/'));
    let mut s = inner.lock().unwrap();

    match method {
        "GET" if q.get("list-type").map(String::as_str) == Some("2") => {
            let prefix = q.get("prefix").cloned().unwrap_or_default();
            s.log.push(format!("GET LIST {prefix}"));
            let after = q.get("continuation-token").cloned();
            let mut keys: Vec<(String, usize)> = s
                .objects
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .filter(|(k, _)| after.as_ref().is_none_or(|a| k.as_str() > a.as_str()))
                .map(|(k, v)| (k.clone(), v.len()))
                .collect();
            let truncated = keys.len() > s.page_size;
            keys.truncate(s.page_size);
            let mut xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult \
                 xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>{BUCKET}</Name><Prefix>{prefix}</Prefix>\
                 <KeyCount>{}</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>{truncated}</IsTruncated>",
                keys.len()
            );
            for (k, len) in &keys {
                xml.push_str(&format!(
                    "<Contents><Key>{k}</Key><LastModified>2026-11-21T00:00:00.000Z</LastModified>\
                     <ETag>\"x\"</ETag><Size>{len}</Size><StorageClass>STANDARD</StorageClass></Contents>"
                ));
            }
            if truncated {
                xml.push_str(&format!(
                    "<NextContinuationToken>{}</NextContinuationToken>",
                    keys.last().unwrap().0
                ));
            }
            xml.push_str("</ListBucketResult>");
            Response::new(200, xml)
        }
        "GET" => {
            s.log.push(format!("GET OBJECT {key}"));
            match s.objects.get(&key) {
                Some(b) => Response::new(200, b.clone()),
                None => Response::error(404, "NoSuchKey"),
            }
        }
        "PUT" if q.contains_key("uploadId") => {
            let n: u16 = q["partNumber"].parse().unwrap();
            s.log.push(format!("PUT PART {key}"));
            match s.uploads.get_mut(&q["uploadId"]) {
                Some((k, parts)) if *k == key => {
                    parts.insert(n, body);
                    Response::new(200, "").header("ETag", &format!("\"part-{n}\""))
                }
                _ => Response::error(404, "NoSuchUpload"),
            }
        }
        "PUT" => {
            s.log.push(format!("PUT OBJECT {key}"));
            s.objects.insert(key, body);
            Response::new(200, "").header("ETag", "\"x\"")
        }
        "POST" if q.contains_key("uploads") => {
            s.log.push(format!("POST CREATE {key}"));
            s.next_upload += 1;
            let id = format!("up{}", s.next_upload);
            s.uploads.insert(id.clone(), (key.clone(), BTreeMap::new()));
            Response::new(
                200,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><InitiateMultipartUploadResult \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Bucket>{BUCKET}</Bucket><Key>{key}</Key>\
                     <UploadId>{id}</UploadId></InitiateMultipartUploadResult>"
                ),
            )
        }
        "POST" if q.contains_key("uploadId") => {
            s.log.push(format!("POST COMPLETE {key}"));
            let Some((k, parts)) = s.uploads.remove(&q["uploadId"]) else {
                return Response::error(404, "NoSuchUpload");
            };
            let text = String::from_utf8_lossy(&body);
            for n in parts.keys() {
                let expected = format!("<PartNumber>{n}</PartNumber>");
                if !text.contains(&expected) || !text.contains(&format!("part-{n}")) {
                    return Response::error(400, "InvalidPart");
                }
            }
            if k != key {
                return Response::error(400, "InvalidPart");
            }
            let obj: Vec<u8> = parts.into_values().flatten().collect();
            s.objects.insert(key.clone(), obj);
            Response::new(
                200,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><CompleteMultipartUploadResult \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Key>{key}</Key><ETag>\"m\"</ETag>\
                     </CompleteMultipartUploadResult>"
                ),
            )
        }
        "DELETE" if q.contains_key("uploadId") => {
            s.log.push(format!("DELETE ABORT {key}"));
            s.uploads.remove(&q["uploadId"]);
            Response::new(204, "")
        }
        _ => Response::error(405, "MethodNotAllowed"),
    }
}
