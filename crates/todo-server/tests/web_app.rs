//! The web app: `/app` behind the owner's session, and the REST API accepting that session with the
//! CSRF header and the origin check (see `todo_server::auth::require_auth_or_session`).
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::web::{Body, Browser, Env, SITE};
use common::{TOKEN_A, id};
use reqwest::header::{
    ACCEPT, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ETAG, IF_NONE_MATCH,
    LOCATION, ORIGIN, WWW_AUTHENTICATE,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;
use tempfile::TempDir;
use todo_server::web::spa::{APP_CSP, SESSION_MARKER};

const INDEX: &str =
    "<!doctype html><html><head><!--todo-session--><title>To-do</title></head><body></body></html>";

/// A fake build: `index.html`, a module and a font.
fn build() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    assert!(INDEX.contains(SESSION_MARKER));
    std::fs::write(dir.path().join("index.html"), INDEX).unwrap();
    std::fs::write(dir.path().join("todo_web_bg.wasm"), b"\0asm\x01\0\0\0").unwrap();
    std::fs::write(dir.path().join("boot.js"), "import('./todo_web.js');").unwrap();
    std::fs::create_dir(dir.path().join("fonts")).unwrap();
    std::fs::write(dir.path().join("fonts").join("Mono.ttf"), b"font").unwrap();
    std::fs::write(dir.path().join("README.md"), b"not served").unwrap();
    dir
}

async fn env_with_app() -> (Env, TempDir) {
    let dist = build();
    let path = dist.path().to_owned();
    let env = Env::start_with(move |cfg| cfg.web.dir = Some(path)).await;
    (env, dist)
}

