//! The Windows release feed is opt-in, flat, owner-only, and streams static files.
#![allow(clippy::unwrap_used)]

mod common;

use common::web::Env;
use common::{TOKEN_A, TestServer, raw};
use reqwest::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use reqwest::{Method, StatusCode};
use serde_json::json;
use std::fs;

const INDEX: &str = "/releases/releases.win.json";
const FULL: &str = "todo-desktop-0.1.0-build.146-full.nupkg";
const SETUP: &str = "todo-desktop-win-Setup.exe";

#[tokio::test]
async fn feed_is_off_by_default() {
    let t = TestServer::start().await;
    assert_eq!(t.get(INDEX).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_owner_device_bearers_get_releases_and_revocation_is_immediate() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("releases.win.json"), r#"{"Assets":[]}"#).unwrap();
    let env = Env::start_with(|cfg| {
        cfg.releases.dir = Some(dir.path().into());
        cfg.auth.allow_config_owner_tokens = false;
    })
    .await;
    for (method, token) in [(Method::GET, None), (Method::HEAD, Some("invalid"))] {
        let (status, headers, _) = env.t.request(method, INDEX, token, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(headers[CACHE_CONTROL], "no-store, no-transform");
    }
    let mut owner = env.owner().await;
    assert_eq!(owner.get(INDEX).await.status, StatusCode::UNAUTHORIZED);
    let (_, agent) = owner.create_agent("test-agent", "read", "").await;
    assert_eq!(
        env.api(Method::GET, INDEX, &agent, None).await.status,
        StatusCode::FORBIDDEN
    );

    let (status, _, start) = env
        .t
        .request(
            Method::POST,
            "/api/v1/pair/start",
            None,
            Some(json!({"device_name": "Test Desktop"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let code = start["user_code"].as_str().unwrap();
    let approval = owner.post_form("/pair/approve", &[("code", code)]).await;
    assert_eq!(approval.status, StatusCode::OK);
    let (_, _, paired) = env
        .t
        .request(
            Method::POST,
            "/api/v1/pair/poll",
            None,
            Some(json!({"poll_token": start["poll_token"]})),
        )
        .await;
    let token = paired["token"].as_str().unwrap();
    let res = env.api(Method::GET, INDEX, token, None).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.headers[CONTENT_TYPE], "application/json");
    assert_eq!(res.headers[CACHE_CONTROL], "no-store, no-transform");
    assert_eq!(res.json, json!({"Assets": []}));

    owner.step_up().await;
    let device = paired["device_id"].as_str().unwrap();
    let revoked = owner
        .post_form(&format!("/admin/devices/{device}/revoke"), &[])
        .await;
    assert_eq!(revoked.status, StatusCode::SEE_OTHER);
    assert_eq!(
        env.api(Method::GET, INDEX, token, None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn packages_support_head_and_single_ranges() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FULL), b"abcdef").unwrap();
    fs::write(dir.path().join(SETUP), b"mock setup").unwrap();
    let t = TestServer::start_with(|cfg| cfg.releases.dir = Some(dir.path().into())).await;
    let path = format!("/releases/{FULL}");
    let (status, headers, body) = t.request(Method::HEAD, &path, Some(TOKEN_A), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[CONTENT_TYPE], "application/octet-stream");
    assert_eq!(headers[CONTENT_LENGTH], "6");
    assert!(body.is_null());

    let partial = t
        .client
        .get(format!("{}{path}", t.url))
        .bearer_auth(TOKEN_A)
        .header(RANGE, "bytes=1-3")
        .send()
        .await
        .unwrap();
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(partial.headers()[CONTENT_RANGE], "bytes 1-3/6");
    assert_eq!(partial.text().await.unwrap(), "bcd");

    let (status, headers, body) = t
        .request(
            Method::GET,
            &format!("/releases/{SETUP}"),
            Some(TOKEN_A),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers[CONTENT_TYPE],
        "application/vnd.microsoft.portable-executable"
    );
    assert_eq!(body, "mock setup");
}

#[tokio::test]
async fn paths_are_flat_and_only_release_files_are_exposed() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("feed");
    fs::create_dir(&dir).unwrap();
    fs::create_dir(dir.join(FULL)).unwrap();
    fs::write(root.path().join("unrelated.txt"), "outside").unwrap();
    fs::write(dir.join("internal.txt"), "inside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path().join("unrelated.txt"), dir.join(SETUP)).unwrap();
    let t = TestServer::start_with(|cfg| cfg.releases.dir = Some(dir)).await;
    // The raw client keeps each request target unchanged, rather than normalizing dot segments.
    for path in [
        "/releases",
        "/releases/",
        "/releases/internal.txt",
        "/releases/../unrelated.txt",
        "/releases/%2e%2e%2funrelated.txt",
        "/releases/%2e%2e%5cunrelated.txt",
        "/releases/nested%2freleases.win.json",
        "/releases/releases.win.json%3a%3a%24DATA",
        &format!("/releases/{FULL}"),
        &format!("/releases/{SETUP}"),
    ] {
        let res = raw::send(
            t.url.strip_prefix("http://").unwrap().parse().unwrap(),
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN_A}\r\nConnection: close\r\n\r\n"),
            b"",
        )
        .await;
        assert_eq!(res.status, 404, "{path}");
    }
}

#[test]
fn releases_config_accepts_file_and_environment_values() {
    let mut cfg = todo_server::Config::from_toml("[releases]\ndir = 'desktop-releases'").unwrap();
    assert_eq!(
        cfg.releases.dir.as_deref(),
        Some(std::path::Path::new("desktop-releases"))
    );
    cfg.apply_env(|key| (key == "TODO_SERVER_RELEASES_DIR").then(String::new))
        .unwrap();
    assert!(cfg.releases.dir.is_none());
}
