//! The security suite: what an outsider or an agent can reach, in one place to audit.
//!
//! | Section | Guarantee |
//! |---|---|
//! | Route inventory | Built from the web route table, the OpenAPI document and the router source, so a new route can't slip past. The public set is exactly [`PUBLIC`]. |
//! | Anonymous access | Every method (HEAD, OPTIONS and unknown ones too) on every other route and on unknown paths is refused: 401, 403, 404 or 405, or a redirect to sign-in for a page load. The admin API token opens none of them either. Nothing is written. |
//! | Admin API | Its routes (`todo_server::admin`, on a separate loopback listener) don't exist on the public listener: 404 for every method and credential. `tests/admin.rs` covers the admin listener itself. |
//! | Public routes | They answer without credentials and reveal nothing: `/health` is an empty 200 without a version, and no response names the server or its version. |
//! | Token scope | Agents at none, read, capture and plan stay inside their grants across REST reads, queries, smart lists, task detail, activity, the event log and stream, commands, undo and every MCP tool. Hidden lists, tasks, steps, tags and events look missing. |
//! | Request limits | Oversized bodies get 413 on every route, with or without credentials, announced or chunked (the attachment upload and content routes have their own, larger cap; `tests/attachments.rs` covers their chunked case). Slow request heads are dropped and the connection cap holds. |
//! | Headers | HTML pages carry the CSP (`frame-ancestors 'none'`), `X-Frame-Options`, `nosniff` and `Referrer-Policy: same-origin` (not `no-referrer`, which makes browsers send `Origin: null` on our own form posts); HSTS is on every response when the origin is HTTPS and never otherwise. Every cookie is `__Host-`, `Secure`, `HttpOnly` and `SameSite=Strict`. |
//! | Email ingest | The one public write route takes no token: only a request signed with the Worker's secret, within the timestamp window and with an unused nonce gets in. A bad, missing or repeated signature, a tampered body, a stale or future timestamp, a replay and a bearer token in place of a signature are all 401 and store nothing. |
//! | Logs | Tokens, session cookies, codes and task content never reach the log at `info` or above. |
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::ingest;
use common::mcp::{ALL_TOOLS, Client, agent, call, connect, fails, ok};
use common::raw::{self, health, read_until_closed};
use common::web::{Env, Page, SITE, assert_session_cookie};
use common::{ADMIN_TOKEN, TOKEN_A, TestServer, admin_on, first_seq, id};
use reqwest::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, LOCATION, ORIGIN,
    REFERRER_POLICY, STRICT_TRANSPORT_SECURITY, WWW_AUTHENTICATE, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use todo_core::{AccessLevel, Grants};
use todo_server::auth::store::ShareTarget;
use todo_server::web::{CSP, Gate, HSTS};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

// ---- route inventory ------------------------------------------------------------------------------

/// Exactly these answer without a session or token. HEAD is allowed wherever GET is.
const PUBLIC: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("POST", "/api/v1/pair/start"),
    ("POST", "/api/v1/pair/poll"),
    ("POST", "/api/v1/pair/redeem"),
    ("GET", "/login"),
    ("GET", "/setup"),
    ("GET", "/pair"),
    ("GET", "/pair/invite"),
    ("POST", "/auth/invite/start"),
    ("POST", "/auth/invite/finish"),
    ("POST", "/auth/login/start"),
    ("POST", "/auth/login/finish"),
    ("POST", "/auth/setup/start"),
    ("POST", "/auth/setup/finish"),
    ("GET", "/assets/{file}"),
    ("GET", "/assets/fonts/{file}"),
    ("GET", "/s/{token}"),
    // Authenticated by an HMAC signature instead (see `email_ingest_needs_a_fresh_signature_and_an_unused_nonce`).
    ("POST", "/api/v1/ingest/email"),
];

/// The MCP endpoint is a service mounted for every method.
const ANY: &str = "*";

/// Every method the sweeps send: the usual ones, HEAD, OPTIONS, TRACE and two unknown ones.
const METHODS: &[&str] = &[
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE", "PROPFIND", "BREW",
];

/// Paths that match no route.
const UNKNOWN: &[&str] = &[
    "/nope",
    "/api/v1/nope",
    "/api/v2/lists",
    "/api",
    "/mcp/extra",
    "/admin/nope",
    "/s/",
    "/assets/",
    "/assets/nope.js",
    "/.env",
    "/health/",
    "/HEALTH",
];

const UUID: &str = "00000000-0000-4000-8000-000000000000";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Route {
    method: String,
    path: String,
    public: bool,
}

fn openapi() -> Value {
    serde_json::to_value(todo_server::api::openapi::spec()).unwrap()
}

/// The admin API's own document (its routes are only on the admin listener).
fn admin_openapi() -> Value {
    serde_json::to_value(todo_server::admin::openapi::spec()).unwrap()
}

/// An operation is open when its `security` is absent or lists the empty requirement
/// (`security(())`); protected operations name the bearer scheme.
fn is_open(op: &Value) -> bool {
    op.get("security")
        .and_then(Value::as_array)
        .is_none_or(|reqs| {
            reqs.iter()
                .any(|r| r.as_object().is_some_and(serde_json::Map::is_empty))
        })
}

/// Every route the server serves: `/health`, the web route table (which the web router is built
/// from), every operation in the OpenAPI document, the MCP endpoint and the optional desktop feed.
/// [`the_inventory_matches_the_router_source`] checks that nothing else is registered.
fn inventory() -> Vec<Route> {
    let mut v = vec![Route {
        method: "GET".into(),
        path: "/health".into(),
        public: true,
    }];
    for (m, p, g) in todo_server::web::routes() {
        v.push(Route {
            method: m.to_string(),
            path: p.to_owned(),
            public: g == Gate::Public,
        });
    }
    let doc = openapi();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if [
                "get", "put", "post", "delete", "options", "head", "patch", "trace",
            ]
            .contains(&method.as_str())
            {
                v.push(Route {
                    method: method.to_uppercase(),
                    path: path.clone(),
                    public: is_open(op),
                });
            }
        }
    }
    for (method, path) in todo_server::assistant::routes() {
        v.push(Route {
            method: method.to_string(),
            path: format!("/api/v1{path}"),
            public: false,
        });
    }
    v.push(Route {
        method: ANY.into(),
        path: todo_server::mcp::PATH.into(),
        public: false,
    });
    v.push(Route {
        method: "GET".into(),
        path: "/releases/{file}".into(),
        public: false,
    });
    v.sort();
    v
}

fn public_set() -> BTreeSet<(String, String)> {
    PUBLIC
        .iter()
        .map(|(m, p)| ((*m).to_owned(), (*p).to_owned()))
        .collect()
}

fn is_public(public: &BTreeSet<(String, String)>, method: &str, template: &str) -> bool {
    let has = |m: &str| public.contains(&(m.to_owned(), template.to_owned()));
    has(method) || (method == "HEAD" && has("GET"))
}

/// A request path for a route template.
fn concrete(path: &str) -> String {
    if path.starts_with("/api/v1/smart-lists/") {
        path.replace("{id}", "my_day")
    } else if path.starts_with("/api/") {
        path.replace("{id}", UUID)
            .replace("{attempt}", UUID)
            .replace("{seq}", "1")
    } else if path == "/releases/{file}" {
        "/releases/releases.win.json".into()
    } else {
        path.replace("{id}", "1")
            .replace("{file}", "app.css")
            .replace("{token}", "AAAAAAAAAAAAAAAAAAAAAA")
    }
}

