//! The local admin API (`todo_server::admin`): the loopback listener, its token and browser guard,
//! every operation end to end, the audit entries, and the `admin-token` command.
//!
//! `tests/security.rs` checks the other side: the public listener has no admin routes, and the
//! admin token opens nothing there.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::{ADMIN_TOKEN, TOKEN_A, TestServer, admin_on, ingest};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, HOST, ORIGIN};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use todo_core::{AccessLevel, AgentId};
use todo_server::auth::store::ShareTarget;
use todo_server::{Config, Options, Server};

async fn server() -> TestServer {
    TestServer::start_with(|cfg| {
        ingest::enabled(cfg);
        admin_on(cfg);
    })
    .await
}

fn admin_url(t: &TestServer) -> String {
    t.server.admin_url().expect("the admin listener is on")
}

/// A request to the admin listener with `token` (if any) and nothing else.
async fn admin(
    t: &TestServer,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = t.client.request(method, format!("{}{path}", admin_url(t)));
    if let Some(tok) = token {
        req = req.bearer_auth(tok);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.unwrap();
    let status = res.status();
    let text = res.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn ok(t: &TestServer, method: Method, path: &str, body: Option<Value>) -> Value {
    let (s, v) = admin(t, method.clone(), path, Some(ADMIN_TOKEN), body).await;
    assert!(s.is_success(), "{method} {path}: {s} {v}");
    v
}

/// The audit entries newer than `after`, oldest first.
fn audit_since(t: &TestServer, after: i64) -> Vec<(String, Value, Option<String>)> {
    let mut rows = t.server.authsvc.store().audit_page(10_000).unwrap();
    rows.retain(|r| r.id > after);
    rows.reverse();
    rows.into_iter()
        .map(|r| (r.kind, serde_json::from_str(&r.detail).unwrap(), r.ip))
        .collect()
}

fn last_audit_id(t: &TestServer) -> i64 {
    t.server
        .authsvc
        .store()
        .audit_page(1)
        .unwrap()
        .first()
        .map_or(0, |r| r.id)
}

// ---- the listener ---------------------------------------------------------------------------------

#[tokio::test]
async fn the_admin_listener_is_off_unless_configured() {
    let t = TestServer::start().await;
    assert_eq!(t.server.admin_addr, None);
    assert_eq!(t.server.admin_url(), None);
}

#[tokio::test]
async fn a_non_loopback_admin_bind_is_refused() {
    for bind in ["0.0.0.0:0", "[::]:0"] {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.path().to_owned(),
            ..Config::default()
        };
        admin_on(&mut cfg);
        cfg.admin.bind = Some(bind.parse().unwrap());
        let e = Server::bind(&cfg, Options::default()).await.unwrap_err();
        assert!(
            e.to_string().contains("not a loopback address"),
            "{bind}: {e}"
        );
    }
    // A bind without a token is refused too.
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().to_owned(),
        ..Config::default()
    };
    cfg.admin.bind = Some("127.0.0.1:0".parse().unwrap());
    assert!(Server::bind(&cfg, Options::default()).await.is_err());
}

#[tokio::test]
async fn the_admin_listener_binds_its_own_loopback_socket() {
    let t = server().await;
    let a = t.server.admin_addr.unwrap();
    assert!(a.ip().is_loopback());
    assert_ne!(a, t.server.addr);
    // The admin listener serves nothing of the public API.
    for path in ["/health", "/api/v1/lists", "/login", "/admin", "/mcp"] {
        let (s, _) = admin(&t, Method::GET, path, Some(ADMIN_TOKEN), None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}");
    }
    let (s, v) = admin(
        &t,
        Method::GET,
        "/api/v1/admin/openapi.json",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["paths"]["/api/v1/admin/agents"].is_object());
}

// ---- the credential -------------------------------------------------------------------------------

