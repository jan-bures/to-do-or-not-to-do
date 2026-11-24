//! The web app (`apps/web`) at `/app`.
//!
//! The built app is a directory (`[web] dir`): `index.html`, the wasm module and its JavaScript glue,
//! the stylesheets and the fonts. [`WebApp::load`] reads all of it into memory at start-up, so a
//! request never touches the file system: a path is looked up in a map, and nothing outside the
//! directory can be named. A deploy replaces the directory and restarts the server.
//!
//! # Who gets it
//!
//! Every `/app` route is behind the owner's session ([`OwnerSession`]); a signed-out browser is sent to
//! `/login?next=/app...`. The app then talks to `/api/v1` with the same session cookie: the API
//! accepts it only with the session's CSRF token in `X-CSRF-Token` on every request, and the right
//! `Origin` on every write (see [`crate::auth::require_auth_or_session`]). The token reaches the app
//! in a `<meta name="csrf-token">` tag that this module writes into `index.html` per request, so no
//! credential is ever stored in the browser.
//!
//! # Headers
//!
//! `index.html` gets [`APP_CSP`]: app resources are same-origin, and `'wasm-unsafe-eval'` lets the
//! browser compile the module. No inline script or style is allowed. It carries the CSRF token, so it is
//! `no-store, no-transform` ([`NO_STORE`]): never cached, and never compressed by a proxy (BREACH).
//! The other files hold no secret and are revalidated with an `ETag` (`private, no-cache`).
//!
//! # A stale token
//!
//! `GET /auth/session` ([`session_token`]) answers the session's current CSRF token as JSON (or 401
//! without a session). A tab whose session changed under it (the owner signed in again in another
//! tab, replacing the cookie) gets `403 csrf` from the API, fetches the new token there and retries.
//! Like the page, it needs the cookie, which `SameSite=Strict` keeps off cross-site requests, and
//! another origin can't read the answer (no CORS). It also answers only this origin's own pages: a
//! request whose `Sec-Fetch-Site` is anything but `same-origin`, or whose `Origin` is not the public
//! origin, is `403 csrf` (a sibling origin on the same site would otherwise get the cookie). And it
//! never counts as activity: only a request that passes the API's CSRF check keeps a session alive.
//!
//! # Compression
//!
//! The build script writes a gzipped copy next to the larger files (`todo-web_bg.wasm.gz`, ...). A
//! client that accepts gzip gets that copy with `Content-Encoding: gzip`; the server never compresses
//! anything itself. The wasm module shrinks from about 4.5 MB to 0.7 MB this way.

use crate::app::AppState;
use crate::auth::session::OwnerSession;
use crate::web::NO_STORE;
use axum::extract::{Path, State};
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG,
    IF_NONE_MATCH, VARY,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path as FsPath;

/// Direct model connections are limited to Tailscale Serve HTTPS and HTTP loopback.
/// Other assets stay same-origin. Keep in sync with the browser's model endpoint validation.
pub const APP_CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self'; img-src 'self' data: blob:; connect-src 'self' https://*.ts.net http://127.0.0.1:* http://localhost:*; font-src 'self'; \
worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";

const WORKER_CSP: &str =
    "default-src 'none'; script-src 'self'; connect-src 'self'; img-src 'self'";
const WORKER: &str = "reminder-worker.js";

pub(crate) fn is_worker_script(path: &str) -> bool {
    matches!(path, "/app/reminder-worker.js" | "/app/reminder-browser.js")
}

/// The marker in `index.html` that is replaced with the session's CSRF meta tag.
pub const SESSION_MARKER: &str = "<!--todo-session-->";

/// Largest app the server loads (all files together).
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// Most files the server loads.
pub const MAX_FILES: usize = 512;

/// Why the web app could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum WebAppError {
    /// Reading the directory failed.
    #[error("reading the web app at {path}: {source}")]
    Io {
        /// The file or directory.
        path: String,
        /// The cause.
        source: std::io::Error,
    },
    /// The directory is not a usable build.
    #[error("the web app at {0}")]
    Invalid(String),
}