/// Every `.route(`, `.route_service(`, `.nest(`, `.nest_service(` and `.fallback_service(` call in
/// the server's source: `(file, call, first argument)`.
fn registrations() -> BTreeSet<(String, String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeSet<(String, String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let file = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // Local auth/Responses stubs, included only under cfg(test).
            if file == "assistant/openai/tests.rs" {
                continue;
            }
            let code: String = std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for call in [
                "route",
                "route_service",
                "nest",
                "nest_service",
                "fallback_service",
            ] {
                let pat = format!(".{call}(");
                let mut rest = code.as_str();
                while let Some(i) = rest.find(&pat) {
                    rest = &rest[i + pat.len()..];
                    let end = rest.find([',', ')']).unwrap();
                    out.insert((file.clone(), call.to_owned(), rest[..end].trim().to_owned()));
                }
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = BTreeSet::new();
    walk(&root, &root, &mut out);
    out
}

#[test]
fn the_inventory_matches_the_router_source() {
    let regs = registrations();
    // Every API route is in the OpenAPI document, and the other way round.
    // `api/attachments.rs` registers the upload routes with their full paths (they are mounted outside
    // the global body limit).
    let api = |f: &str, c: &str| (f == "api/mod.rs" || f == "api/attachments.rs") && c == "route";
    let in_source: BTreeSet<String> = regs
        .iter()
        .filter(|(f, c, _)| api(f, c))
        .map(|(f, _, a)| {
            let a = a.trim_matches('"');
            if f == "api/mod.rs" {
                format!("/api/v1{a}")
            } else {
                a.to_owned()
            }
        })
        .collect();
    let documented: BTreeSet<String> = openapi()["paths"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(in_source, documented, "API routes vs the OpenAPI document");
    // The admin listener's routes are exactly its own document's, all under `/api/v1/admin/`.
    let admin = |f: &str, c: &str| f == "admin/mod.rs" && c == "route";
    let admin_in_source: BTreeSet<String> = regs
        .iter()
        .filter(|(f, c, _)| admin(f, c))
        .map(|(_, _, a)| a.trim_matches('"').to_owned())
        .collect();
    let admin_documented: BTreeSet<String> = admin_openapi()["paths"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        admin_in_source, admin_documented,
        "admin routes vs the admin OpenAPI document"
    );
    assert!(
        admin_in_source
            .iter()
            .all(|p| p.starts_with("/api/v1/admin/")),
        "{admin_in_source:?}"
    );
    assert!(
        documented.iter().all(|p| !p.starts_with("/api/v1/admin")),
        "the public document lists no admin route"
    );
    // Everything else is one of these. A new registration anywhere fails here: add it to
    // `inventory()` (and to `PUBLIC` if it is meant to be open).
    let others: BTreeSet<(String, String, String)> = regs
        .into_iter()
        .filter(|(f, c, _)| !api(f, c) && !admin(f, c))
        .collect();
    let expected: BTreeSet<(String, String, String)> = [
        ("app.rs", "route", "\"/health\""),
        ("app.rs", "nest", "\"/api/v1\""),
        // The assistant router, built from `todo_server::assistant::routes()`'s table.
        ("assistant/mod.rs", "route", "$path"),
        // The MCP service, at `todo_server::mcp::PATH`.
        ("mcp/mod.rs", "route_service", "PATH"),
        ("releases.rs", "route", "\"/releases/{file}\""),
        // The web router, built from `todo_server::web::routes()`.
        ("web/mod.rs", "route", "path"),
    ]
    .iter()
    .map(|(f, c, a)| ((*f).to_owned(), (*c).to_owned(), (*a).to_owned()))
    .collect();
    assert_eq!(others, expected);
}

#[test]
fn the_public_set_is_exactly_the_documented_one() {
    let from_inventory: BTreeSet<(String, String)> = inventory()
        .into_iter()
        .filter(|r| r.public)
        .map(|r| (r.method, r.path))
        .collect();
    assert_eq!(from_inventory, public_set());
    let declared: BTreeSet<(String, String)> =
        todo_server::app::public_routes().into_iter().collect();
    assert_eq!(declared, public_set());
    // Sanity: the inventory is the whole server, not just the open part.
    assert!(inventory().len() > 40, "{}", inventory().len());
}

// ---- anonymous access -----------------------------------------------------------------------------

/// A response to an anonymous request.
#[derive(Debug)]
struct Answer {
    status: StatusCode,
    location: String,
    headers: reqwest::header::HeaderMap,
    body: String,
}

/// How an anonymous request is dressed up.
#[derive(Clone, Copy, Debug)]
enum Creds {
    None,
    /// A well-formed agent token that doesn't exist.
    WrongToken,
    /// A made-up session cookie, with the right `Origin` and a CSRF header.
    BogusSession,
    /// The real admin API token (valid only on the admin listener).
    AdminToken,
}

async fn anonymous(
    env: &Env,
    client: &reqwest::Client,
    method: &str,
    path: &str,
    creds: Creds,
) -> Answer {
    let m = Method::from_bytes(method.as_bytes()).unwrap();
    let mut req = client.request(m, format!("{}{path}", env.t.url));
    match creds {
        Creds::None => {}
        Creds::WrongToken => {
            req = req.bearer_auth(format!("tda_{}", "A".repeat(43)));
        }
        Creds::BogusSession => {
            req = req
                .header(COOKIE, format!("__Host-todo_session={}", "B".repeat(43)))
                .header(ORIGIN, SITE)
                .header("x-csrf-token", "C".repeat(43));
        }
        Creds::AdminToken => {
            req = req.bearer_auth(ADMIN_TOKEN);
        }
    }
    if matches!(method, "POST" | "PUT" | "PATCH") {
        req = req.header(CONTENT_TYPE, "application/json").body("{}");
    }
    let res = req.send().await.unwrap();
    Answer {
        status: res.status(),
        location: res
            .headers()
            .get(LOCATION)
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default(),
        headers: res.headers().clone(),
        body: res.text().await.unwrap(),
    }
}

/// Refused: 401, 403, 404 or 405, or (for a page load) a redirect to sign-in.
fn refused(a: &Answer) -> bool {
    matches!(a.status.as_u16(), 401 | 403 | 404 | 405)
        || (a.status == StatusCode::SEE_OTHER && a.location.starts_with("/login?next="))
}

fn no_redirects() -> reqwest::Client {
    common::http()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Nothing in the headers or body names the server or its version.
fn assert_anonymous(what: &str, headers: &reqwest::header::HeaderMap, body: &str) {
    let version = env!("CARGO_PKG_VERSION");
    for (name, value) in headers {
        let n = name.as_str();
        assert!(
            ![
                "server",
                "x-powered-by",
                "x-version",
                "x-app-version",
                "via"
            ]
            .contains(&n),
            "{what}: header {n}"
        );
        let v = value.to_str().unwrap_or("");
        assert!(
            !v.contains(version) && !v.contains("todo-server"),
            "{what}: {n}: {v}"
        );
    }
    assert!(!body.contains(version), "{what}: version in body");
    assert!(!body.contains("todo-server"), "{what}: server name in body");
}

#[tokio::test]
async fn every_method_outside_the_public_set_is_refused_without_credentials() {
    // With the admin listener on: its token must open nothing here. Enable the private feed too.
    let releases = tempfile::tempdir().unwrap();
    std::fs::write(
        releases.path().join("releases.win.json"),
        r#"{"Assets":[]}"#,
    )
    .unwrap();
    let env = Env::start_with(|c| {
        admin_on(c);
        c.releases.dir = Some(releases.path().into());
    })
    .await;
    let client = no_redirects();
    let public = public_set();
    let mut targets: Vec<(String, String)> = inventory()
        .into_iter()
        .map(|r| (r.path.clone(), concrete(&r.path)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    targets.extend(UNKNOWN.iter().map(|p| ((*p).to_owned(), (*p).to_owned())));
    let mut checked = 0;
    for (template, path) in &targets {
        for method in METHODS {
            if is_public(&public, method, template) {
                continue;
            }
            for creds in [
                Creds::None,
                Creds::WrongToken,
                Creds::BogusSession,
                Creds::AdminToken,
            ] {
                let a = anonymous(&env, &client, method, path, creds).await;
                let what = format!("{method} {path} ({creds:?}): {} {}", a.status, a.body);
                assert!(refused(&a), "{what}");
                // The API and MCP turn every method away at authentication.
                let behind_tokens = (template.starts_with("/api/v1/")
                    && !template.starts_with("/api/v1/pair/")
                    && !template.starts_with("/api/v1/ingest/"))
                    || template == "/mcp";
                if behind_tokens {
                    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{what}");
                    assert_eq!(a.headers.get(WWW_AUTHENTICATE).unwrap(), "Bearer", "{what}");
                    if *method != "HEAD" {
                        let v: Value = serde_json::from_str(&a.body).unwrap();
                        assert_eq!(v["error"]["code"], "unauthorized", "{what}");
                    }
                }
                assert!(a.body.len() <= 512, "{what}");
                assert_anonymous(&what, &a.headers, &a.body);
                assert_eq!(
                    a.headers.get(X_CONTENT_TYPE_OPTIONS).unwrap(),
                    "nosniff",
                    "{what}"
                );
                assert_eq!(
                    a.headers.get(STRICT_TRANSPORT_SECURITY).unwrap(),
                    HSTS,
                    "{what}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 1500, "{checked}");
    // The SSE variant of the event endpoint too.
    let res = client
        .get(format!("{}/api/v1/events", env.t.url))
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    // None of it wrote anything, and no owner exists.
    assert_eq!(env.t.server.engine.head_seq(), 0);
    assert!(env.take_code().is_some(), "setup is still open");
}

#[tokio::test]
async fn public_routes_answer_without_credentials_and_reveal_nothing() {
    let env = Env::start().await;
    let anon = env.browser();
    for (method, path) in PUBLIC {
        let url = concrete(path);
        let p = match *method {
            "GET" => anon.get(&url).await,
            _ => {
                let body = if path.starts_with("/api/") {
                    json!({ "device_name": "Laptop", "poll_token": "x" })
                } else {
                    json!({})
                };
                let res = anon
                    .client
                    .post(format!("{}{url}", env.t.url))
                    .header(ORIGIN, SITE)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                Page {
                    status: res.status(),
                    headers: res.headers().clone(),
                    body: res.text().await.unwrap(),
                }
            }
        };
        let what = format!("{method} {path}: {} {}", p.status, p.body);
        assert_ne!(p.status, StatusCode::UNAUTHORIZED, "{what}");
        assert!(!p.location().starts_with("/login"), "{what}");
        assert_ne!(p.status, StatusCode::METHOD_NOT_ALLOWED, "{what}");
        assert!(!p.status.is_server_error(), "{what}");
        assert_anonymous(&what, &p.headers, &p.body);
        if *method == "GET" {
            let h = anon
                .send(Method::HEAD, &url, None, common::web::Body::None)
                .await;
            assert_eq!(h.status, p.status, "HEAD {path}");
        }
    }

    // `/health`: an empty 200 with only the standard headers.
    for method in [Method::GET, Method::HEAD] {
        let res = anon
            .client
            .request(method.clone(), format!("{}/health", env.t.url))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let names: BTreeSet<String> = res
            .headers()
            .keys()
            .map(|k| k.as_str().to_owned())
            .collect();
        let allowed: BTreeSet<String> = [
            "content-length",
            "date",
            "x-content-type-options",
            "referrer-policy",
            "cache-control",
            "strict-transport-security",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert!(names.is_subset(&allowed), "{method} /health: {names:?}");
        assert!(res.text().await.unwrap().is_empty(), "{method} /health");
    }

    // Unknown share tokens and assets: the same bare 404 whatever the guess.
    let a = anon.get("/s/AAAAAAAAAAAAAAAAAAAAAA").await;
    let b = anon.get("/s/BBBBBBBBBBBBBBBBBBBBBB").await;
    let c = anon.get("/s/short").await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert_eq!((a.status, &a.body), (b.status, &b.body));
    assert_eq!((a.status, &a.body), (c.status, &c.body));
    let x = anon.get("/assets/nope.js").await;
    assert_eq!(x.status, StatusCode::NOT_FOUND);
    assert!(x.body.is_empty());
    // A made-up pairing poll token learns nothing either.
    let poll = |t: &'static str| {
        let client = anon.client.clone();
        let url = format!("{}/api/v1/pair/poll", env.t.url);
        async move {
            let r = client
                .post(url)
                .json(&json!({ "poll_token": t }))
                .send()
                .await
                .unwrap();
            (r.status(), r.text().await.unwrap())
        }
    };
    let (s1, b1) = poll("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").await;
    let (s2, b2) = poll("x").await;
    assert!(s1.is_client_error(), "{s1}");
    assert_eq!((s1, b1), (s2, b2));

    // Once the owner exists, setup is gone for good.
    let _owner = env.owner().await;
    for (m, p) in [(Method::GET, "/setup"), (Method::POST, "/auth/setup/start")] {
        let r = anon
            .send(
                m.clone(),
                p,
                Some(SITE),
                common::web::Body::Json(json!({ "code": "AAAA-AAAA" })),
            )
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{m} {p}: {}", r.body);
    }
}

// ---- the admin API --------------------------------------------------------------------------------

#[tokio::test]
async fn the_admin_api_is_absent_on_the_public_listener() {
    let env = Env::start_with(admin_on).await;
    let client = no_redirects();
    let mut paths: Vec<String> = admin_openapi()["paths"]
        .as_object()
        .unwrap()
        .keys()
        .map(|p| {
            p.replace("{id}", "1")
                .replace("{address}", "you@example.com")
                .replace("{key}", "errands")
        })
        .collect();
    paths.extend(
        [
            "/api/v1/admin",
            "/api/v1/admin/",
            "/api/v1/admin/nope",
            "/api/v1/admin/agents/1/revoke/extra",
        ]
        .map(str::to_owned),
    );
    let mut checked = 0;
    for path in &paths {
        for method in METHODS {
            for creds in [
                Creds::None,
                Creds::WrongToken,
                Creds::BogusSession,
                Creds::AdminToken,
            ] {
                let a = anonymous(&env, &client, method, path, creds).await;
                let what = format!("{method} {path} ({creds:?}): {} {}", a.status, a.body);
                assert_eq!(a.status, StatusCode::NOT_FOUND, "{what}");
                if *method != "HEAD" {
                    let v: Value = serde_json::from_str(&a.body).unwrap();
                    assert_eq!(v["error"]["code"], "not_found", "{what}");
                }
                assert_anonymous(&what, &a.headers, &a.body);
                checked += 1;
            }
            // An owner's own device token doesn't reach them either.
            let m = Method::from_bytes(method.as_bytes()).unwrap();
            let res = client
                .request(m, format!("{}{path}", env.t.url))
                .bearer_auth(TOKEN_A)
                .send()
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::NOT_FOUND,
                "{method} {path} (owner)"
            );
        }
    }
    assert!(checked > 500, "{checked}");
    // Meanwhile the admin listener answers them.
    let res = client
        .get(format!(
            "{}/api/v1/admin/agents",
            env.t.server.admin_url().unwrap()
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(env.t.server.engine.head_seq(), 0);
}

// ---- token scope ----------------------------------------------------------------------------------

/// Everything the owner keeps from the agents contains this.
const ZZ: &str = "zzhidden";

/// An owner's data: one list granted to the agent under test, and one list (in its own collection,
/// with a tagged, important, dated task, a step and a note) that is hidden from it.
struct Scope {
    t: TestServer,
    granted: String,
    granted_task: String,
    /// A tag on the granted task, so agents that can read the granted list see it.
    granted_tag: String,
    granted_event: (u64, String),
    hidden: String,
    vault: String,
    hidden_task: String,
    hidden_step: String,
    hidden_tag: String,
    hidden_reminder: String,
    hidden_event: (u64, String),
}

fn event_of(result: &Value) -> (u64, String) {
    (
        first_seq(result),
        result["events"][0]["id"].as_str().unwrap().to_owned(),
    )
}

impl Scope {
    async fn new() -> Self {
        let t = TestServer::start().await;
        let vault = id();
        t.ok(json!({ "cmd": "create_collection", "id": vault, "name": "zzhiddencol" }))
            .await;
        let hidden = id();
        t.ok(json!({ "cmd": "create_list", "id": hidden, "name": "zzhiddenlist", "collection": vault }))
            .await;
        let granted = t.create_list("Granted").await;
        let hidden_task = t.create_task(&hidden, "zzhiddentask", json!({})).await;
        let hidden_step = id();
        t.ok(json!({ "cmd": "create_step", "id": hidden_step, "task": hidden_task, "title": "zzhiddenstep" }))
            .await;
        t.ok(json!({ "cmd": "add_note", "task": hidden_task, "text": "zzhiddennote" }))
            .await;
        let hidden_tag = id();
        t.ok(json!({ "cmd": "create_tag", "id": hidden_tag, "name": "zzhiddentag" }))
            .await;
        t.ok(json!({ "cmd": "add_tag", "task": hidden_task, "tag": hidden_tag }))
            .await;
        let hidden_reminder = id();
        t.ok(
            json!({ "cmd": "add_reminder", "id": hidden_reminder, "task": hidden_task,
                    "at": "2030-01-01T09:00:00Z" }),
        )
        .await;
        let r = t
            .ok(json!({ "cmd": "update_task", "id": hidden_task,
                        "patch": { "title": "zzhiddentask!", "priority": "high", "due": "2030-01-01" } }))
            .await;
        let hidden_event = event_of(&r);
        let granted_task = t
            .create_task(&granted, "Visible", json!({ "priority": "high" }))
            .await;
        let granted_tag = id();
        t.ok(json!({ "cmd": "create_tag", "id": granted_tag, "name": "Granted tag" }))
            .await;
        t.ok(json!({ "cmd": "add_tag", "task": granted_task, "tag": granted_tag }))
            .await;
        let r = t
            .ok(json!({ "cmd": "update_task", "id": granted_task, "patch": { "title": "Visible!" } }))
            .await;
        let granted_event = event_of(&r);
        Self {
            t,
            granted,
            granted_task,
            granted_tag,
            granted_event,
            hidden,
            vault,
            hidden_task,
            hidden_step,
            hidden_tag,
            hidden_reminder,
            hidden_event,
        }
    }

    fn hidden_ids(&self) -> [&str; 6] {
        [
            &self.hidden,
            &self.vault,
            &self.hidden_task,
            &self.hidden_step,
            &self.hidden_tag,
            &self.hidden_reminder,
        ]
    }

    /// Panics if `v` mentions anything hidden.
    fn assert_clean(&self, what: &str, v: &Value) {
        self.assert_clean_except(what, v, &[]);
    }

    /// Panics if `v` mentions anything hidden other than the IDs in `sent` (which the caller sent).
    fn assert_clean_except(&self, what: &str, v: &Value, sent: &[&str]) {
        let mut text = v.to_string();
        for s in sent {
            text = text.replace(s, "<sent>");
        }
        assert!(!text.contains(ZZ), "{what} leaks hidden content: {text}");
        for h in self.hidden_ids() {
            assert!(!text.contains(h), "{what} leaks the hidden ID {h}: {text}");
        }
    }

    /// A fresh task (with a step and a reminder) in the granted list, made by the owner:
    /// `(task, step, reminder)`.
    async fn fresh(&self) -> (String, String, String) {
        let task = self.t.create_task(&self.granted, "Fresh", json!({})).await;
        let step = id();
        self.t
            .ok(json!({ "cmd": "create_step", "id": step, "task": task, "title": "Fresh step" }))
            .await;
        let reminder = id();
        self.t
            .ok(json!({ "cmd": "add_reminder", "id": reminder, "task": task,
                        "at": "2030-01-01T09:00:00Z" }))
            .await;
        (task, step, reminder)
    }

    /// What the owner sees of the hidden list, to show that nothing an agent did changed it.
    async fn hidden_snapshot(&self) -> Value {
        let task = self
            .t
            .get_ok(&format!("/api/v1/tasks/{}", self.hidden_task))
            .await;
        let list = self
            .t
            .get_ok(&format!("/api/v1/lists/{}/tasks", self.hidden))
            .await;
        json!([task, list])
    }
}

/// Answers for a hidden ID and a missing one are the same, apart from the IDs themselves.
fn same_answer(
    hidden: &(StatusCode, Value),
    h_ids: &[&str],
    missing: &(StatusCode, Value),
    m_ids: &[&str],
) -> bool {
    let strip = |v: &Value, ids: &[&str]| {
        let mut s = v.to_string();
        for i in ids {
            s = s.replace(i, "<id>");
        }
        s
    };
    hidden.0 == missing.0 && strip(&hidden.1, h_ids) == strip(&missing.1, m_ids)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    Allowed,
    Forbidden,
    Missing,
}

/// What an agent at `level` on the granted list gets for something that needs `need` there.
fn expected(level: AccessLevel, need: AccessLevel) -> Outcome {
    if level >= need {
        Outcome::Allowed
    } else if level == AccessLevel::None {
        // A list the agent can't read is a list that doesn't exist.
        Outcome::Missing
    } else {
        Outcome::Forbidden
    }
}

/// The IDs a tool call is aimed at.
struct Ids {
    task: String,
    step: String,
    list: String,
    /// A second task in the granted list (merge source or target, move subject, the task a tag is
    /// put on or taken off when the tag is the target).
    other: String,
    tag: String,
    reminder: String,
}

/// One case per MCP tool that targets something (the read-only listing tools are checked for leaks).
const TARGETED: &[(&str, &str, AccessLevel)] = &[
    ("get_task", "task", AccessLevel::Read),
    ("create_task", "list", AccessLevel::Capture),
    ("add_note", "task", AccessLevel::Capture),
    ("add_step", "task", AccessLevel::Plan),
    ("update_task", "task", AccessLevel::Plan),
    ("complete_task", "task", AccessLevel::Plan),
    ("set_my_day", "task", AccessLevel::Plan),
    ("archive_task", "task", AccessLevel::Plan),
    ("move_task", "task", AccessLevel::Plan),
    ("move_task", "destination", AccessLevel::Plan),
    ("update_step", "step", AccessLevel::Plan),
    ("merge_tasks", "source", AccessLevel::Plan),
    ("merge_tasks", "target", AccessLevel::Plan),
    ("add_link", "task", AccessLevel::Capture),
    ("tag_task", "task", AccessLevel::Plan),
    ("tag_task", "tag", AccessLevel::Plan),
    ("untag_task", "task", AccessLevel::Plan),
    ("untag_task", "tag", AccessLevel::Plan),
    ("add_reminder", "task", AccessLevel::Plan),
    ("update_reminder", "reminder", AccessLevel::Plan),
];

fn tool_args(tool: &str, aim: &str, ids: &Ids, s: &Scope) -> Value {
    let granted = &s.granted;
    match (tool, aim) {
        ("get_task", _) => json!({ "task_id": ids.task }),
        ("create_task", _) => json!({ "title": "From the agent", "list_id": ids.list }),
        ("add_note", _) => json!({ "task_id": ids.task, "text": "A note" }),
        ("add_step", _) => json!({ "task_id": ids.task, "title": "A step" }),
        ("update_task", _) => json!({ "task_id": ids.task, "title": "Renamed" }),
        ("complete_task", _) => json!({ "task_id": ids.task }),
        ("set_my_day", _) => json!({ "task_id": ids.task, "date": "2030-01-01" }),
        ("archive_task", _) => json!({ "task_id": ids.task }),
        ("move_task", "task") => json!({ "task_id": ids.task, "list_id": granted }),
        ("move_task", _) => json!({ "task_id": ids.other, "list_id": ids.list }),
        ("update_step", _) => json!({ "step_id": ids.step, "title": "Renamed step" }),
        ("merge_tasks", "source") => {
            json!({ "source_task_id": ids.task, "target_task_id": ids.other })
        }
        ("merge_tasks", _) => json!({ "source_task_id": ids.other, "target_task_id": ids.task }),
        ("add_link", _) => json!({ "task_id": ids.task, "url": "https://todo.example.com/" }),
        ("tag_task" | "untag_task", "task") => {
            json!({ "task_id": ids.task, "tag_id": s.granted_tag })
        }
        ("tag_task" | "untag_task", _) => json!({ "task_id": ids.other, "tag_id": ids.tag }),
        ("add_reminder", _) => json!({ "task_id": ids.task, "at": "2030-01-02T09:00:00Z" }),
        ("update_reminder", _) => {
            json!({ "reminder_id": ids.reminder, "at": "2030-01-02T09:00:00Z" })
        }
        _ => unreachable!("{tool} {aim}"),
    }
}

/// A REST command aimed at `task` (or, for `create_task`, at `list`).
fn rest_command(name: &str, task: &str, list: &str) -> Value {
    match name {
        "create_task" => {
            json!({ "cmd": "create_task", "id": id(), "list": list, "title": "Agent" })
        }
        "add_note" => json!({ "cmd": "add_note", "task": task, "text": "Agent note" }),
        "update_task" => {
            json!({ "cmd": "update_task", "id": task, "patch": { "title": "Agent" } })
        }
        "complete_task" => json!({ "cmd": "complete_task", "id": task }),
        "delete" => json!({ "cmd": "delete", "id": { "kind": "task", "id": task } }),
        _ => unreachable!("{name}"),
    }
}

/// A tool call as `(status-like, body)`: success is 200 with the structured result, a tool error is
/// 400 with its error object (only compared with other answers, never checked for 400).
async fn tool(c: &Client, name: &str, a: Value) -> (StatusCode, Value) {
    let r = call(c, name, a).await;
    let v = r.structured_content.clone().unwrap_or(Value::Null);
    if r.is_error == Some(true) {
        (StatusCode::BAD_REQUEST, v["error"].clone())
    } else {
        (StatusCode::OK, v)
    }
}

async fn check_level(s: &Scope, level: AccessLevel) {
    let label = format!("{level:?}").to_lowercase();
    let mut grants = Grants::with_default(AccessLevel::None);
    grants
        .lists
        .insert(serde_json::from_value(json!(s.granted)).unwrap(), level);
    grants.lists.insert(
        serde_json::from_value(json!(s.hidden)).unwrap(),
        AccessLevel::None,
    );
    grants.collections.insert(
        serde_json::from_value(json!(s.vault)).unwrap(),
        AccessLevel::None,
    );
    let (_, token) = agent(&s.t, &format!("agent-{label}"), &grants, None);
    let can_read = level >= AccessLevel::Read;
    let rest = |method: Method, path: String, body: Option<Value>| {
        let token = token.clone();
        async move {
            let (st, _, v) = s.t.request(method, &path, Some(&token), body).await;
            (st, v)
        }
    };

    // ---- REST reads: collections, lists, tags, smart lists, queries, activity, the event log.
    let mut read_paths: Vec<String> = [
        "/api/v1/lists",
        "/api/v1/collections",
        "/api/v1/tags",
        "/api/v1/activity",
        "/api/v1/events?since=0",
        "/api/v1/tasks?query=",
        "/api/v1/tasks?query=zzhiddentask",
        "/api/v1/tasks?query=important",
        "/api/v1/smart-lists",
        "/api/v1/openapi.json",
    ]
    .iter()
    .map(|p| (*p).to_owned())
    .collect();
    let (st, smart) = rest(Method::GET, "/api/v1/smart-lists".into(), None).await;
    assert_eq!(st, StatusCode::OK, "{label}: {smart}");
    for sl in smart.as_array().unwrap() {
        read_paths.push(format!(
            "/api/v1/smart-lists/{}/tasks",
            sl["id"].as_str().unwrap()
        ));
    }
    for p in &read_paths {
        let (st, v) = rest(Method::GET, p.clone(), None).await;
        assert_eq!(st, StatusCode::OK, "{label} GET {p}: {v}");
        if !p.ends_with("openapi.json") {
            s.assert_clean(&format!("{label} GET {p}"), &v);
        }
    }
    let (_, lists) = rest(Method::GET, "/api/v1/lists".into(), None).await;
    let names: Vec<&str> = lists["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        if can_read { vec!["Granted"] } else { vec![] },
        "{label}"
    );
    let (_, tasks) = rest(Method::GET, "/api/v1/tasks?query=".into(), None).await;
    assert_eq!(
        tasks["items"].as_array().unwrap().is_empty(),
        !can_read,
        "{label}: {tasks}"
    );
    // Naming a hidden list in a query is like naming one that doesn't exist.
    let h = rest(
        Method::GET,
        "/api/v1/tasks?query=list%3Azzhiddenlist".into(),
        None,
    )
    .await;
    let m = rest(
        Method::GET,
        "/api/v1/tasks?query=list%3Azznosuchlist".into(),
        None,
    )
    .await;
    assert!(
        same_answer(&h, &["zzhiddenlist"], &m, &["zznosuchlist"]),
        "{label}: {h:?} vs {m:?}"
    );

    // ---- REST detail reads: hidden is missing; granted is readable from `read` up.
    let missing = id();
    for (hidden, granted, path) in [
        (&s.hidden, &s.granted, "/api/v1/lists/{}/tasks"),
        (&s.hidden_task, &s.granted_task, "/api/v1/tasks/{}"),
    ] {
        let h = rest(Method::GET, path.replace("{}", hidden), None).await;
        let m = rest(Method::GET, path.replace("{}", &missing), None).await;
        assert_eq!(h.0, StatusCode::NOT_FOUND, "{label} {path}");
        assert!(
            same_answer(&h, &[hidden], &m, &[&missing]),
            "{label}: {h:?} vs {m:?}"
        );
        let g = rest(Method::GET, path.replace("{}", granted), None).await;
        if can_read {
            assert_eq!(g.0, StatusCode::OK, "{label} {path}");
            s.assert_clean(&format!("{label} {path}"), &g.1);
        } else {
            assert!(
                same_answer(&g, &[granted], &m, &[&missing]),
                "{label}: {g:?} vs {m:?}"
            );
        }
    }

    // ---- The event stream: the replay and live events leave out the hidden list.
    let mut sse = s.t.sse(&token, Some(0)).await;
    while let Some(f) = sse.next_event(Duration::from_millis(500)).await {
        s.assert_clean(&format!("{label} replayed event"), &f.json());
        assert!(can_read, "{label} got an event: {}", f.data);
    }
    s.t.ok(json!({ "cmd": "add_note", "task": s.hidden_task, "text": "zzhiddenlive" }))
        .await;
    s.t.ok(json!({ "cmd": "add_note", "task": s.granted_task, "text": "Visible live" }))
        .await;
    let live = sse
        .next_event(Duration::from_secs(if can_read { 5 } else { 1 }))
        .await;
    if can_read {
        let f = live.expect("the granted list's event");
        assert!(f.data.contains("Visible live"), "{label}: {}", f.data);
        s.assert_clean(&format!("{label} live event"), &f.json());
    } else {
        assert!(live.is_none(), "{label} got {:?}", live.map(|f| f.data));
    }
    drop(sse);
    // From here on the owner writes nothing to the hidden list; the agent must not either.
    let before = s.hidden_snapshot().await;

    // ---- REST commands.
    let cmd = |c: Value| json!({ "command_id": id(), "command": c });
    let (task, _, _) = s.fresh().await;
    let rest_cases = [
        ("create_task", AccessLevel::Capture),
        ("add_note", AccessLevel::Capture),
        ("update_task", AccessLevel::Plan),
        ("complete_task", AccessLevel::Plan),
        ("delete", AccessLevel::Plan),
    ];
    for (name, need) in &rest_cases {
        let build = |task: &str, list: &str| rest_command(name, task, list);
        let what = format!("{label} REST {name}");
        let h = rest(
            Method::POST,
            "/api/v1/commands".into(),
            Some(cmd(build(&s.hidden_task, &s.hidden))),
        )
        .await;
        let m = rest(
            Method::POST,
            "/api/v1/commands".into(),
            Some(cmd(build(&missing, &missing))),
        )
        .await;
        assert_eq!(h.0, StatusCode::NOT_FOUND, "{what} on hidden: {}", h.1);
        assert!(
            same_answer(&h, &[&s.hidden_task, &s.hidden], &m, &[&missing]),
            "{what}: {h:?} vs {m:?}"
        );
        let g = rest(
            Method::POST,
            "/api/v1/commands".into(),
            Some(cmd(build(&task, &s.granted))),
        )
        .await;
        match expected(level, *need) {
            Outcome::Allowed => assert_eq!(g.0, StatusCode::OK, "{what}: {}", g.1),
            Outcome::Forbidden => {
                assert_eq!(g.0, StatusCode::FORBIDDEN, "{what}: {}", g.1);
                assert_eq!(g.1["error"]["code"], "forbidden", "{what}");
            }
            Outcome::Missing => assert!(
                same_answer(&g, &[&task, &s.granted], &m, &[&missing]),
                "{what}: {g:?} vs {m:?}"
            ),
        }
        s.assert_clean(&what, &g.1);
    }
    // Structure is the owner's: no agent creates lists or collections, whatever its level.
    for c in [
        json!({ "cmd": "create_list", "id": id(), "name": "Agent list" }),
        json!({ "cmd": "create_collection", "id": id(), "name": "Agent collection" }),
    ] {
        let r = rest(
            Method::POST,
            "/api/v1/commands".into(),
            Some(cmd(c.clone())),
        )
        .await;
        assert!(
            matches!(r.0, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{label} {c}: {:?}",
            r
        );
    }

    // ---- REST undo: a hidden event is a missing one; someone else's visible event is refused.
    let undo = |seq: u64, event: &str| {
        (
            format!("/api/v1/events/{seq}/undo"),
            json!({ "command_id": id(), "expected_event_id": event }),
        )
    };
    let (hp, hb) = undo(s.hidden_event.0, &s.hidden_event.1);
    let h = rest(Method::POST, hp, Some(hb)).await;
    let missing_event = id();
    let (mp, mb) = undo(9_999_999, &missing_event);
    let m = rest(Method::POST, mp, Some(mb)).await;
    assert_eq!(h.0, StatusCode::NOT_FOUND, "{label}: {}", h.1);
    assert!(
        same_answer(
            &h,
            &[&s.hidden_event.0.to_string(), &s.hidden_event.1],
            &m,
            &["9999999", &missing_event]
        ),
        "{label}: {h:?} vs {m:?}"
    );
    let (gp, gb) = undo(s.granted_event.0, &s.granted_event.1);
    let g = rest(Method::POST, gp, Some(gb)).await;
    if can_read {
        assert_eq!(g.1["error"]["code"], "not_own_event", "{label}: {}", g.1);
    } else {
        assert!(
            same_answer(
                &g,
                &[&s.granted_event.0.to_string(), &s.granted_event.1],
                &m,
                &["9999999", &missing_event]
            ),
            "{label}: {g:?} vs {m:?}"
        );
    }

    // ---- MCP: every tool.
    let c = connect(&s.t, &token).await;
    let mut exercised: BTreeSet<&str> = BTreeSet::new();
    for tool_name in [
        "list_lists",
        "list_collections",
        "list_tags",
        "get_activity",
    ] {
        let v = ok(&c, tool_name, json!({})).await;
        s.assert_clean(&format!("{label} {tool_name}"), &v);
        exercised.insert(tool_name);
    }
    for q in ["", "zzhiddentask", "important"] {
        let v = ok(&c, "query_tasks", json!({ "query": q })).await;
        s.assert_clean(&format!("{label} query_tasks {q}"), &v);
    }
    exercised.insert("query_tasks");
    for (tool_name, aim, need) in TARGETED {
        exercised.insert(tool_name);
        let what = format!("{label} {tool_name} ({aim})");
        let (other, _, _) = s.fresh().await;
        let hidden_ids = Ids {
            task: s.hidden_task.clone(),
            step: s.hidden_step.clone(),
            list: s.hidden.clone(),
            other: other.clone(),
            tag: s.hidden_tag.clone(),
            reminder: s.hidden_reminder.clone(),
        };
        let (m1, m2, m3, m4, m5) = (id(), id(), id(), id(), id());
        let missing_ids = Ids {
            task: m1.clone(),
            step: m2.clone(),
            list: m3.clone(),
            other: other.clone(),
            tag: m4.clone(),
            reminder: m5.clone(),
        };
        let h_sent = [
            &s.hidden_task,
            &s.hidden_step,
            &s.hidden,
            &s.hidden_tag,
            &s.hidden_reminder,
        ]
        .map(String::as_str);
        let m_sent = [&m1, &m2, &m3, &m4, &m5].map(String::as_str);
        let h = tool(&c, tool_name, tool_args(tool_name, aim, &hidden_ids, s)).await;
        let m = tool(&c, tool_name, tool_args(tool_name, aim, &missing_ids, s)).await;
        assert_ne!(h.0, StatusCode::OK, "{what} on hidden: {}", h.1);
        assert!(
            same_answer(&h, &h_sent, &m, &m_sent),
            "{what}: {h:?} vs {m:?}"
        );
        // The error may quote the IDs the agent sent itself, but nothing else that is hidden.
        s.assert_clean_except(&what, &h.1, &h_sent);
        let (task, step, reminder) = s.fresh().await;
        let (other2, _, _) = s.fresh().await;
        let granted_ids = Ids {
            task: task.clone(),
            step: step.clone(),
            list: s.granted.clone(),
            other: other2.clone(),
            tag: s.granted_tag.clone(),
            reminder: reminder.clone(),
        };
        let g = tool(&c, tool_name, tool_args(tool_name, aim, &granted_ids, s)).await;
        match expected(level, *need) {
            Outcome::Allowed => assert_eq!(g.0, StatusCode::OK, "{what}: {}", g.1),
            Outcome::Forbidden => assert_eq!(g.1["code"], "forbidden", "{what}: {}", g.1),
            Outcome::Missing => {
                let mg = Ids {
                    task: m1.clone(),
                    step: m2.clone(),
                    list: m3.clone(),
                    other: m1.clone(),
                    tag: m4.clone(),
                    reminder: m5.clone(),
                };
                let m = tool(&c, tool_name, tool_args(tool_name, aim, &mg, s)).await;
                assert_ne!(g.0, StatusCode::OK, "{what}: {}", g.1);
                assert!(
                    same_answer(
                        &g,
                        &[&task, &step, &s.granted, &other2, &s.granted_tag, &reminder],
                        &m,
                        &m_sent
                    ),
                    "{what}: {g:?} vs {m:?}"
                );
            }
        }
        s.assert_clean(&what, &g.1);
    }
    // create_tag: plan level on some list *and* read by default, which these agents (default none)
    // lack. So it's refused before the name is checked: a hidden tag's name and a free one get the
    // same answer, over MCP and REST alike.
    exercised.insert("create_tag");
    let needed = if level == AccessLevel::Plan {
        "read"
    } else {
        "plan"
    };
    let (taken, free) = ("zzhiddentag", "Agent tag");
    let ht = tool(&c, "create_tag", json!({ "name": taken })).await;
    let mt = tool(&c, "create_tag", json!({ "name": free })).await;
    assert_eq!(ht.1["code"], "forbidden", "{label} create_tag: {}", ht.1);
    assert_eq!(
        ht.1["details"]["needed"], needed,
        "{label} create_tag: {}",
        ht.1
    );
    assert_eq!(ht, mt, "{label} create_tag");
    s.assert_clean(&format!("{label} create_tag"), &ht.1);
    let tag_cmd = |name: &str| json!({ "command_id": id(), "command": { "cmd": "create_tag", "id": id(), "name": name } });
    let hr = rest(
        Method::POST,
        "/api/v1/commands".into(),
        Some(tag_cmd(taken)),
    )
    .await;
    let mr = rest(Method::POST, "/api/v1/commands".into(), Some(tag_cmd(free))).await;
    assert_eq!(
        hr.0,
        StatusCode::FORBIDDEN,
        "{label} REST create_tag: {}",
        hr.1
    );
    assert_eq!(
        hr.1["error"]["details"]["needed"], needed,
        "{label}: {}",
        hr.1
    );
    assert_eq!(hr, mr, "{label} REST create_tag");
    s.assert_clean(&format!("{label} REST create_tag"), &hr.1);
    // undo_event: hidden and missing events answer alike; someone else's visible event is refused;
    // the agent's own write can be undone.
    exercised.insert("undo_event");
    let h = fails(
        &c,
        "undo_event",
        json!({ "seq": s.hidden_event.0, "event_id": s.hidden_event.1 }),
    )
    .await;
    let m = fails(
        &c,
        "undo_event",
        json!({ "seq": 9_999_999, "event_id": missing_event }),
    )
    .await;
    assert_eq!(h["code"], "event_not_found", "{label}: {h}");
    assert!(
        same_answer(
            &(StatusCode::OK, h.clone()),
            &[&s.hidden_event.0.to_string(), &s.hidden_event.1],
            &(StatusCode::OK, m.clone()),
            &["9999999", &missing_event]
        ),
        "{label}: {h} vs {m}"
    );
    let g = fails(
        &c,
        "undo_event",
        json!({ "seq": s.granted_event.0, "event_id": s.granted_event.1 }),
    )
    .await;
    if can_read {
        assert_eq!(g["code"], "not_own_event", "{label}: {g}");
    } else {
        assert_eq!(g["code"], "event_not_found", "{label}: {g}");
    }
    if level >= AccessLevel::Capture {
        let r = ok(
            &c,
            "create_task",
            json!({ "title": "Undo me", "list_id": s.granted }),
        )
        .await;
        let e = &r["events"][0];
        ok(
            &c,
            "undo_event",
            json!({ "seq": e["seq"], "event_id": e["id"] }),
        )
        .await;
    }
    let all: BTreeSet<&str> = ALL_TOOLS.into_iter().collect();
    assert_eq!(exercised, all, "every MCP tool is in the matrix");
    c.cancel().await.unwrap();

    // Nothing an agent did touched the hidden list.
    assert_eq!(s.hidden_snapshot().await, before, "{label}");
}

#[tokio::test]
async fn an_agent_at_none_sees_and_changes_nothing() {
    let s = Scope::new().await;
    check_level(&s, AccessLevel::None).await;
}

#[tokio::test]
async fn an_agent_at_read_reads_its_list_and_nothing_else() {
    let s = Scope::new().await;
    check_level(&s, AccessLevel::Read).await;
}

#[tokio::test]
async fn an_agent_at_capture_captures_into_its_list_and_nothing_else() {
    let s = Scope::new().await;
    check_level(&s, AccessLevel::Capture).await;
}

#[tokio::test]
async fn an_agent_at_plan_plans_its_list_and_nothing_else() {
    let s = Scope::new().await;
    check_level(&s, AccessLevel::Plan).await;
}

// ---- email ingest ---------------------------------------------------------------------------------

/// `POST /api/v1/ingest/email` is the one public route that writes. Only a request signed with the
/// Worker's secret, within the timestamp window and with a nonce never seen before gets in; anything
/// else is a content-free 401 and stores nothing. (Order of the checks, races, limits and the
/// allowlist: `tests/ingest.rs`.)
#[tokio::test]
async fn email_ingest_needs_a_fresh_signature_and_an_unused_nonce() {
    let t = ingest::server().await;
    ingest::make_inbox(&t).await;
    let head = t.server.engine.head_seq();
    let mid = "<security-mid@mail.example.org>";
    let body = serde_json::to_vec(&ingest::payload(mid)).unwrap();
    let now = ingest::now_secs();
    let window = todo_server::ingest::WINDOW_SECS;
    let refused = |what: &str, (s, r): (StatusCode, Value)| {
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{what}: {r}");
        assert_eq!(r["error"]["code"], "unauthorized", "{what}: {r}");
        let text = r.to_string();
        assert!(
            !text.contains("security-mid") && !text.contains(ingest::OWNER),
            "{what}: the error echoes the request: {text}"
        );
    };
    let headers = |ts: i64, nonce: &str, sig: &str| {
        vec![
            ("x-todo-timestamp".to_owned(), ts.to_string()),
            ("x-todo-nonce".to_owned(), nonce.to_owned()),
            ("x-todo-signature".to_owned(), sig.to_owned()),
        ]
    };
    let post = |h: Vec<(String, String)>, body: Vec<u8>| {
        let t = &t;
        async move {
            let h: Vec<(&str, &str)> = h.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            ingest::post_raw(t, &h, body).await
        }
    };

    // No signature at all, and a valid owner token in place of one.
    refused("unsigned", post(vec![], body.clone()).await);
    let res = t
        .client
        .post(format!("{}{}", t.url, ingest::PATH))
        .bearer_auth(TOKEN_A)
        .header("content-type", "application/json")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "a bearer token");
    // A bad signature: another secret, a body changed after signing, a signature for another nonce.
    let n = ingest::nonce();
    let wrong_key = ingest::sign(
        "another-secret-0123456789abcdef0123",
        &now.to_string(),
        &n,
        &body,
    );
    refused(
        "another secret",
        post(headers(now, &n, &wrong_key), body.clone()).await,
    );
    let good = ingest::sign(ingest::SECRET, &now.to_string(), &n, &body);
    let mut tampered = body.clone();
    let at = tampered.len() - 2;
    tampered[at] = b' ';
    refused(
        "tampered body",
        post(headers(now, &n, &good), tampered).await,
    );
    refused(
        "signature for another nonce",
        post(headers(now, &ingest::nonce(), &good), body.clone()).await,
    );
    // Correctly signed, but stale or from the future.
    for ts in [now - window - 5, now + window + 5] {
        let n = ingest::nonce();
        let sig = ingest::sign(ingest::SECRET, &ts.to_string(), &n, &body);
        refused(
            &format!("timestamp {ts}"),
            post(headers(ts, &n, &sig), body.clone()).await,
        );
    }
    assert_eq!(t.server.engine.head_seq(), head, "nothing was stored");

    // The genuine request gets in once. Replaying it, or reusing its nonce for another (correctly
    // signed) email, is refused.
    let (s, r) = post(headers(now, &n, &good), body.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    let after = t.server.engine.head_seq();
    refused("replay", post(headers(now, &n, &good), body.clone()).await);
    let other = serde_json::to_vec(&ingest::payload("<another@mail.example.org>")).unwrap();
    let sig = ingest::sign(ingest::SECRET, &now.to_string(), &n, &other);
    refused("reused nonce", post(headers(now, &n, &sig), other).await);
    assert_eq!(t.server.engine.head_seq(), after);
    assert_eq!(ingest::live_tasks(&t).len(), 1);
}

// ---- request limits -------------------------------------------------------------------------------

#[tokio::test]
async fn oversized_bodies_are_rejected_on_every_route() {
    const LIMIT: usize = 4096;
    let releases = tempfile::tempdir().unwrap();
    let env = Env::start_with(|c| {
        c.limits.max_body_bytes = LIMIT;
        c.releases.dir = Some(releases.path().into());
    })
    .await;
    let addr = env.t.server.addr;
    let bearer = format!("Bearer {TOKEN_A}");
    let mut targets: Vec<(String, String)> = Vec::new();
    for r in inventory() {
        let methods: Vec<String> = if r.method == ANY {
            vec!["POST".into(), "GET".into(), "DELETE".into()]
        } else {
            vec![r.method.clone()]
        };
        for m in methods {
            targets.push((m, concrete(&r.path)));
        }
    }
    targets.extend(UNKNOWN.iter().map(|p| ("POST".to_owned(), (*p).to_owned())));
    for (method, path) in &targets {
        // Uploads (and filling in imported content) have their own, larger cap
        // (`attachments.max_upload_bytes`), checked the same way.
        let upload = (method == "POST" && path.ends_with("/attachments"))
            || (method == "PUT" && path.ends_with("/content"));
        let limit = if upload {
            usize::try_from(todo_server::blobs::MAX_UPLOAD_BYTES).unwrap()
        } else {
            LIMIT
        };
        for extra in [
            vec![],
            vec![("Authorization", bearer.as_str()), ("Origin", SITE)],
        ] {
            let r = raw::oversized(addr, method, path, limit + 1, &extra).await;
            let what = format!("{method} {path} {extra:?}: {} {}", r.status, r.body);
            assert_eq!(r.status, 413, "{what}");
            // A response to HEAD never has a body to read the code from.
            if method != "HEAD" {
                let v: Value = serde_json::from_str(r.body.trim()).unwrap_or(Value::Null);
                assert_eq!(v["error"]["code"], "payload_too_large", "{what}");
            }
        }
    }
    // Without a Content-Length to check up front, the body is cut off as it is read.
    for (path, extra) in [
        ("/api/v1/commands", vec![("Authorization", bearer.as_str())]),
        (
            "/api/v1/events/1/undo",
            vec![("Authorization", bearer.as_str())],
        ),
        (
            "/mcp",
            vec![
                ("Authorization", bearer.as_str()),
                ("Accept", "application/json, text/event-stream"),
            ],
        ),
        ("/api/v1/pair/start", vec![]),
        ("/api/v1/pair/poll", vec![]),
        ("/auth/setup/start", vec![("Origin", SITE)]),
        ("/auth/setup/finish", vec![("Origin", SITE)]),
    ] {
        let r = raw::chunked(addr, "POST", path, 2 * LIMIT, &extra).await;
        assert_eq!(r.status, 413, "chunked POST {path}: {}", r.body);
    }
    // The sign-in ceremony reads a body only once there is an owner (and `login/start` never does).
    let _owner = env.owner().await;
    let r = raw::chunked(
        addr,
        "POST",
        "/auth/login/finish",
        2 * LIMIT,
        &[("Origin", SITE)],
    )
    .await;
    assert_eq!(r.status, 413, "chunked POST /auth/login/finish: {}", r.body);
    // Under the limit is fine.
    let list = id();
    let r = env
        .command(
            TOKEN_A,
            json!({ "cmd": "create_list", "id": list, "name": "x".repeat(200) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
}

#[tokio::test]
async fn a_client_that_sends_headers_slowly_is_dropped() {
    let t = TestServer::start_with(|c| c.limits.header_read_timeout_ms = 300).await;
    let mut sock = TcpStream::connect(t.server.addr).await.unwrap();
    // Half a request head, then nothing.
    sock.write_all(b"GET /health HTTP/1.1\r\nHost: loc")
        .await
        .unwrap();
    let started = Instant::now();
    let got = read_until_closed(&mut sock, Duration::from_secs(5)).await;
    assert!(started.elapsed() >= Duration::from_millis(200));
    let text = String::from_utf8_lossy(&got);
    assert!(
        text.is_empty() || text.starts_with("HTTP/1.1 408"),
        "unexpected answer: {text}"
    );
    // A head sent a byte at a time is held to the same deadline as a whole.
    let mut sock = TcpStream::connect(t.server.addr).await.unwrap();
    let started = Instant::now();
    for b in b"GET /health HTTP/1.1\r\nHost: localhost\r\nX-Padding: aaaaaaaaaaaaaaaaaaaaaaaa\r\n" {
        if sock.write_all(&[*b]).await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    read_until_closed(&mut sock, Duration::from_secs(5)).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    // An idle keep-alive connection is closed by the same timer.
    let mut sock = TcpStream::connect(t.server.addr).await.unwrap();
    assert_eq!(health(&mut sock).await, "HTTP/1.1 200 OK");
    read_until_closed(&mut sock, Duration::from_secs(5)).await;
    // A prompt client is served.
    let mut sock = TcpStream::connect(t.server.addr).await.unwrap();
    assert_eq!(health(&mut sock).await, "HTTP/1.1 200 OK");
}

#[tokio::test]
async fn connections_beyond_the_cap_are_closed_until_a_slot_frees() {
    let t = TestServer::start_with(|c| c.limits.max_connections = 2).await;
    let mut a = TcpStream::connect(t.server.addr).await.unwrap();
    assert_eq!(health(&mut a).await, "HTTP/1.1 200 OK");
    let mut b = TcpStream::connect(t.server.addr).await.unwrap();
    assert_eq!(health(&mut b).await, "HTTP/1.1 200 OK");
    // The third is accepted by the kernel and closed by the server without an answer.
    let mut c = TcpStream::connect(t.server.addr).await.unwrap();
    let _ = c
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await;
    assert!(
        read_until_closed(&mut c, Duration::from_secs(5))
            .await
            .is_empty()
    );
    // Closing one frees its slot.
    drop(a);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut d = TcpStream::connect(t.server.addr).await.unwrap();
        let _ = d
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await;
        let mut buf = vec![0u8; 1024];
        if let Ok(Ok(n)) = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::io::AsyncReadExt::read(&mut d, &mut buf),
        )
        .await
            && n > 0
        {
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
            break;
        }
        assert!(Instant::now() < deadline, "the slot was not freed");
    }
    assert_eq!(health(&mut b).await, "HTTP/1.1 200 OK");
}

// ---- headers and cookies --------------------------------------------------------------------------

fn assert_html_headers(what: &str, p: &Page) {
    assert!(
        p.header(CONTENT_TYPE).starts_with("text/html"),
        "{what}: {}",
        p.header(CONTENT_TYPE)
    );
    let csp = p.header(CONTENT_SECURITY_POLICY);
    for part in [
        "default-src",
        "frame-ancestors 'none'",
        "base-uri 'none'",
        "form-action",
    ] {
        assert!(csp.contains(part), "{what}: {part} missing in {csp}");
    }
    assert!(
        csp.contains("object-src 'none'") || csp.contains("default-src 'none'"),
        "{what}: plugins allowed by {csp}"
    );
    // Quoted: the web app's `'wasm-unsafe-eval'` only lets it compile its wasm module, which is not
    // JavaScript `eval`.
    assert!(
        !csp.contains("'unsafe-inline'") && !csp.contains("'unsafe-eval'"),
        "{what}: {csp}"
    );
    assert_eq!(p.header(X_FRAME_OPTIONS), "DENY", "{what}");
    assert_eq!(p.header(X_CONTENT_TYPE_OPTIONS), "nosniff", "{what}");
    assert_eq!(p.header(REFERRER_POLICY), "same-origin", "{what}");
    assert_eq!(p.header(CACHE_CONTROL), "no-store, no-transform", "{what}");
    assert_eq!(p.header(STRICT_TRANSPORT_SECURITY), HSTS, "{what}");
    // No external resources.
    for bad in ["http://", "https://", "//"] {
        assert!(
            !p.body.contains(&format!("src=\"{bad}")) && !p.body.contains(&format!("href=\"{bad}")),
            "{what} loads {bad}"
        );
    }
}

#[tokio::test]
async fn html_pages_carry_strict_security_headers() {
    // A web app build, so `/app` is served too.
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(
        dist.path().join("index.html"),
        "<!doctype html><head><!--todo-session--></head>",
    )
    .unwrap();
    let web_dir = dist.path().to_owned();
    let env = Env::start_with(move |cfg| cfg.web.dir = Some(web_dir)).await;
    let anon = env.browser();
    let mut pages = Vec::new();
    for path in [
        "/login",
        "/setup",
        "/pair",
        "/pair?code=ABCD-EFGH",
        "/pair/invite",
    ] {
        pages.push((path.to_owned(), anon.get(path).await));
    }
    let owner = env.owner().await;
    let (agent_row, _) = owner.create_agent("scout", "none", "").await;
    let list = id();
    env.command(
        TOKEN_A,
        json!({ "cmd": "create_list", "id": list, "name": "Shared" }),
    )
    .await;
    let share = env
        .t
        .server
        .authsvc
        .create_share(
            ShareTarget::List(serde_json::from_value(json!(list)).unwrap()),
            None,
        )
        .unwrap();
    for path in [
        "/admin".to_owned(),
        "/admin/devices".to_owned(),
        "/admin/agents".to_owned(),
        format!("/admin/agents/{agent_row}"),
        "/admin/shares".to_owned(),
        "/admin/ingest".to_owned(),
        "/admin/audit".to_owned(),
        "/auth/verify".to_owned(),
        "/pair".to_owned(),
        "/app".to_owned(),
        "/app/list/x".to_owned(),
    ] {
        let p = owner.get(&path).await;
        pages.push((format!("{path} (owner)"), p));
    }
    pages.push(("/s/<token>".into(), anon.get(&format!("/s/{share}")).await));
    // Every GET page in the route table is in this list.
    let html_routes: BTreeSet<&str> = todo_server::web::routes()
        .into_iter()
        // `/auth/session` answers JSON (the app's CSRF token), not a page: `web_app.rs` covers it.
        .filter(|(m, p, _)| {
            *m == Method::GET && !p.starts_with("/assets/") && *p != "/" && *p != "/auth/session"
        })
        .map(|(_, p, _)| p)
        .collect();
    for r in &html_routes {
        let prefix = r.split('{').next().unwrap();
        assert!(
            pages.iter().any(|(p, _)| p.starts_with(prefix)),
            "GET {r} is not checked"
        );
    }
    for (path, p) in &pages {
        assert_eq!(p.status, StatusCode::OK, "{path}: {}", p.body);
        assert_html_headers(path, p);
    }
    // Admin pages use the shared CSP; share pages a stricter one of their own.
    assert_eq!(pages[0].1.header(CONTENT_SECURITY_POLICY), CSP);
    // Static assets: typed and nosniff, so they can't be read as anything else.
    for (path, ty) in [
        ("/assets/auth.js", "text/javascript"),
        ("/assets/app.css", "text/css"),
        ("/assets/admin.css", "text/css"),
        ("/assets/tokens.css", "text/css"),
        ("/assets/fonts/Geist-400.ttf", "font/ttf"),
        ("/assets/fonts/GeistMono-400.ttf", "font/ttf"),
    ] {
        let p = anon.get(path).await;
        assert_eq!(p.status, StatusCode::OK);
        assert!(p.header(CONTENT_TYPE).starts_with(ty), "{path}");
        assert_eq!(p.header(X_CONTENT_TYPE_OPTIONS), "nosniff", "{path}");
        assert_eq!(p.header(STRICT_TRANSPORT_SECURITY), HSTS, "{path}");
    }
    // HSTS covers API JSON and event streams too.
    let r = env.api(Method::GET, "/api/v1/lists", TOKEN_A, None).await;
    assert_eq!(r.headers.get(STRICT_TRANSPORT_SECURITY).unwrap(), HSTS);
    let stream = env
        .t
        .client
        .get(format!("{}/api/v1/events", env.t.url))
        .bearer_auth(TOKEN_A)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(
        stream.headers().get(STRICT_TRANSPORT_SECURITY).unwrap(),
        HSTS
    );
    drop(stream);
}

#[tokio::test]
async fn no_hsts_without_an_https_origin() {
    let plain = TestServer::start().await;
    for path in ["/health", "/login", "/api/v1/lists", "/nope"] {
        let (_, headers, _) = plain.request(Method::GET, path, Some(TOKEN_A), None).await;
        assert!(headers.get(STRICT_TRANSPORT_SECURITY).is_none(), "{path}");
        assert_eq!(
            headers.get(X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff",
            "{path}"
        );
    }
}

#[tokio::test]
async fn every_cookie_is_host_only_secure_http_only_and_same_site_strict() {
    let env = Env::start().await;
    let anon = env.browser();
    for path in ["/login", "/setup", "/pair", "/health"] {
        anon.get(path).await;
    }
    let p = anon.post_json("/auth/login/start", json!({})).await;
    assert!(!p.status.is_server_error());
    assert!(
        anon.set_cookies.lock().unwrap().is_empty(),
        "anonymous pages set no cookies"
    );

    let mut owner = env.owner().await;
    owner.step_up().await;
    let p = owner.login().await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    assert_session_cookie(p.set_cookie());
    owner.get("/admin").await;
    let p = owner.post_form("/auth/logout", &[]).await;
    assert!(
        p.status.is_redirection() || p.status.is_success(),
        "{}",
        p.body
    );

    let cookies = owner.set_cookies.lock().unwrap().clone();
    assert!(cookies.len() >= 2, "{cookies:?}");
    for c in &cookies {
        assert!(c.starts_with("__Host-"), "{c}");
        let attrs: Vec<&str> = c.split(';').skip(1).map(str::trim).collect();
        for want in ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"] {
            assert!(attrs.contains(&want), "{want} missing in {c}");
        }
        assert!(!c.to_ascii_lowercase().contains("domain="), "{c}");
    }
}

// ---- logs -----------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// The process-wide log capture: `(what the server's default filter lets through, everything at
/// debug)`. Global, because the server logs from the blocking pool too, which a thread-local
/// subscriber would miss.
fn logs() -> &'static (Capture, Capture) {
    static LOGS: OnceLock<(Capture, Capture)> = OnceLock::new();
    LOGS.get_or_init(|| {
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::SubscriberExt;
        let (info, debug) = (Capture::default(), Capture::default());
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(info.clone())
                    // What `todo-server serve` logs without RUST_LOG: info and above.
                    .with_filter(todo_server::mcp::log_filter(None)),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(debug.clone())
                    .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG),
            );
        tracing::subscriber::set_global_default(subscriber).unwrap();
        (info, debug)
    })
}

#[tokio::test]
async fn tokens_secrets_and_task_content_never_reach_the_log_at_info() {
    let (info, debug) = logs();
    let env = Env::start_with(admin_on).await;
    let mark = uuid::Uuid::new_v4().simple().to_string();
    let content = |what: &str| format!("zzsecret{what}{mark}");
    let mut secrets: Vec<String> = vec![
        TOKEN_A.to_owned(),
        todo_server::auth::hash_token(TOKEN_A),
        ADMIN_TOKEN.to_owned(),
        todo_server::auth::hash_token(ADMIN_TOKEN),
    ];

    // Setup, sign-in and step-up: the setup code, session cookies and CSRF tokens.
    let code = env.take_code().unwrap();
    secrets.push(code.clone());
    let mut owner = env.browser();
    assert_eq!(owner.setup(&code).await.status, StatusCode::OK);
    owner.step_up().await;
    secrets.extend(owner.cookie.clone());
    secrets.extend(owner.csrf.clone());
    // An agent token, made on the admin page.
    let (_, agent_token) = owner.create_agent("logger", "plan", "").await;
    secrets.push(agent_token.clone());
    secrets.push(todo_server::auth::hash_token(&agent_token));

    // REST: content in commands, notes, queries, paths and errors.
    let list = id();
    let r = env
        .command(
            TOKEN_A,
            json!({ "cmd": "create_list", "id": list, "name": content("list") }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let task = id();
    env.command(
        &agent_token,
        json!({ "cmd": "create_task", "id": task, "list": list, "title": content("title") }),
    )
    .await;
    env.command(
        TOKEN_A,
        json!({ "cmd": "add_note", "task": task, "text": content("note") }),
    )
    .await;
    env.command(
        TOKEN_A,
        json!({ "cmd": "create_task", "id": id(), "list": id(), "title": content("orphan") }),
    )
    .await;
    env.command(
        TOKEN_A,
        json!({ "cmd": "update_task", "id": task, "patch": { "title": "" } }),
    )
    .await;
    for path in [
        format!("/api/v1/tasks?query={}", content("query")),
        format!("/api/v1/tasks?query=((({}", content("badquery")),
        format!("/api/v1/tasks/{}", content("path")),
        format!("/{}", content("unmatched")),
        "/api/v1/activity".to_owned(),
        "/api/v1/events?since=0".to_owned(),
    ] {
        env.api(Method::GET, &path, TOKEN_A, None).await;
    }
    let wrong = format!("tda_{}", content("wrongtoken"));
    secrets.push(wrong.clone());
    env.api(Method::GET, "/api/v1/lists", &wrong, None).await;
    let r = env
        .api(
            Method::POST,
            "/api/v1/commands",
            TOKEN_A,
            Some(json!({ "command_id": id(), "command": { "cmd": content("cmd") } })),
        )
        .await;
    assert!(r.status.is_client_error());

    // The event stream.
    let mut sse = env.t.sse(&agent_token, Some(0)).await;
    assert!(sse.next_event(Duration::from_secs(5)).await.is_some());
    drop(sse);

    // MCP, including failing calls and a malformed request.
    let c = connect(&env.t, &agent_token).await;
    ok(
        &c,
        "create_task",
        json!({ "title": content("mcptitle"), "list_id": list }),
    )
    .await;
    ok(&c, "query_tasks", json!({ "query": content("mcpquery") })).await;
    fails(
        &c,
        "update_task",
        json!({ "task_id": id(), "title": content("mcpmissing") }),
    )
    .await;
    let _ = call(
        &c,
        "add_note",
        json!({ "task_id": content("mcpbadid"), "text": content("mcpbad") }),
    )
    .await;
    c.cancel().await.unwrap();
    let res = env
        .t
        .client
        .post(format!("{}/mcp", env.t.url))
        .bearer_auth(&agent_token)
        .header("accept", "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .body(format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{}\"",
            content("rpc")
        ))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_client_error() || res.status().is_success());

    // Pairing: the device name, the user code and the poll token.
    let res = env
        .t
        .client
        .post(format!("{}/api/v1/pair/start", env.t.url))
        .json(&json!({ "device_name": content("device") }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let started: Value = res.json().await.unwrap();
    let poll_token = started["poll_token"].as_str().unwrap().to_owned();
    let user_code = started["user_code"].as_str().unwrap().to_owned();
    secrets.push(poll_token.clone());
    secrets.push(user_code.clone());
    env.t
        .client
        .post(format!("{}/api/v1/pair/poll", env.t.url))
        .json(&json!({ "poll_token": poll_token }))
        .send()
        .await
        .unwrap();
    let p = owner.get(&format!("/pair?code={user_code}")).await;
    assert_eq!(p.status, StatusCode::OK);

    let created = owner.post_json("/api/v1/pair/invites", json!({})).await;
    assert_eq!(created.status, StatusCode::OK);
    let invite = created.json();
    let code = invite["code"].as_str().unwrap();
    let secret = invite["link"].as_str().unwrap().split_once('#').unwrap().1;
    secrets.extend([code.to_owned(), secret.to_owned()]);
    let redeemed = env
        .browser()
        .post_json(
            "/api/v1/pair/redeem",
            json!({ "secret": secret, "device_name": "New device" }),
        )
        .await;
    assert_eq!(redeemed.status, StatusCode::OK);
    secrets.push(redeemed.json()["token"].as_str().unwrap().to_owned());

    // A share link.
    let share = env
        .t
        .server
        .authsvc
        .create_share(
            ShareTarget::List(serde_json::from_value(json!(list)).unwrap()),
            None,
        )
        .unwrap();
    secrets.push(share.clone());
    let p = env.browser().get(&format!("/s/{share}")).await;
    assert_eq!(p.status, StatusCode::OK);
    assert!(p.body.contains(&content("title")));

    // The admin API: a change, a read, a wrong admin token and the admin token on the public API.
    let admin_url = env.t.server.admin_url().unwrap();
    let res = env
        .t
        .client
        .post(format!("{admin_url}/api/v1/admin/ingest/senders"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({ "address": "logged@example.org" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    for token in [
        ADMIN_TOKEN.to_owned(),
        format!("tdx_{}", content("admintoken")),
    ] {
        env.t
            .client
            .get(format!("{admin_url}/api/v1/admin/audit"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
    }
    env.api(Method::GET, "/api/v1/lists", ADMIN_TOKEN, None)
        .await;

    // Signing in again.
    let p = owner.login().await;
    assert_eq!(p.status, StatusCode::OK);
    secrets.extend(owner.cookie.clone());

    env.t.stop().await;
    let info = info.text();
    let debug = debug.text();
    assert!(
        info.contains("route=/api/v1/commands"),
        "the capture works:\n{info}"
    );
    assert!(debug.contains("rejected request"), "the capture works");
    secrets.push(format!("zzsecret{mark}"));
    for what in [
        "list",
        "title",
        "note",
        "orphan",
        "query",
        "badquery",
        "path",
        "unmatched",
        "wrongtoken",
        "cmd",
        "mcptitle",
        "mcpquery",
        "mcpmissing",
        "mcpbadid",
        "mcpbad",
        "rpc",
        "device",
        "admintoken",
    ] {
        secrets.push(content(what));
    }
    for s in &secrets {
        assert!(!s.is_empty());
        assert!(!info.contains(s.as_str()), "`{s}` is in the info log");
    }
    // Content is unique to this run, so nothing that contains the run's mark may show up at all.
    assert!(
        !info.contains(&mark),
        "this run's content is in the info log"
    );
}