#[tokio::test]
async fn only_the_admin_token_in_the_authorization_header_gets_in() {
    let t = server().await;
    let svc = &t.server.authsvc;
    let (_, device) = svc.create_device("Laptop", None).unwrap();
    let (_, agent) = svc
        .create_agent(
            &AgentId::new("bot").unwrap(),
            AccessLevel::Plan,
            None,
            None,
            None,
        )
        .unwrap();
    let url = format!("{}/api/v1/admin/agents", admin_url(&t));
    let wrong = format!("tdx_{}", "A".repeat(43));
    for token in [
        None,
        Some(wrong.as_str()),
        Some(TOKEN_A),
        Some(device.as_str()),
        Some(agent.as_str()),
        Some("tdx_"),
        Some(&ADMIN_TOKEN[4..]),
        Some(&ADMIN_TOKEN[..ADMIN_TOKEN.len() - 1]),
    ] {
        let (s, v) = admin(&t, Method::GET, "/api/v1/admin/agents", token, None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{token:?}: {v}");
        assert_eq!(v["error"]["code"], "unauthorized");
    }
    // Anywhere but `Authorization: Bearer`.
    for req in [
        t.client.get(format!("{url}?token={ADMIN_TOKEN}")),
        t.client.get(format!("{url}?access_token={ADMIN_TOKEN}")),
        t.client.get(&url).header("x-api-key", ADMIN_TOKEN),
        t.client
            .get(&url)
            .header(AUTHORIZATION, format!("Basic {ADMIN_TOKEN}")),
        t.client.get(&url).header(AUTHORIZATION, ADMIN_TOKEN),
        t.client.get(&url).basic_auth("admin", Some(ADMIN_TOKEN)),
    ] {
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
    // Unknown admin paths need the token too.
    let (s, _) = admin(&t, Method::GET, "/api/v1/admin/nope", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = admin(
        &t,
        Method::GET,
        "/api/v1/admin/nope",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // The right one works, with either case of the scheme.
    let res = t
        .client
        .get(&url)
        .header(AUTHORIZATION, format!("bearer {ADMIN_TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn browsers_cannot_drive_the_admin_api() {
    let t = server().await;
    let url = format!("{}/api/v1/admin/ingest/senders", admin_url(&t));
    let body = json!({ "address": "mallory@example.org" });
    // A cross-site page's fetch: its preflight, and the request itself, both carry `Origin`.
    let preflight = t
        .client
        .request(Method::OPTIONS, &url)
        .header(ORIGIN, "https://evil.example.org")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization, content-type",
        )
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::FORBIDDEN);
    assert!(
        preflight
            .headers()
            .keys()
            .all(|k| !k.as_str().starts_with("access-control-")),
        "no CORS grant"
    );
    let dressed = [
        (ORIGIN.as_str(), "https://evil.example.org"),
        (ORIGIN.as_str(), "null"),
        (ORIGIN.as_str(), "http://127.0.0.1"),
        ("sec-fetch-site", "same-origin"),
        ("sec-fetch-mode", "navigate"),
        (COOKIE.as_str(), "a=b"),
    ];
    for (name, value) in dressed {
        let res = t
            .client
            .post(&url)
            .bearer_auth(ADMIN_TOKEN)
            .header(name, value)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{name}: {value}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "browser_refused");
    }
    // DNS rebinding: a page on another name that resolves to loopback.
    for host in [
        "evil.example.org",
        "evil.example.org:1234",
        "todo.example.com",
    ] {
        let res = t
            .client
            .post(&url)
            .bearer_auth(ADMIN_TOKEN)
            .header(HOST, host)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{host}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["error"]["code"], "bad_host");
    }
    // Nothing was added.
    assert!(
        !t.server
            .ingest
            .store
            .is_allowed("mallory@example.org")
            .unwrap()
    );
    let res = t
        .client
        .post(&url)
        .bearer_auth(ADMIN_TOKEN)
        .header(HOST, "localhost")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn requests_and_failures_are_rate_limited() {
    let t = TestServer::start_with(|cfg| {
        admin_on(cfg);
        cfg.admin.requests_per_minute = 1;
        cfg.admin.request_burst = 3;
    })
    .await;
    for _ in 0..3 {
        let (s, _) = admin(
            &t,
            Method::GET,
            "/api/v1/admin/lists",
            Some(ADMIN_TOKEN),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let res = t
        .client
        .get(format!("{}/api/v1/admin/lists", admin_url(&t)))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key("retry-after"));

    // Wrong tokens spend a budget of their own; once it is gone they get 429, but the right token
    // still gets in (another local process can't lock the operator out).
    let t = TestServer::start_with(|cfg| {
        admin_on(cfg);
        cfg.admin.failures_per_minute = 1;
        cfg.admin.failure_burst = 3;
    })
    .await;
    for _ in 0..3 {
        let (s, _) = admin(
            &t,
            Method::GET,
            "/api/v1/admin/lists",
            Some("tdx_wrong"),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, _) = admin(
        &t,
        Method::GET,
        "/api/v1/admin/lists",
        Some("tdx_wrong"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    let (s, _) = admin(
        &t,
        Method::GET,
        "/api/v1/admin/lists",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = admin(&t, Method::GET, "/api/v1/admin/lists", None, None).await;
    assert_eq!(
        s,
        StatusCode::TOO_MANY_REQUESTS,
        "missing tokens are failures too"
    );
}

#[tokio::test]
async fn an_admin_change_whose_audit_entry_fails_is_not_made() {
    let t = server().await;
    let svc = t.server.authsvc.clone();
    let list = t.create_list("Mine").await;
    let (id, token) = svc
        .create_agent(
            &AgentId::new("helper").unwrap(),
            AccessLevel::Read,
            Some(5),
            None,
            None,
        )
        .unwrap();
    let (device, device_token) = svc.create_device("Phone", None).unwrap();
    let share_token = svc
        .create_share(ShareTarget::List(list.parse().unwrap()), None)
        .unwrap();
    let share = svc.store().shares().unwrap()[0].id;
    let audited = svc.store().audit_page(10_000).unwrap().len();
    // Every audit insert fails from now on.
    rusqlite::Connection::open(t.db_path())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT, 'audit down'); END;",
        )
        .unwrap();

    let id_path = |p: &str| p.replace("{id}", &id.to_string());
    for (method, path, body) in [
        (
            Method::PUT,
            id_path("/api/v1/admin/agents/{id}/access"),
            Some(json!({ "grants": { "default": "plan" }, "writes_per_hour": 500 })),
        ),
        (
            Method::POST,
            id_path("/api/v1/admin/agents/{id}/revoke"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/admin/devices/{device}/revoke"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/admin/shares/{share}/revoke"),
            None,
        ),
        (
            Method::PUT,
            "/api/v1/admin/ingest/inbox".to_owned(),
            Some(json!({ "list": list })),
        ),
        (
            Method::POST,
            "/api/v1/admin/ingest/senders".to_owned(),
            Some(json!({ "address": "new@example.org" })),
        ),
        (
            Method::POST,
            "/api/v1/admin/ingest/keys".to_owned(),
            Some(json!({ "key": "mine", "list": list })),
        ),
        (
            Method::DELETE,
            "/api/v1/admin/ingest/senders/you@example.com".to_owned(),
            None,
        ),
    ] {
        let (s, v) = admin(&t, method.clone(), &path, Some(ADMIN_TOKEN), body).await;
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{method} {path}: {v}");
    }

    let row = svc.store().agent(id).unwrap().unwrap();
    assert_eq!(row.grants.default, AccessLevel::Read, "grants unchanged");
    assert_eq!(row.writes_per_hour, 5, "quota unchanged");
    assert_eq!(row.revoked_at, None);
    for tok in [&token, &device_token] {
        let (s, _, _) = t
            .request(Method::GET, "/api/v1/lists", Some(tok), None)
            .await;
        assert_eq!(s, StatusCode::OK);
    }
    let res = t
        .client
        .get(format!("{}/s/{share_token}", t.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let store = &t.server.ingest.store;
    assert_eq!(store.inbox().unwrap(), None);
    assert!(!store.is_allowed("new@example.org").unwrap());
    assert!(store.is_allowed("you@example.com").unwrap());
    assert!(store.list_keys().unwrap().is_empty());
    assert_eq!(svc.store().audit_page(10_000).unwrap().len(), audited);
}

// ---- operations -----------------------------------------------------------------------------------

#[tokio::test]
async fn lists_and_collections_are_names_only() {
    let t = server().await;
    let col = common::id();
    t.ok(json!({ "cmd": "create_collection", "id": col, "name": "Work" }))
        .await;
    let list = common::id();
    t.ok(json!({ "cmd": "create_list", "id": list, "name": "Projects", "collection": col }))
        .await;
    let top = t.create_list("Errands").await;
    t.create_task(&top, "zzsecret task title", json!({})).await;
    let v = ok(&t, Method::GET, "/api/v1/admin/lists", None).await;
    assert_eq!(v["collections"], json!([{ "id": col, "name": "Work" }]));
    let lists = v["lists"].as_array().unwrap();
    assert!(lists.contains(&json!({ "id": list, "name": "Projects", "collection": col })));
    assert!(lists.contains(&json!({ "id": top, "name": "Errands", "collection": null })));
    assert!(!v.to_string().contains("zzsecret"), "no task content");
}

#[tokio::test]
async fn ingest_settings_made_here_take_effect_for_email() {
    let t = server().await;
    let inbox = t.create_list("Inbox").await;
    let errands = t.create_list("Errands").await;
    let before = last_audit_id(&t);

    // Without an Inbox, email is refused.
    let v = ok(&t, Method::GET, "/api/v1/admin/ingest", None).await;
    assert_eq!(v["enabled"], true);
    assert_eq!(v["inbox"], Value::Null);
    assert_eq!(
        v["senders"].as_array().unwrap().len(),
        2,
        "seeded from the config"
    );
    let (s, _) = ingest::send(&t, &ingest::payload("<m1@example.org>")).await;
    assert!(!s.is_success(), "{s}");

    // Set the Inbox.
    let v = ok(
        &t,
        Method::PUT,
        "/api/v1/admin/ingest/inbox",
        Some(json!({ "list": inbox })),
    )
    .await;
    assert_eq!(
        v,
        json!({ "inbox": { "id": inbox, "name": "Inbox" }, "changed": true })
    );
    let v = ok(
        &t,
        Method::PUT,
        "/api/v1/admin/ingest/inbox",
        Some(json!({ "list": inbox })),
    )
    .await;
    assert_eq!(v["changed"], false);
    let (s, v) = admin(
        &t,
        Method::PUT,
        "/api/v1/admin/ingest/inbox",
        Some(ADMIN_TOKEN),
        Some(json!({ "list": common::id() })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");

    // A new sender: its mail is refused until it is allowed.
    let mut p = ingest::payload("<m2@example.org>");
    p["forwarded_by"] = json!("new@example.org");
    let (s, _) = ingest::send(&t, &p).await;
    assert!(!s.is_success(), "{s}");
    let (s, v) = admin(
        &t,
        Method::POST,
        "/api/v1/admin/ingest/senders",
        Some(ADMIN_TOKEN),
        Some(json!({ "address": " New@Example.org " })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(v, json!({ "address": "new@example.org", "added": true }));
    let (s, v) = admin(
        &t,
        Method::POST,
        "/api/v1/admin/ingest/senders",
        Some(ADMIN_TOKEN),
        Some(json!({ "address": "new@example.org" })),
    )
    .await;
    assert_eq!((s, v["added"].clone()), (StatusCode::OK, json!(false)));
    let (s, _) = admin(
        &t,
        Method::POST,
        "/api/v1/admin/ingest/senders",
        Some(ADMIN_TOKEN),
        Some(json!({ "address": "not an address" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, v) = ingest::send(&t, &p).await;
    assert!(s.is_success(), "{s} {v}");
    let task = ingest::task_id(&v);
    let state = t.server.engine.state();
    assert_eq!(
        state.tasks[&task.parse().unwrap()].list.to_string(),
        inbox,
        "to the Inbox set here"
    );

    // A list key.
    let (s, v) = admin(
        &t,
        Method::POST,
        "/api/v1/admin/ingest/keys",
        Some(ADMIN_TOKEN),
        Some(json!({ "key": "errands", "list": errands })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(
        v,
        json!({ "key": "errands", "list": { "id": errands, "name": "Errands" } })
    );
    for (body, status) in [
        (
            json!({ "key": "errands", "list": inbox }),
            StatusCode::CONFLICT,
        ),
        (
            json!({ "key": "inbox", "list": inbox }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "key": "Bad Key", "list": inbox }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "key": "other", "list": common::id() }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (json!({ "key": "other" }), StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let (s, v) = admin(
            &t,
            Method::POST,
            "/api/v1/admin/ingest/keys",
            Some(ADMIN_TOKEN),
            Some(body.clone()),
        )
        .await;
        assert_eq!(s, status, "{body}: {v}");
    }
    let mut p = ingest::payload("<m3@example.org>");
    p["list"] = json!("errands");
    let (s, v) = ingest::send(&t, &p).await;
    assert!(s.is_success(), "{s} {v}");
    let state = t.server.engine.state();
    assert_eq!(
        state.tasks[&ingest::task_id(&v).parse().unwrap()]
            .list
            .to_string(),
        errands,
        "to the list the key names"
    );
    let v = ok(&t, Method::GET, "/api/v1/admin/ingest/keys", None).await;
    assert_eq!(
        v["keys"],
        json!([{ "key": "errands", "list": { "id": errands, "name": "Errands" } }])
    );
    let v = ok(&t, Method::GET, "/api/v1/admin/ingest/senders", None).await;
    assert!(
        v["senders"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["address"] == "new@example.org" && s["source"] == "admin")
    );
    let v = ok(&t, Method::GET, "/api/v1/admin/ingest", None).await;
    assert_eq!(v["inbox"]["id"], inbox);
    assert_eq!(v["keys"][0]["key"], "errands");

    // Removing them takes effect too.
    let (s, _) = admin(
        &t,
        Method::DELETE,
        "/api/v1/admin/ingest/keys/errands",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = admin(
        &t,
        Method::DELETE,
        "/api/v1/admin/ingest/keys/errands",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = admin(
        &t,
        Method::DELETE,
        "/api/v1/admin/ingest/senders/New%40Example.org",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = admin(
        &t,
        Method::DELETE,
        "/api/v1/admin/ingest/senders/new@example.org",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let mut p = ingest::payload("<m4@example.org>");
    p["forwarded_by"] = json!("new@example.org");
    let (s, _) = ingest::send(&t, &p).await;
    assert!(!s.is_success(), "removed senders are refused again: {s}");
    let v = ok(
        &t,
        Method::PUT,
        "/api/v1/admin/ingest/inbox",
        Some(json!({ "list": null })),
    )
    .await;
    assert_eq!(v, json!({ "inbox": null, "changed": true }));
    let (s, _) = ingest::send(&t, &ingest::payload("<m5@example.org>")).await;
    assert!(!s.is_success(), "no Inbox: {s}");

    // Every change is audited, by the admin API, from the TCP peer.
    let entries = audit_since(&t, before);
    let kinds: Vec<&str> = entries.iter().map(|(k, _, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "ingest_inbox_set",
            "ingest_sender_added",
            "list_key_added",
            "list_key_removed",
            "ingest_sender_removed",
            "ingest_inbox_set",
        ]
    );
    for (kind, detail, ip) in &entries {
        assert_eq!(detail["actor"], "admin API", "{kind}: {detail}");
        assert_eq!(ip.as_deref(), Some("127.0.0.1"), "{kind}");
    }
    assert_eq!(entries[1].1["address"], "new@example.org");
    assert_eq!(
        entries[2].1,
        json!({ "key": "errands", "list": errands, "actor": "admin API" })
    );
}

#[tokio::test]
async fn agents_are_listed_retuned_and_revoked() {
    let t = server().await;
    let svc = t.server.authsvc.clone();
    let mine = t.create_list("Mine").await;
    let theirs = t.create_list("Theirs").await;
    let (id, token) = svc
        .create_agent(
            &AgentId::new("helper").unwrap(),
            AccessLevel::Plan,
            Some(50),
            None,
            None,
        )
        .unwrap();
    let before = last_audit_id(&t);

    let v = ok(&t, Method::GET, "/api/v1/admin/agents", None).await;
    let a = &v["agents"][0];
    assert_eq!(a["id"], id);
    assert_eq!(a["agent"], "helper");
    assert_eq!(a["status"], "active");
    assert_eq!(a["writes_per_hour"], 50);
    assert_eq!(
        a["grants"],
        json!({ "default": "plan", "collections": {}, "lists": {} })
    );
    assert!(!v.to_string().contains(&token), "no secrets");
    assert!(
        !v.to_string()
            .contains(&todo_server::auth::hash_token(&token)),
        "no hashes"
    );
    let one = ok(&t, Method::GET, &format!("/api/v1/admin/agents/{id}"), None).await;
    assert_eq!(&one, a);
    let (s, _) = admin(
        &t,
        Method::GET,
        "/api/v1/admin/agents/999",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // The agent sees both lists, then only its own.
    let visible = |t: &TestServer| {
        let (token, url, client) = (token.clone(), t.url.clone(), t.client.clone());
        async move {
            let v: Value = client
                .get(format!("{url}/api/v1/lists"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let mut ids: Vec<String> = v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["id"].as_str().unwrap().to_owned())
                .collect();
            ids.sort();
            ids
        }
    };
    assert_eq!(visible(&t).await.len(), 2);
    let body = json!({
        "grants": { "default": "none", "lists": { mine.clone(): "read" } },
        "writes_per_hour": 7
    });
    let v = ok(
        &t,
        Method::PUT,
        &format!("/api/v1/admin/agents/{id}/access"),
        Some(body),
    )
    .await;
    assert_eq!(v["writes_per_hour"], 7);
    assert_eq!(
        v["grants"],
        json!({ "default": "none", "collections": {}, "lists": { mine.clone(): "read" } })
    );
    assert_eq!(visible(&t).await, vec![mine.clone()]);
    assert!(!visible(&t).await.contains(&theirs));
    let row = svc.store().agent(id).unwrap().unwrap();
    assert_eq!(row.writes_per_hour, 7);
    // Read only now: writes are refused.
    let (s, _, _) = t
        .command_with_id(
            &token,
            &common::id(),
            json!({ "cmd": "create_task", "id": common::id(), "list": mine, "title": "x" }),
        )
        .await;
    assert!(s.is_client_error(), "{s}");

    for (body, status) in [
        (
            json!({ "grants": { "default": "read" }, "writes_per_hour": 0 }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "grants": { "default": "read", "lists": { common::id(): "read" } }, "writes_per_hour": 1 }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "grants": { "default": "read", "collections": { "x": "read" } }, "writes_per_hour": 1 }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "grants": { "default": "admin" }, "writes_per_hour": 1 }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "grants": { "default": "read" } }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (s, v) = admin(
            &t,
            Method::PUT,
            &format!("/api/v1/admin/agents/{id}/access"),
            Some(ADMIN_TOKEN),
            Some(body.clone()),
        )
        .await;
        assert!(
            s == status || s == StatusCode::BAD_REQUEST,
            "{body}: {s} {v}"
        );
    }

    // Revoked: the token stops working at once.
    let (s, _) = admin(
        &t,
        Method::POST,
        &format!("/api/v1/admin/agents/{id}/revoke"),
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _, _) = t
        .request(Method::GET, "/api/v1/lists", Some(&token), None)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = admin(
        &t,
        Method::POST,
        &format!("/api/v1/admin/agents/{id}/revoke"),
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = admin(
        &t,
        Method::PUT,
        &format!("/api/v1/admin/agents/{id}/access"),
        Some(ADMIN_TOKEN),
        Some(json!({ "grants": { "default": "plan" }, "writes_per_hour": 1 })),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "a revoked token can't be given access again"
    );
    let v = ok(&t, Method::GET, &format!("/api/v1/admin/agents/{id}"), None).await;
    assert_eq!(v["status"], "revoked");

    let entries = audit_since(&t, before);
    let kinds: Vec<&str> = entries.iter().map(|(k, _, _)| k.as_str()).collect();
    assert_eq!(kinds, ["grants_changed", "agent_token_revoked"]);
    let changed = &entries[0].1;
    assert_eq!(changed["actor"], "admin API");
    assert_eq!(changed["writes_per_hour_before"], 50);
    assert_eq!(changed["writes_per_hour_after"], 7);
    assert_eq!(changed["agent"], "helper");
    assert_eq!(entries[1].1["actor"], "admin API");
    assert_eq!(entries[1].1["token_id"], id);
}

#[tokio::test]
async fn devices_and_share_links_are_listed_and_revoked() {
    let t = server().await;
    let svc = t.server.authsvc.clone();
    let list = t.create_list("Shared").await;
    let (device, device_token) = svc.create_device("Phone", None).unwrap();
    let share_token = svc
        .create_share(ShareTarget::List(list.parse().unwrap()), None)
        .unwrap();
    let before = last_audit_id(&t);

    let v = ok(&t, Method::GET, "/api/v1/admin/devices", None).await;
    let d = &v["devices"][0];
    assert_eq!(d["id"], device.as_str());
    assert_eq!(d["name"], "Phone");
    assert_eq!(d["revoked_at"], Value::Null);
    assert!(!v.to_string().contains(&device_token));
    let v = ok(&t, Method::GET, "/api/v1/admin/shares", None).await;
    let share = &v["shares"][0];
    assert_eq!(
        share["target"],
        json!({ "kind": "list", "id": list, "name": "Shared" })
    );
    assert!(!v.to_string().contains(&share_token));
    let share_id = share["id"].as_i64().unwrap();

    let (s, _) = admin(
        &t,
        Method::POST,
        &format!("/api/v1/admin/devices/{device}/revoke"),
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _, _) = t
        .request(Method::GET, "/api/v1/lists", Some(&device_token), None)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = admin(
        &t,
        Method::POST,
        &format!("/api/v1/admin/devices/{device}/revoke"),
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let res = t
        .client
        .get(format!("{}/s/{share_token}", t.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let (s, _) = admin(
        &t,
        Method::POST,
        &format!("/api/v1/admin/shares/{share_id}/revoke"),
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let res = t
        .client
        .get(format!("{}/s/{share_token}", t.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let (s, _) = admin(
        &t,
        Method::POST,
        &format!("/api/v1/admin/shares/{share_id}/revoke"),
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let v = ok(&t, Method::GET, "/api/v1/admin/devices", None).await;
    assert!(v["devices"][0]["revoked_at"].is_i64());

    let entries = audit_since(&t, before);
    let kinds: Vec<&str> = entries.iter().map(|(k, _, _)| k.as_str()).collect();
    assert_eq!(kinds, ["device_revoked", "share_revoked"]);
    assert_eq!(
        entries[0].1,
        json!({ "device": device.as_str(), "actor": "admin API" })
    );
    assert_eq!(
        entries[1].1,
        json!({ "share_id": share_id, "actor": "admin API" })
    );
}

#[tokio::test]
async fn nothing_creates_a_credential_or_reaches_task_data() {
    let t = server().await;
    let list = t.create_list("Private").await;
    t.create_task(&list, "zzsecret title", json!({})).await;
    // None of these exist on the admin listener, with any method.
    for path in [
        "/api/v1/admin/agents/new",
        "/api/v1/admin/devices/pair",
        "/api/v1/admin/pairings",
        "/api/v1/admin/shares/1/rotate",
        "/api/v1/admin/passkeys",
        "/api/v1/admin/tokens",
        "/api/v1/admin/tasks",
        "/api/v1/admin/events",
        "/api/v1/admin/commands",
        "/api/v1/tasks",
        "/api/v1/events",
        "/api/v1/commands",
        "/mcp",
    ] {
        for m in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
            let (s, _) = admin(&t, m.clone(), path, Some(ADMIN_TOKEN), Some(json!({}))).await;
            // `agents/new` matches `agents/{id}` (a GET with a bad ID is 400).
            let bad_id = s == StatusCode::BAD_REQUEST && path.ends_with("/agents/new");
            assert!(
                bad_id || matches!(s, StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED),
                "{m} {path}: {s}"
            );
        }
    }
    for (m, path) in [
        (Method::POST, "/api/v1/admin/agents"),
        (Method::POST, "/api/v1/admin/devices"),
        (Method::POST, "/api/v1/admin/shares"),
        (Method::POST, "/api/v1/admin/shares/1/rotate"),
    ] {
        let (s, _) = admin(
            &t,
            m.clone(),
            path,
            Some(ADMIN_TOKEN),
            Some(json!({ "name": "x" })),
        )
        .await;
        assert!(
            matches!(s, StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED),
            "{m} {path}: {s}"
        );
    }
    // Every documented route answers, and none of the answers contains task content.
    let doc = todo_server::admin::openapi::spec();
    let doc = serde_json::to_value(doc).unwrap();
    for (path, item) in doc["paths"].as_object().unwrap() {
        if item.get("get").is_some() && !path.contains('{') {
            let (s, v) = admin(&t, Method::GET, path, Some(ADMIN_TOKEN), None).await;
            assert_eq!(s, StatusCode::OK, "{path}");
            assert!(!v.to_string().contains("zzsecret"), "{path}");
        }
    }
    assert!(svc_agents_empty(&t));
}

fn svc_agents_empty(t: &TestServer) -> bool {
    t.server.authsvc.agents().unwrap().is_empty()
        && t.server.authsvc.store().devices().unwrap().is_empty()
        && t.server.authsvc.store().shares().unwrap().is_empty()
}

#[tokio::test]
async fn the_audit_log_pages_newest_first() {
    let t = server().await;
    let svc = &t.server.authsvc;
    for i in 0..5 {
        svc.audit("test_entry", None, json!({ "i": i }));
    }
    let v = ok(&t, Method::GET, "/api/v1/admin/audit?limit=2", None).await;
    let e = v["entries"].as_array().unwrap();
    assert_eq!(e.len(), 2);
    assert_eq!(e[0]["kind"], "test_entry");
    assert_eq!(e[0]["detail"], json!({ "i": 4 }));
    assert_eq!(e[1]["detail"], json!({ "i": 3 }));
    let next = v["next_before"].as_i64().unwrap();
    assert_eq!(next, e[1]["id"].as_i64().unwrap());
    let v = ok(
        &t,
        Method::GET,
        &format!("/api/v1/admin/audit?limit=2&before={next}"),
        None,
    )
    .await;
    let e = v["entries"].as_array().unwrap();
    assert_eq!(e[0]["detail"], json!({ "i": 2 }));
    assert_eq!(e[1]["detail"], json!({ "i": 1 }));
    let v = ok(&t, Method::GET, "/api/v1/admin/audit?limit=500", None).await;
    assert_eq!(v["next_before"], Value::Null, "the last page");
    let (s, _) = admin(
        &t,
        Method::GET,
        "/api/v1/admin/audit?limit=x",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // Reading changes nothing, so nothing is audited.
    let n = svc.store().audit_page(10_000).unwrap().len();
    ok(&t, Method::GET, "/api/v1/admin/audit", None).await;
    ok(&t, Method::GET, "/api/v1/admin/agents", None).await;
    assert_eq!(svc.store().audit_page(10_000).unwrap().len(), n);
}

#[tokio::test]
async fn forwarding_headers_are_ignored_on_the_admin_listener() {
    let t = TestServer::start_with(|cfg| {
        admin_on(cfg);
        cfg.trust_cloudflare_headers = true;
    })
    .await;
    let before = last_audit_id(&t);
    let res = t
        .client
        .post(format!("{}/api/v1/admin/ingest/senders", admin_url(&t)))
        .bearer_auth(ADMIN_TOKEN)
        .header("cf-connecting-ip", "203.0.113.9")
        .header("x-forwarded-for", "203.0.113.10")
        .header(CONTENT_TYPE, "application/json")
        .body(r#"{"address":"x@example.org"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let entries = audit_since(&t, before);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].2.as_deref(), Some("127.0.0.1"));
}

// ---- the admin-token command ----------------------------------------------------------------------

#[tokio::test]
async fn admin_token_writes_an_owner_only_file_and_prints_its_hash() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("admin-token");
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_todo-server"))
            .args(["admin-token", "--out"])
            .arg(&out)
            // No config and no database: it works while a server is running.
            .env_remove("TODO_SERVER_CONFIG")
            .output()
            .unwrap()
    };
    let o = run();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    todo_fs::verify_private(&out, todo_fs::Private::File).unwrap();
    let token = std::fs::read_to_string(&out).unwrap();
    let token = token.trim();
    assert!(token.starts_with("tdx_"), "{token}");
    assert_eq!(token.len(), 47);
    let stdout = String::from_utf8(o.stdout).unwrap();
    let hash = todo_server::auth::hash_token(token);
    assert!(
        stdout.contains(&format!("token_sha256 = \"{hash}\"")),
        "{stdout}"
    );
    assert!(stdout.contains("[admin]"));
    assert!(!stdout.contains(token), "the token is only in the file");
    assert!(!String::from_utf8_lossy(&o.stderr).contains(token));
    // It never overwrites an existing file.
    let again = run();
    assert!(!again.status.success());
    assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), token);

    // The printed hash works as the config.
    let token = token.to_owned();
    let t = TestServer::start_with(|cfg| {
        cfg.admin.bind = Some("127.0.0.1:0".parse().unwrap());
        cfg.admin.token_sha256 = Some(hash);
    })
    .await;
    let (s, _) = admin(&t, Method::GET, "/api/v1/admin/lists", Some(&token), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = admin(
        &t,
        Method::GET,
        "/api/v1/admin/lists",
        Some(ADMIN_TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}