#[derive(Debug)]
struct File {
    body: Vec<u8>,
    content_type: &'static str,
    etag: String,
    /// The gzipped copy the build wrote next to it (`<name>.gz`), with its own ETag.
    gzip: Option<(Vec<u8>, String)>,
}

fn etag(body: &[u8]) -> String {
    format!("\"{}\"", &hex::encode(Sha256::digest(body))[..32])
}

/// The built web app, in memory.
#[derive(Debug)]
pub struct WebApp {
    /// `index.html`, split at [`SESSION_MARKER`].
    index: (String, String),
    /// Every other file by its path relative to the directory (`/`-separated).
    files: HashMap<String, File>,
}

fn content_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "ttf" => "font/ttf",
        "woff2" => "font/woff2",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

impl WebApp {
    /// Reads the build in `dir`. Files with unknown extensions are skipped; `index.html` must exist
    /// and hold [`SESSION_MARKER`] exactly once.
    pub fn load(dir: &FsPath) -> Result<Self, WebAppError> {
        let mut entries = Vec::new();
        let mut total = 0u64;
        let mut stack = vec![(dir.to_owned(), String::new())];
        while let Some((path, prefix)) = stack.pop() {
            let io = |source| WebAppError::Io {
                path: path.display().to_string(),
                source,
            };
            for entry in std::fs::read_dir(&path).map_err(io)? {
                let entry = entry.map_err(io)?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let rel = format!("{prefix}{name}");
                let kind = entry.file_type().map_err(io)?;
                if kind.is_dir() {
                    stack.push((entry.path(), format!("{rel}/")));
                    continue;
                }
                if content_type(name.strip_suffix(".gz").unwrap_or(&name)).is_none() {
                    continue;
                }
                let body = std::fs::read(entry.path()).map_err(|source| WebAppError::Io {
                    path: entry.path().display().to_string(),
                    source,
                })?;
                total += body.len() as u64;
                if total > MAX_TOTAL_BYTES || entries.len() >= MAX_FILES {
                    return Err(WebAppError::Invalid(format!(
                        "{} is larger than {MAX_TOTAL_BYTES} bytes or {MAX_FILES} files",
                        dir.display()
                    )));
                }
                entries.push((rel, body));
            }
        }
        let (index, files) = Self::assemble(entries);
        let index = index
            .ok_or_else(|| WebAppError::Invalid(format!("{} has no index.html", dir.display())))?;
        let index = String::from_utf8(index)
            .map_err(|_| WebAppError::Invalid("index.html is not UTF-8".into()))?;
        let index = Self::split_index(&index)?;
        Ok(Self { index, files })
    }

    /// Sorts files into `index.html` and the rest, attaching each `<name>.gz` to `<name>` (a `.gz`
    /// without its original is dropped) and skipping unknown types.
    fn assemble(entries: Vec<(String, Vec<u8>)>) -> (Option<Vec<u8>>, HashMap<String, File>) {
        let mut index = None;
        let mut files = HashMap::new();
        let mut gz = Vec::new();
        for (name, body) in entries {
            if name == "index.html" {
                index = Some(body);
            } else if let Some(base) = name.strip_suffix(".gz") {
                gz.push((base.to_owned(), body));
            } else if let Some(content_type) = content_type(&name) {
                let etag = etag(&body);
                files.insert(
                    name,
                    File {
                        body,
                        content_type,
                        etag,
                        gzip: None,
                    },
                );
            }
        }
        for (base, body) in gz {
            if let Some(f) = files.get_mut(&base) {
                let tag = etag(&body);
                f.gzip = Some((body, tag));
            }
        }
        (index, files)
    }

    fn split_index(index: &str) -> Result<(String, String), WebAppError> {
        if index.matches(SESSION_MARKER).count() != 1 {
            return Err(WebAppError::Invalid(format!(
                "index.html must contain {SESSION_MARKER} exactly once"
            )));
        }
        let (a, b) = index
            .split_once(SESSION_MARKER)
            .ok_or_else(|| WebAppError::Invalid("index.html has no session marker".into()))?;
        Ok((a.to_owned(), b.to_owned()))
    }

