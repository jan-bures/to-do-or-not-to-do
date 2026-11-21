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
//! `index.html` gets [`APP_CSP`]: everything from this origin only, and `'wasm-unsafe-eval'` so the
//! module can be compiled. No inline script or style is allowed. It is never cached. The other files
//! are revalidated with an `ETag` (`private, no-cache`).

use crate::app::AppState;
use crate::auth::session::OwnerSession;
use axum::extract::{Path, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path as FsPath;

/// The content security policy of the web app's page.
pub const APP_CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; font-src 'self'; \
frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";

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
        let mut files = HashMap::new();
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
                let Some(ty) = content_type(&name) else {
                    continue;
                };
                let body = std::fs::read(entry.path()).map_err(|source| WebAppError::Io {
                    path: entry.path().display().to_string(),
                    source,
                })?;
                total += body.len() as u64;
                if total > MAX_TOTAL_BYTES || files.len() >= MAX_FILES {
                    return Err(WebAppError::Invalid(format!(
                        "{} is larger than {MAX_TOTAL_BYTES} bytes or {MAX_FILES} files",
                        dir.display()
                    )));
                }
                let etag = format!("\"{}\"", &hex::encode(Sha256::digest(&body))[..32]);
                files.insert(
                    rel,
                    File {
                        body,
                        content_type: ty,
                        etag,
                    },
                );
            }
        }
        let index = files
            .remove("index.html")
            .ok_or_else(|| WebAppError::Invalid(format!("{} has no index.html", dir.display())))?;
        let index = String::from_utf8(index.body)
            .map_err(|_| WebAppError::Invalid("index.html is not UTF-8".into()))?;
        let index = Self::split_index(&index)?;
        Ok(Self { index, files })
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
        let files = files
            .into_iter()
            .filter_map(|(name, body)| {
                let content_type = content_type(&name)?;
                let etag = format!("\"{}\"", &hex::encode(Sha256::digest(&body))[..32]);
                Some((
                    name,
                    File {
                        body,
                        content_type,
                        etag,
                    },
                ))
            })
            .collect();
        Ok(Self { index, files })
    }

    /// The page for a session: `index.html` with the CSRF token.
    #[must_use]
    pub fn page(&self, csrf: &str) -> String {
        format!(
            "{}<meta name=\"csrf-token\" content=\"{}\">{}",
            self.index.0,
            crate::web::html::esc(csrf),
            self.index.1
        )
    }
}

fn page_response(app: &WebApp, session: &OwnerSession) -> Response {
    (
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (CONTENT_SECURITY_POLICY, HeaderValue::from_static(APP_CSP)),
            (CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        app.page(&session.row.csrf),
    )
        .into_response()
}

/// `GET /app`.
pub async fn root(State(app): State<AppState>, session: OwnerSession) -> Response {
    match &app.web {
        Some(web) => page_response(web, &session),
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
        let etag = HeaderValue::from_str(&f.etag).unwrap_or(HeaderValue::from_static("\"\""));
        let fresh = headers
            .get(IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|t| t.trim() == f.etag));
        let cache = HeaderValue::from_static("private, no-cache");
        if fresh {
            return (
                StatusCode::NOT_MODIFIED,
                [(ETAG, etag), (CACHE_CONTROL, cache)],
            )
                .into_response();
        }
        return (
            [
                (CONTENT_TYPE, HeaderValue::from_static(f.content_type)),
                (ETAG, etag),
                (CACHE_CONTROL, cache),
            ],
            f.body.clone(),
        )
            .into_response();
    }
    let last = path.rsplit('/').next().unwrap_or_default();
    if last.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    page_response(web, &session)
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
            app.page("a\"b"),
            "<head><meta name=\"csrf-token\" content=\"a&quot;b\"></head>"
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
}
