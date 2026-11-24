//! The web app: `/app` behind the owner's session, and the REST API accepting that session with the
//! CSRF header and the origin check (see `todo_server::auth::require_auth_or_session`).
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::web::{Body, Browser, Env, SITE};
use common::{TOKEN_A, id};
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, COOKIE, ETAG, IF_NONE_MATCH, LOCATION, ORIGIN, VARY, WWW_AUTHENTICATE,
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
    // The build's gzipped copy (the server sends it as is; the bytes needn't be real gzip here).
    std::fs::write(dir.path().join("todo_web_bg.wasm.gz"), b"gzipped").unwrap();
    std::fs::write(dir.path().join("boot.js"), "import('./todo_web.js');").unwrap();
    std::fs::write(dir.path().join("reminder-worker.js"), "// Worker fixture").unwrap();
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

#[tokio::test]
async fn web_push_requires_the_session_and_does_not_touch_it_from_the_worker() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::sync::atomic::Ordering;

    let (env, _) = env_with_app().await;
    let b = env.owner().await;
    let config = "/api/v1/web-push";
    assert_eq!(
        api(&b, Method::GET, config, b.csrf.as_deref(), None, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        api(&b, Method::GET, config, None, None, None).await.0,
        StatusCode::FORBIDDEN
    );
    drop(env);

    let dist = build();
    let key = dist.path().join("test.secret");
    todo_server::write_vapid_key(&key).unwrap();
    let env = Env::start_with(|cfg| {
        cfg.web_push.vapid_key_file = Some(key);
        cfg.web.dir = Some(dist.path().to_owned());
    })
    .await;
    let b = env.owner().await;
    let csrf = b.csrf.as_deref();
    let (s, _, key) = api(&b, Method::GET, config, csrf, None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(key["public_key"].as_str().unwrap().len(), 87);
    assert_eq!(
        env.api(Method::GET, config, TOKEN_A, None).await.status,
        StatusCode::NOT_FOUND
    );
    let sub = json!({
        "endpoint": "https://fcm.googleapis.com/fcm/send/fixture",
        "keys": { "p256dh": key["public_key"], "auth": URL_SAFE_NO_PAD.encode([0_u8; 16]) }
    });
    assert_eq!(
        api(&b, Method::PUT, config, csrf, None, Some(sub.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        api(&b, Method::PUT, config, csrf, Some(SITE), Some(sub.clone()))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let endpoint = json!({"endpoint": sub["endpoint"]});
    let status = format!("{config}/status");
    assert_eq!(
        api(
            &b,
            Method::POST,
            &status,
            csrf,
            Some(SITE),
            Some(endpoint.clone())
        )
        .await
        .2,
        json!({"active":true})
    );
    assert_eq!(
        api(
            &b,
            Method::DELETE,
            config,
            csrf,
            Some(SITE),
            Some(endpoint.clone())
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        api(&b, Method::POST, &status, csrf, Some(SITE), Some(endpoint))
            .await
            .2,
        json!({"active":false})
    );

    let list = env.t.create_list("Errands").await;
    let task = id();
    assert_eq!(env.command(TOKEN_A, json!({"cmd":"create_task", "id":task, "list":list, "title":"Private title", "notes":"Private notes"})).await.status, StatusCode::OK);
    let task_path = format!("{config}/tasks/{task}");
    env.offset.store(50 * 60 * 1000, Ordering::SeqCst);
    let (s, headers, body) = api(&b, Method::GET, &task_path, csrf, None, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(headers[CACHE_CONTROL], "no-store, no-transform");
    assert_eq!(body, json!({ "title": "Private title" }));
    let (s, headers, _) = api(&b, Method::GET, "/app/reminder-worker.js", None, None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(headers["service-worker-allowed"], "/app");
    assert!(
        headers[CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("connect-src 'self'")
    );
    env.offset.store(61 * 60 * 1000, Ordering::SeqCst);
    assert_eq!(
        api(&b, Method::GET, &task_path, csrf, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
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
async fn activity_filters_before_paging_without_expanding_visibility() {
    use todo_core::{AccessLevel, Grants};

    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.as_deref();
    let visible = env.t.create_list("Visible").await;
    let hidden = env.t.create_list("Hidden").await;
    let (_, agent) = common::mcp::agent(
        &env.t,
        "writer",
        &Grants::with_default(AccessLevel::Plan),
        None,
    );
    for (list, title) in [(&visible, "Older visible task"), (&hidden, "Private task")] {
        let (s, _, v) = env
            .t
            .command_with_id(
                &agent,
                &id(),
                json!({
                    "cmd": "create_task", "id": id(), "title": title, "list": list,
                }),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    // A newer non-agent event does not consume the one-item page.
    env.t.create_list("Newer owner list").await;
    let path = "/api/v1/activity?agent=writer&limit=1";
    let (s, _, page) = api(&b, Method::GET, path, csrf, None, None).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(page["items"][0]["summary"], "Created task \"Private task\"");
    let before = page["next_before"].as_u64().unwrap();
    let (s, _, page) = api(
        &b,
        Method::GET,
        &format!("{path}&before={before}"),
        csrf,
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(
        page["items"][0]["summary"],
        "Created task \"Older visible task\""
    );
    assert_eq!(page["items"][0]["event"]["actor"]["agent_id"], "writer");

    // Exactly one page of matches still gives a cursor. Older owner events don't fill the
    // next page, which terminates the scan.
    let path_exact = "/api/v1/activity?agent=writer&limit=2";
    let (s, _, full) = api(&b, Method::GET, path_exact, csrf, None, None).await;
    assert_eq!(s, StatusCode::OK, "{full}");
    assert_eq!(full["items"].as_array().unwrap().len(), 2);
    assert_eq!(full["next_before"], full["items"][1]["event"]["seq"]);
    let before = full["next_before"].as_u64().unwrap();
    let (s, _, end) = api(
        &b,
        Method::GET,
        &format!("{path_exact}&before={before}"),
        csrf,
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{end}");
    assert_eq!(end["items"], json!([]));
    assert_eq!(end["next_before"], Value::Null);

    let (s, _, _) = api(&b, Method::GET, path, None, None, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let mut grants = Grants::with_default(AccessLevel::None);
    grants
        .lists
        .insert(visible.parse().unwrap(), AccessLevel::Read);
    let (_, reader) = common::mcp::agent(&env.t, "reader", &grants, None);
    // Filtered cursors for a scoped reader must reveal no more than unfiltered cursors.
    let mut cursor = String::new();
    for visible_item in [false, false, true] {
        let (s, _, baseline) = env
            .t
            .request(
                Method::GET,
                &format!("/api/v1/activity?limit=1{cursor}"),
                Some(&reader),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{baseline}");
        let (s, _, page) = env
            .t
            .request(Method::GET, &format!("{path}{cursor}"), Some(&reader), None)
            .await;
        assert_eq!(s, StatusCode::OK, "{page}");
        assert_eq!(page["next_before"], baseline["next_before"]);
        if visible_item {
            assert_eq!(
                page["items"][0]["summary"],
                "Created task \"Older visible task\""
            );
            assert!(page["items"][0]["event"].get("command_len").is_none());
        } else {
            assert_eq!(page["items"], json!([]));
        }
        cursor = format!("&before={}", page["next_before"].as_u64().unwrap());
    }
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
async fn the_session_opens_neither_mcp_nor_imported_content_fill() {
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
        Method::PUT,
        &format!("/api/v1/attachments/{}/content", id()),
        Some(&csrf),
        Some(SITE),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

async fn upload_url(env: &Env) -> String {
    let list = env.t.create_list("Inbox").await;
    let task = env.t.create_task(&list, "With files", json!({})).await;
    format!(
        "{}/api/v1/tasks/{task}/attachments?file_name=letter.txt",
        env.t.url
    )
}

fn upload_request(b: &Browser, url: &str) -> reqwest::RequestBuilder {
    b.client
        .post(url)
        .header(
            COOKIE,
            format!("__Host-todo_session={}", b.cookie.as_ref().unwrap()),
        )
        .header(CONTENT_TYPE, "text/plain")
        .body("A letter")
}

#[tokio::test]
async fn a_session_upload_records_the_attachment_as_the_web_device() {
    let env = Env::start().await;
    let b = env.owner().await;
    let url = upload_url(&env).await;
    let res = upload_request(&b, &url)
        .header(ORIGIN, SITE)
        .header("x-csrf-token", b.csrf.as_ref().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["attachment"]["file_name"], "letter.txt");
    assert_eq!(v["attachment"]["size_bytes"], 8);
    assert_eq!(
        v["attachment"]["created"]["by"],
        json!({"type": "owner", "device": "web"})
    );
}

#[tokio::test]
async fn uploads_require_csrf_and_same_origin_for_a_session() {
    let env = Env::start().await;
    let b = env.owner().await;
    let url = upload_url(&env).await;
    let before = event_count(&env).await;
    for req in [
        upload_request(&b, &url).header(ORIGIN, SITE),
        upload_request(&b, &url)
            .header(ORIGIN, SITE)
            .header("x-csrf-token", "wrong"),
        upload_request(&b, &url)
            .header(ORIGIN, "https://other.example.com")
            .header("x-csrf-token", b.csrf.as_ref().unwrap()),
    ] {
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "csrf");
    }
    assert_eq!(event_count(&env).await, before);
    assert!(env.t.server.engine.state().attachments.is_empty());
}

#[tokio::test]
async fn an_expired_session_cannot_upload() {
    let env = Env::start().await;
    let b = env.owner().await;
    let url = upload_url(&env).await;
    env.advance(Duration::from_secs(61 * 60));
    let res = upload_request(&b, &url)
        .header(ORIGIN, SITE)
        .header("x-csrf-token", b.csrf.as_ref().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(env.t.server.engine.state().attachments.is_empty());
}

#[tokio::test]
async fn session_uploads_keep_the_streaming_attachment_size_limit() {
    let env = Env::start_with(|c| c.attachments.max_upload_bytes = 7).await;
    let b = env.owner().await;
    let url = upload_url(&env).await;
    // No Content-Length: this must pass session auth and hit the streaming cap in the handler.
    let body = reqwest::Body::wrap_stream(futures_util::stream::iter([
        Ok::<_, std::io::Error>("A let"),
        Ok("ter"),
    ]));
    let res = upload_request(&b, &url)
        .body(body)
        .header(ORIGIN, SITE)
        .header("x-csrf-token", b.csrf.as_ref().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["error"]["code"], "payload_too_large");
    assert_eq!(v["error"]["details"]["max_bytes"], 7);
    assert!(env.t.server.engine.state().attachments.is_empty());
}

#[tokio::test]
async fn bearer_uploads_keep_working_and_take_precedence_over_a_session() {
    let env = Env::start().await;
    let b = env.owner().await;
    let url = upload_url(&env).await;
    let res = upload_request(&b, &url)
        .bearer_auth("tda_nope")
        .header(ORIGIN, SITE)
        .header("x-csrf-token", b.csrf.as_ref().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(env.t.server.engine.state().attachments.is_empty());

    let res = upload_request(&b, &url)
        .bearer_auth(TOKEN_A)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let v: Value = res.json().await.unwrap();
    assert_eq!(
        v["attachment"]["created"]["by"],
        json!({"type": "owner", "device": "alpha"})
    );
}

#[tokio::test]
async fn the_app_needs_a_session_and_gets_its_csrf_token_and_a_strict_csp() {
    let (env, _dist) = env_with_app().await;

    // Signed out: every app route goes to sign-in, files included.
    let anon = env.browser();
    for path in ["/app", "/app/", "/app/list/x", "/app/todo_web_bg.wasm"] {
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
    let session = b.get("/auth/session").await.json();
    let scope = session["owner_scope"].as_str().unwrap();
    assert_eq!(scope.len(), 64);
    assert!(scope.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(scope, csrf);
    for path in ["/app", "/app/", "/app/list/abc", "/app/task/abc/steps"] {
        let p = b.get(path).await;
        assert_eq!(p.status, StatusCode::OK, "{path}");
        assert_eq!(p.header(CONTENT_SECURITY_POLICY), APP_CSP, "{path}");
        assert_eq!(p.header(CACHE_CONTROL), "no-store, no-transform");
        assert!(
            p.body
                .contains(&format!("<meta name=\"csrf-token\" content=\"{csrf}\">")),
            "{}",
            p.body
        );
        assert!(p.body.contains(&format!(
            "<meta name=\"todo-owner-scope\" content=\"{scope}\">"
        )));
        assert!(!p.body.contains(SESSION_MARKER));
    }
    // A different session for the same owner sees the same scope but a different CSRF token.
    let mut other = b;
    other.login().await;
    assert_ne!(other.csrf.as_deref(), Some(csrf.as_str()));
    assert_eq!(
        other.get("/auth/session").await.json()["owner_scope"],
        scope
    );
    let b = other;
    // The CSP allows wasm compilation but no inline script or style, and limits connections.
    assert!(APP_CSP.contains("script-src 'self' 'wasm-unsafe-eval'"));
    assert!(
        APP_CSP
            .contains("connect-src 'self' https://*.ts.net http://127.0.0.1:* http://localhost:*;")
    );
    assert!(
        !APP_CSP
            .split_ascii_whitespace()
            .any(|source| source == "https:")
    );
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

    // A client that accepts gzip gets the build's gzipped copy, under its own ETag.
    let gz = |accept: &'static str| {
        reqwest::Client::new()
            .get(format!("{}/app/todo_web_bg.wasm", b.url))
            .header(
                COOKIE,
                format!("__Host-todo_session={}", b.cookie.as_ref().unwrap()),
            )
            .header(ACCEPT_ENCODING, accept)
            .send()
    };
    let res = gz("gzip, deflate, br").await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[CONTENT_ENCODING], "gzip");
    assert_eq!(res.headers()[CONTENT_TYPE], "application/wasm");
    assert_eq!(res.headers()[VARY], "accept-encoding");
    assert_ne!(res.headers()[ETAG].to_str().unwrap(), etag);
    assert_eq!(res.bytes().await.unwrap().as_ref(), b"gzipped");
    let res = gz("br, gzip;q=0").await.unwrap();
    assert!(res.headers().get(CONTENT_ENCODING).is_none());
    assert_eq!(res.bytes().await.unwrap().as_ref(), b"\0asm\x01\0\0\0");
    // The copy is not a file of its own.
    assert_eq!(
        b.get("/app/todo_web_bg.wasm.gz").await.status,
        StatusCode::NOT_FOUND
    );

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
    assert_eq!(b.get("/app/").await.status, StatusCode::NOT_FOUND);
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

#[tokio::test]
async fn responses_that_carry_a_secret_are_neither_stored_nor_transformed() {
    let (env, _dist) = env_with_app().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    // The app's page (the CSRF meta), an admin page (CSRF form fields), the token endpoint and API
    // JSON: a proxy must not compress any of them (BREACH).
    for path in ["/app", "/app/list/x", "/admin", "/auth/session"] {
        let p = b.get(path).await;
        assert_eq!(p.status, StatusCode::OK, "{path}");
        assert_eq!(p.header(CACHE_CONTROL), "no-store, no-transform", "{path}");
    }
    let (s, h, _) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[CACHE_CONTROL], "no-store, no-transform");
    let (s, h, _) = api(&b, Method::GET, "/api/v1/lists", None, None, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(h[CACHE_CONTROL], "no-store, no-transform");
    // The static files carry no secret and keep their revalidation.
    let p = b.get("/app/boot.js").await;
    assert_eq!(p.header(CACHE_CONTROL), "private, no-cache");
}

#[tokio::test]
async fn the_session_token_endpoint_answers_the_current_token() {
    let env = Env::start().await;
    let mut b = env.owner().await;
    let p = b.get("/auth/session").await;
    assert_eq!(p.status, StatusCode::OK);
    assert!(p.header(CONTENT_TYPE).starts_with("application/json"));
    assert_eq!(p.json()["csrf"], json!(b.csrf.clone().unwrap()));

    // Signing in again (another tab) replaces the cookie: the old token is refused, the endpoint
    // gives the new one, and that one works.
    let old = b.csrf.clone().unwrap();
    b.login().await;
    let (s, _, v) = api(&b, Method::GET, "/api/v1/lists", Some(&old), None, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["error"]["code"], "csrf");
    let fresh = b.get("/auth/session").await.json()["csrf"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(fresh, old);
    let (s, _, _) = api(&b, Method::GET, "/api/v1/lists", Some(&fresh), None, None).await;
    assert_eq!(s, StatusCode::OK);

    // Without a session: 401 JSON, never a redirect to the sign-in page.
    let p = env.browser().get("/auth/session").await;
    assert_eq!(p.status, StatusCode::UNAUTHORIZED);
    assert_eq!(p.json()["error"]["code"], "signed_out");
}

#[tokio::test]
async fn only_a_request_that_passes_the_csrf_check_keeps_the_session_alive() {
    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    // The idle timeout is an hour. Requests with a wrong token at 40 minutes don't extend it...
    env.advance(Duration::from_secs(40 * 60));
    for _ in 0..3 {
        let (s, _, _) = api(&b, Method::GET, "/api/v1/lists", Some("wrong"), None, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
    }
    let (s, _, _) = api(
        &b,
        Method::POST,
        "/api/v1/commands",
        Some(&csrf),
        Some("https://evil.example.com"),
        Some(create_list("Nope")),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // ...so at 61 minutes the session has ended.
    env.advance(Duration::from_secs(21 * 60));
    let (s, _, _) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // A good request does extend it.
    let mut b = b;
    b.login().await;
    let csrf = b.csrf.clone().unwrap();
    env.advance(Duration::from_secs(40 * 60));
    let (s, _, _) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::OK);
    env.advance(Duration::from_secs(40 * 60));
    let (s, _, _) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::OK);
}

/// `GET /auth/session` with the session cookie and optional fetch metadata.
async fn session_token(
    b: &Browser,
    site: Option<&str>,
    origin: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = reqwest::Client::new()
        .get(format!("{}/auth/session", b.url))
        .header(
            COOKIE,
            format!("__Host-todo_session={}", b.cookie.as_ref().unwrap()),
        );
    if let Some(s) = site {
        req = req.header("sec-fetch-site", s);
    }
    if let Some(o) = origin {
        req = req.header(ORIGIN, o);
    }
    let res = req.send().await.unwrap();
    let status = res.status();
    (status, res.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_session_token_answers_only_this_origin_and_keeps_no_session_alive() {
    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    // This origin's own pages (and clients that send no fetch metadata) get it.
    for (site, origin) in [
        (Some("same-origin"), None),
        (Some("same-origin"), Some(SITE)),
        (None, None),
    ] {
        let (s, v) = session_token(&b, site, origin).await;
        assert_eq!(s, StatusCode::OK, "{site:?} {origin:?}");
        assert_eq!(v["csrf"], json!(csrf));
    }
    // A sibling origin on the same site, another site, or a navigation from elsewhere does not.
    for (site, origin) in [
        (Some("same-site"), None),
        (Some("cross-site"), None),
        (Some("none"), None),
        (Some("same-site"), Some("https://other.todo.example.com")),
        (None, Some("https://other.todo.example.com")),
        (Some("same-origin"), Some("https://evil.example.com")),
    ] {
        let (s, v) = session_token(&b, site, origin).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{site:?} {origin:?}");
        assert_eq!(v["error"]["code"], "csrf");
    }

    // Asking for the token is not activity: at 40 minutes, then every few minutes, it doesn't extend
    // the hour of idle time, and at 61 minutes the session has ended.
    env.advance(Duration::from_secs(40 * 60));
    for _ in 0..4 {
        let (s, _) = session_token(&b, Some("same-origin"), None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = session_token(&b, Some("same-site"), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        env.advance(Duration::from_secs(5 * 60));
    }
    let (s, _, _) = api(&b, Method::GET, "/api/v1/lists", Some(&csrf), None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v) = session_token(&b, Some("same-origin"), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["code"], "signed_out");
}