/// An API request as the web app makes it: the session cookie, and (when given) the CSRF header and
/// `Origin`.
async fn api(
    b: &Browser,
    method: Method,
    path: &str,
    csrf: Option<&str>,
    origin: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, reqwest::header::HeaderMap, Value) {
    let client = reqwest::Client::new();
    let mut req = client.request(method, format!("{}{path}", b.url));
    if let Some(c) = &b.cookie {
        req = req.header(COOKIE, format!("__Host-todo_session={c}"));
    }
    if let Some(c) = csrf {
        req = req.header("x-csrf-token", c);
    }
    if let Some(o) = origin {
        req = req.header(ORIGIN, o);
    }
    if let Some(v) = body {
        req = req.json(&v);
    }
    let res = req.send().await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let text = res.text().await.unwrap();
    (
        status,
        headers,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

fn create_list(name: &str) -> Value {
    json!({ "command_id": id(), "command": { "cmd": "create_list", "id": id(), "name": name } })
}

async fn event_count(env: &Env) -> usize {
    let (s, v) = env.t.get("/api/v1/events?since=0").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["events"].as_array().unwrap().len()
}

#[tokio::test]
async fn the_session_with_its_csrf_token_reads_and_writes_as_the_web_device() {
    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();

    let (s, _, v) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    let (s, _, v) = api(
        &b,
        Method::POST,
        "/api/v1/commands",
        Some(&csrf),
        Some(SITE),
        Some(create_list("Errands")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Recorded as the owner on the `web` device.
    let (_, v) = env.t.get("/api/v1/events?since=0").await;
    let last = v["events"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["actor"], json!({ "type": "owner", "device": "web" }));

    // The JSON log and the event stream work with the session too.
    let (s, _, v) = api(
        &b,
        Method::GET,
        "/api/v1/events?since=0",
        Some(&csrf),
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let res = reqwest::Client::new()
        .get(format!("{}/api/v1/events?since=0", b.url))
        .header(
            COOKIE,
            format!("__Host-todo_session={}", b.cookie.as_ref().unwrap()),
        )
        .header("x-csrf-token", &csrf)
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut stream = common::SseReader::new(res);
    // The reader checks that the stream opens with `hello`; then the list's event arrives.
    let event = stream
        .next_event_or_named(Duration::from_secs(5))
        .await
        .unwrap();
    assert!(stream.hello.is_some());
    assert!(event.data.contains("Errands"), "{}", event.data);
}

#[tokio::test]
async fn a_missing_or_wrong_csrf_token_is_refused_and_writes_nothing() {
    let env = Env::start().await;
    let b = env.owner().await;
    let before = event_count(&env).await;
    for csrf in [None, Some("wrong"), Some("")] {
        let (s, _, v) = api(&b, Method::GET, "/api/v1/lists", csrf, None, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{csrf:?}: {v}");
        assert_eq!(v["error"]["code"], "csrf");
        let (s, _, v) = api(
            &b,
            Method::POST,
            "/api/v1/commands",
            csrf,
            Some(SITE),
            Some(create_list("Nope")),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{csrf:?}: {v}");
        assert_eq!(v["error"]["code"], "csrf");
    }
    // Another session's token doesn't fit this one either.
    let mut other = b;
    let old = other.csrf.clone();
    other.login().await;
    assert_ne!(other.csrf, old);
    let (s, _, _) = api(
        &other,
        Method::GET,
        "/api/v1/lists",
        old.as_deref(),
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(event_count(&env).await, before);
}

#[tokio::test]
async fn a_cross_origin_or_originless_write_is_refused() {
    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    let before = event_count(&env).await;
    for origin in [
        Some("https://evil.example.com"),
        Some("http://todo.example.com"),
        Some("https://todo.example.com:8443"),
        Some("null"),
        None,
    ] {
        let (s, _, v) = api(
            &b,
            Method::POST,
            "/api/v1/commands",
            Some(&csrf),
            origin,
            Some(create_list("Nope")),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{origin:?}: {v}");
        assert_eq!(v["error"]["code"], "csrf", "{origin:?}");
    }
    // Undo is a write too.
    let (s, _, _) = api(
        &b,
        Method::POST,
        "/api/v1/events/1/undo",
        Some(&csrf),
        Some("https://evil.example.com"),
        Some(json!({ "command_id": id(), "expected_event_id": id() })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(event_count(&env).await, before);
}

#[tokio::test]
async fn an_unknown_or_ended_session_is_a_401() {
    let env = Env::start().await;
    let mut b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();

    let real = b.cookie.clone();
    b.cookie = Some("B".repeat(43));
    let (s, h, v) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_eq!(v["error"]["code"], "unauthorized");
    assert_eq!(h[WWW_AUTHENTICATE], "Bearer");

    // Signing out ends the session for the API too.
    b.cookie = real;
    let p = b.post_form("/auth/logout", &[]).await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let (s, _, _) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // And the idle timeout.
    b.login().await;
    let csrf = b.csrf.clone().unwrap();
    env.advance(Duration::from_secs(61 * 60));
    let (s, _, _) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bearer_tokens_are_unchanged_and_take_precedence_over_a_cookie() {
    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    let client = reqwest::Client::new();

    // A device token works with no cookie, CSRF header or Origin, exactly as before.
    let (s, _) = env.t.get("/api/v1/lists").await;
    assert_eq!(s, StatusCode::OK);

    // With an Authorization header the cookie is ignored: a wrong token is a 401 even with a live
    // session and its CSRF token.
    let res = client
        .get(format!("{}/api/v1/lists", b.url))
        .bearer_auth("tda_nope")
        .header(
            COOKIE,
            format!("__Host-todo_session={}", b.cookie.as_ref().unwrap()),
        )
        .header("x-csrf-token", &csrf)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // A right token with a cookie acts as the token's device.
    let res = client
        .post(format!("{}/api/v1/commands", b.url))
        .bearer_auth(TOKEN_A)
        .header(
            COOKIE,
            format!("__Host-todo_session={}", b.cookie.as_ref().unwrap()),
        )
        .json(&create_list("Via token"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let (_, v) = env.t.get("/api/v1/events?since=0").await;
    let last = v["events"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["actor"], json!({ "type": "owner", "device": "alpha" }));
}

#[tokio::test]
async fn the_session_opens_neither_mcp_nor_the_upload() {
    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    let (s, _, _) = api(
        &b,
        Method::POST,
        "/mcp",
        Some(&csrf),
        Some(SITE),
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = api(
        &b,
        Method::POST,
        &format!("/api/v1/tasks/{}/attachments", id()),
        Some(&csrf),
        Some(SITE),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_app_needs_a_session_and_gets_its_csrf_token_and_a_strict_csp() {
    let (env, _dist) = env_with_app().await;

    // Signed out: every app route goes to sign-in, files included.
    let anon = env.browser();
    for path in ["/app", "/app/list/x", "/app/todo_web_bg.wasm"] {
        let p = anon.get(path).await;
        assert_eq!(p.status, StatusCode::SEE_OTHER, "{path}");
        assert!(
            p.location().starts_with("/login?next=/app"),
            "{path}: {}",
            p.location()
        );
    }

    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    for path in ["/app", "/app/list/abc", "/app/task/abc/steps"] {
        let p = b.get(path).await;
        assert_eq!(p.status, StatusCode::OK, "{path}");
        assert_eq!(p.header(CONTENT_SECURITY_POLICY), APP_CSP, "{path}");
        assert_eq!(p.header(CACHE_CONTROL), "no-store");
        assert!(
            p.body
                .contains(&format!("<meta name=\"csrf-token\" content=\"{csrf}\">")),
            "{}",
            p.body
        );
        assert!(!p.body.contains(SESSION_MARKER));
    }
    // The CSP allows wasm compilation but no inline script or style, and only this origin.
    assert!(APP_CSP.contains("script-src 'self' 'wasm-unsafe-eval'"));
    assert!(!APP_CSP.contains("unsafe-inline"));
    assert!(APP_CSP.contains("frame-ancestors 'none'"));

    // `/` opens the app.
    let p = b.get("/").await;
    assert_eq!(p.status, StatusCode::SEE_OTHER);
    assert_eq!(p.location(), "/app");

    // Files, with their types and an ETag.
    let p = b.get("/app/todo_web_bg.wasm").await;
    assert_eq!(p.status, StatusCode::OK);
    assert_eq!(p.header(CONTENT_TYPE), "application/wasm");
    assert_eq!(p.header(CACHE_CONTROL), "private, no-cache");
    let etag = p.header(ETAG).to_owned();
    let p = b.get("/app/fonts/Mono.ttf").await;
    assert_eq!(p.status, StatusCode::OK);
    assert_eq!(p.header(CONTENT_TYPE), "font/ttf");
    let p = b.get("/app/boot.js").await;
    assert!(p.header(CONTENT_TYPE).starts_with("text/javascript"));

    // Revalidation.
    let res = reqwest::Client::new()
        .get(format!("{}/app/todo_web_bg.wasm", b.url))
        .header(
            COOKIE,
            format!("__Host-todo_session={}", b.cookie.as_ref().unwrap()),
        )
        .header(IF_NONE_MATCH, &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    // Nothing else: unknown files, other types, and no way out of the directory.
    for path in [
        "/app/nope.js",
        "/app/README.md",
        "/app/../Cargo.toml",
        "/app/%2e%2e/%2e%2e/Cargo.toml",
        "/app/fonts/../index.html",
        "/app/index.html",
    ] {
        let p = b.get(path).await;
        assert!(
            matches!(p.status.as_u16(), 404 | 400),
            "{path}: {} {}",
            p.status,
            p.body
        );
    }

    // HEAD works like GET for the page; other methods are not routed.
    let p = b
        .send(Method::POST, "/app", Some(SITE), Body::Json(json!({})))
        .await;
    assert_eq!(p.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn without_a_build_the_app_is_absent_and_root_opens_the_admin_pages() {
    let env = Env::start().await;
    let b = env.owner().await;
    assert_eq!(b.get("/app").await.status, StatusCode::NOT_FOUND);
    assert_eq!(b.get("/app/list/x").await.status, StatusCode::NOT_FOUND);
    let p = b.get("/").await;
    assert_eq!(p.status, StatusCode::SEE_OTHER);
    assert_eq!(p.location(), "/admin");
    // The redirect to sign-in keeps the app as the target.
    let p = env.browser().get("/app").await;
    assert_eq!(p.headers[LOCATION], "/login?next=/app");
}

#[tokio::test]
async fn a_build_without_the_session_marker_is_refused_at_start() {
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(dist.path().join("index.html"), "<html></html>").unwrap();
    let mut cfg = todo_server::Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        ..todo_server::Config::default()
    };
    let data = tempfile::tempdir().unwrap();
    cfg.data_dir = data.path().to_owned();
    cfg.web.dir = Some(dist.path().to_owned());
    let err = todo_server::Server::bind(&cfg, todo_server::Options::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("session"), "{err}");
}

#[tokio::test]
async fn signing_out_closes_the_apps_event_stream() {
    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    let res = reqwest::Client::new()
        .get(format!("{}/api/v1/events?since=0", b.url))
        .header(
            COOKIE,
            format!("__Host-todo_session={}", b.cookie.as_ref().unwrap()),
        )
        .header("x-csrf-token", &csrf)
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut stream = common::SseReader::new(res);
    // Open: a live event arrives.
    env.t
        .ok(json!({ "cmd": "create_list", "id": id(), "name": "Live" }))
        .await;
    let f = stream
        .next_event_or_named(Duration::from_secs(5))
        .await
        .unwrap();
    assert!(f.data.contains("Live"), "{}", f.data);
    // Sign out: the stream ends.
    let p = b.post_form("/auth/logout", &[]).await;
    assert_eq!(p.status, StatusCode::SEE_OTHER);
    assert!(stream.ended(Duration::from_secs(5)).await);
}
