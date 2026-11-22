//! Private, flat Velopack feed. The operator controls the directory; the server never writes to it.

use crate::app::AppState;
use crate::auth::{Authenticated, Principal, require_auth};
use axum::Router;
use axum::extract::{Path as RoutePath, Request, State};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_TYPE, HeaderValue};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use std::path::{Path, PathBuf};
use tower::ServiceExt as _;
use tower_http::services::ServeFile;

pub(crate) async fn directory(path: &Path) -> std::io::Result<PathBuf> {
    let path = tokio::fs::canonicalize(path).await?;
    if !tokio::fs::metadata(&path).await?.is_dir() {
        return Err(std::io::Error::other("release path is not a directory"));
    }
    Ok(path)
}

pub(crate) fn router(state: &AppState) -> Router<AppState> {
    if state.releases.is_none() {
        return Router::new();
    }
    Router::new()
        .route("/releases/{file}", get(serve))
        .route_layer(from_fn_with_state(state.clone(), require_auth))
}

fn content_type(name: &str) -> Option<&'static str> {
    match name {
        "releases.win.json" => Some("application/json"),
        "todo-desktop-win-Setup.exe" => Some("application/vnd.microsoft.portable-executable"),
        _ => {
            let name = name.strip_prefix("todo-desktop-")?;
            let version = name
                .strip_suffix("-full.nupkg")
                .or_else(|| name.strip_suffix("-delta.nupkg"))?;
            // No separators, colon (including Windows alternate data streams), or other path
            // syntax, even after axum decodes a percent-encoded segment.
            let (base, height) = version.split_once("-build.")?;
            let decimal = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            (base.split('.').count() == 3 && base.split('.').all(decimal) && decimal(height))
                .then_some("application/octet-stream")
        }
    }
}

async fn serve(
    State(state): State<AppState>,
    caller: Authenticated,
    RoutePath(file): RoutePath<String>,
    request: Request,
) -> Response {
    // Browser sessions and agent/administration tokens cannot fetch packages. The normal auth
    // layer also checks revocation and applies the existing per-IP and per-token budgets.
    if !matches!(caller.principal, Principal::Owner { .. }) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (Some(dir), Some(content_type)) = (&state.releases, content_type(&file)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let file = dir.join(file);
    // Reject directories and links/reparse points even with an allowed name. Only the trusted
    // publisher can change entries here, and it publishes regular files by atomic rename.
    if !tokio::fs::symlink_metadata(&file)
        .await
        .is_ok_and(|m| m.is_file() && todo_fs::is_plain(&m))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(res) = ServeFile::new(file).oneshot(request).await;
    let mut res = res.into_response();
    if res.status().is_success() {
        res.headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    }
    res
}