    /// Builds an app from memory (tests).
    pub fn from_parts(
        index_html: &str,
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, WebAppError> {
        let index = Self::split_index(index_html)?;
        let (_, files) = Self::assemble(files.into_iter().collect());
        Ok(Self { index, files })
    }

    /// The page for a session: `index.html` with the CSRF token and browser storage scope.
    #[must_use]
    pub fn page(&self, csrf: &str, owner_scope: &str) -> String {
        format!(
            "{}<meta name=\"csrf-token\" content=\"{}\"><meta name=\"todo-owner-scope\" content=\"{}\">{}",
            self.index.0,
            crate::web::html::esc(csrf),
            crate::web::html::esc(owner_scope),
            self.index.1
        )
    }
}

async fn page_response(app: &AppState, web: &WebApp, session: &OwnerSession) -> Response {
    let owner_scope =
        match crate::auth::session::blocking(&app.authsvc, |svc| svc.browser_owner_scope()).await {
            Ok(Ok(scope)) => scope,
            Ok(Err(error)) | Err(error) => return error.into_response(),
        };
    (
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (CONTENT_SECURITY_POLICY, HeaderValue::from_static(APP_CSP)),
            (CACHE_CONTROL, HeaderValue::from_static(NO_STORE)),
        ],
        web.page(&session.row.csrf, &owner_scope),
    )
        .into_response()
}

/// Whether the request accepts gzip: an explicit `gzip` entry decides (refused with `q=0`, even
/// next to `*;q=1`); without one, a `*` entry does.
fn accepts_gzip(headers: &HeaderMap) -> bool {
    let (mut gzip, mut any) = (None, None);
    for enc in headers
        .get_all(ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
    {
        let mut parts = enc.split(';').map(str::trim);
        let name = parts.next().unwrap_or_default();
        let q = parts
            .find_map(|p| p.strip_prefix("q="))
            .map_or(Some(1.0), |q| q.trim().parse::<f32>().ok());
        // An unreadable weight counts as a refusal.
        let ok = q.is_some_and(|q| q > 0.0);
        if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
            gzip = Some(gzip.unwrap_or(true) && ok);
        } else if name == "*" {
            any = Some(any.unwrap_or(true) && ok);
        }
    }
    gzip.or(any).unwrap_or(false)
}

fn file_response(f: &File, headers: &HeaderMap) -> Response {
    let gzip = f.gzip.as_ref().filter(|_| accepts_gzip(headers));
    let (body, tag) = match gzip {
        Some((body, tag)) => (body, tag),
        None => (&f.body, &f.etag),
    };
    let etag = HeaderValue::from_str(tag).unwrap_or(HeaderValue::from_static("\"\""));
    let cache = HeaderValue::from_static("private, no-cache");
    let vary = HeaderValue::from_static("accept-encoding");
    let fresh = headers
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == tag));
    if fresh {
        return (
            StatusCode::NOT_MODIFIED,
            [(ETAG, etag), (CACHE_CONTROL, cache), (VARY, vary)],
        )
            .into_response();
    }
    let mut res = (
        [
            (CONTENT_TYPE, HeaderValue::from_static(f.content_type)),
            (ETAG, etag),
            (CACHE_CONTROL, cache),
            (VARY, vary),
        ],
        body.clone(),
    )
        .into_response();
    if gzip.is_some() {
        res.headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    res
}

/// `GET /auth/session`: the session's CSRF token and owner scope, or 401 `signed_out` without a
/// session (never a redirect: only the app's `fetch` asks). Same-origin requests only, and the session is not
/// touched (see the module docs).
pub async fn session_token(State(app): State<AppState>, headers: HeaderMap) -> Response {
    use crate::api::error::ApiError;
    use crate::auth::session::{blocking, check_origin, cookie_value, csrf_failed};
    let cross = headers
        .get("sec-fetch-site")
        .is_some_and(|v| v.as_bytes() != b"same-origin");
    if cross {
        return csrf_failed().into_response();
    }
    if headers.contains_key(axum::http::header::ORIGIN)
        && let Err(e) = check_origin(&headers, &app.authsvc)
    {
        return e.into_response();
    }
    let signed_out =
        || ApiError::new(StatusCode::UNAUTHORIZED, "signed_out", "sign in first").into_response();
    let Some(cookie) = cookie_value(&headers) else {
        return signed_out();
    };
    match blocking(&app.authsvc, move |svc| {
        svc.peek_session(&cookie)?
            .map(|row| svc.browser_owner_scope().map(|scope| (row, scope)))
            .transpose()
    })
    .await
    {
        Ok(Ok(Some((row, scope)))) => {
            axum::Json(serde_json::json!({ "csrf": row.csrf, "owner_scope": scope }))
                .into_response()
        }
        Ok(Ok(None)) => signed_out(),
        Ok(Err(e)) | Err(e) => e.into_response(),
    }
}

/// `GET /app`.
pub async fn root(State(app): State<AppState>, session: OwnerSession) -> Response {
    match &app.web {
        Some(web) => page_response(&app, web, &session).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /app/{*path}`: a file of the build, or the page for any route of the app (a path whose last
/// segment has no extension).
pub async fn path(
    State(app): State<AppState>,
    session: OwnerSession,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(web) = &app.web else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Some(f) = web.files.get(path.as_str()) {
        let mut response = file_response(f, &headers);
        if path == WORKER {
            let headers = response.headers_mut();
            headers.insert("service-worker-allowed", HeaderValue::from_static("/app"));
            headers.insert(
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(WORKER_CSP),
            );
        }
        return response;
    }
    let last = path.rsplit('/').next().unwrap_or_default();
    if last.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    page_response(&app, web, &session).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_needs_the_marker_once() {
        assert!(WebApp::from_parts("<head></head>", []).is_err());
        assert!(WebApp::from_parts(&format!("{SESSION_MARKER}{SESSION_MARKER}"), []).is_err());
        let app = WebApp::from_parts(&format!("<head>{SESSION_MARKER}</head>"), []).unwrap();
        assert_eq!(
            app.page("a\"b", "scope\"value"),
            "<head><meta name=\"csrf-token\" content=\"a&quot;b\"><meta name=\"todo-owner-scope\" content=\"scope&quot;value\"></head>"
        );
    }

    #[test]
    fn unknown_file_types_are_not_served() {
        let app = WebApp::from_parts(
            SESSION_MARKER,
            [
                ("app.wasm".to_owned(), vec![0]),
                ("notes.md".to_owned(), vec![0]),
            ],
        )
        .unwrap();
        assert!(app.files.contains_key("app.wasm"));
        assert!(!app.files.contains_key("notes.md"));
        assert_eq!(app.files["app.wasm"].content_type, "application/wasm");
    }

    #[test]
    fn a_gzipped_copy_is_attached_to_its_original() {
        let app = WebApp::from_parts(
            SESSION_MARKER,
            [
                ("app.wasm".to_owned(), vec![1]),
                ("app.wasm.gz".to_owned(), vec![2]),
                ("orphan.js.gz".to_owned(), vec![3]),
            ],
        )
        .unwrap();
        let f = &app.files["app.wasm"];
        assert_eq!(f.gzip.as_ref().unwrap().0, vec![2]);
        assert_ne!(f.gzip.as_ref().unwrap().1, f.etag);
        assert!(!app.files.contains_key("app.wasm.gz"));
        assert!(!app.files.contains_key("orphan.js"));
    }

    #[test]
    fn gzip_is_sent_only_to_clients_that_accept_it() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert(ACCEPT_ENCODING, HeaderValue::from_str(v).unwrap());
            m
        };
        assert!(accepts_gzip(&h("gzip, deflate, br")));
        assert!(accepts_gzip(&h("br;q=1.0, gzip;q=0.8")));
        assert!(accepts_gzip(&h("*")));
        assert!(!accepts_gzip(&h("gzip;q=0")));
        // An explicit refusal wins over the wildcard, in either order.
        assert!(!accepts_gzip(&h("gzip;q=0, *;q=1")));
        assert!(!accepts_gzip(&h("*, gzip;q=0")));
        assert!(!accepts_gzip(&h("*;q=0")));
        assert!(accepts_gzip(&h("*;q=0, gzip")));
        assert!(accepts_gzip(&h("GZIP;q=0.5")));
        assert!(!accepts_gzip(&h("gzip;q=nope")));
        assert!(!accepts_gzip(&h("br")));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }
}
