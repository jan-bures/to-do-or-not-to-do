//! Authentication and authorization end to end, against an in-process server: owner setup, passkey
//! sign-in and step-up (driven by a software passkey), browser sessions and CSRF, device pairing,
//! agent tokens and their access levels, write quotas, revocation, share pages, rate limits, and the
//! exact list of routes reachable without authentication.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::{TOKEN_A, TestServer, first_seq, id};
use reqwest::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, HeaderMap, LOCATION, ORIGIN,
    REFERRER_POLICY, RETRY_AFTER, SET_COOKIE, STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use todo_core::time::Timestamp;
use todo_server::web::Gate;
use todo_server::{Config, Options};
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::prelude::Url;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse};

/// The public origin the tests configure (a placeholder; nothing resolves it).
const SITE: &str = "https://todo.example.com";

const MINUTE: Duration = Duration::from_secs(60);

/// What precedes a value shown once (an agent token or a share link).
const SECRET_VALUE: &str = r#"<input class="secret" readonly value=""#;

// ---- harness --------------------------------------------------------------------------------------

/// A server with `public_origin` set, a clock the test can move forward, and the setup code captured.
struct Env {
    t: TestServer,
    offset: Arc<AtomicI64>,
    code: Arc<Mutex<Option<String>>>,
}

impl Env {
    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// Generous per-IP limits, so only the rate-limit tests hit them; `f` can change anything.
    async fn start_with(f: impl FnOnce(&mut Config)) -> Self {
        let offset = Arc::new(AtomicI64::new(0));
        let code = Arc::new(Mutex::new(None));
        let (o, c) = (offset.clone(), code.clone());
        let options = Options {
            clock: Arc::new(move || {
                Timestamp::from_millisecond(
                    Timestamp::now().as_millisecond() + o.load(Ordering::SeqCst),
                )
                .unwrap()
            }),
            setup_code: Some(Arc::new(move |s: &str| {
                *c.lock().unwrap() = Some(s.to_owned());
            })),
            ..Options::default()
        };
        let t = TestServer::start_with_options(
            |cfg| {
                cfg.public_origin = Some(SITE.into());
                cfg.auth.auth_requests_per_minute = 100_000;
                cfg.auth.auth_request_burst = 10_000;
                f(cfg);
            },
            options,
        )
        .await;
        Self { t, offset, code }
    }

    fn advance(&self, by: Duration) {
        self.offset
            .fetch_add(i64::try_from(by.as_millis()).unwrap(), Ordering::SeqCst);
    }

    /// Restarts the server on the same data directory (with the same clock and code sink).
    async fn restart(self) -> Self {
        self.restart_with(|_| {}).await
    }

    /// Restarts, running `f` on the stopped server's database file first.
    async fn restart_with(self, f: impl FnOnce(&std::path::Path)) -> Self {
        let Self { t, offset, code } = self;
        Self {
            t: t.restart_with(f).await,
            offset,
            code,
        }
    }

    fn take_code(&self) -> Option<String> {
        self.code.lock().unwrap().take()
    }

    fn browser(&self) -> Browser {
        Browser::new(&self.t.url)
    }

    /// Completes setup and returns the signed-in owner's browser.
    async fn owner(&self) -> Browser {
        let mut b = self.browser();
        let code = self.take_code().expect("a setup code");
        let p = b.setup(&code).await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
        b
    }

    /// Owner-token API call (the config fallback the shared helpers use).
    async fn api(&self, method: Method, path: &str, token: &str, body: Option<Value>) -> Response {
        let (status, headers, json) = self.t.request(method, path, Some(token), body).await;
        Response {
            status,
            headers,
            json,
        }
    }

    async fn command(&self, token: &str, command: Value) -> Response {
        self.api(
            Method::POST,
            "/api/v1/commands",
            token,
            Some(json!({ "command_id": id(), "command": command })),
        )
        .await
    }

    /// The ID of the event at `seq` (read with an owner token).
    async fn event_id(&self, seq: u64) -> String {
        let r = self
            .api(
                Method::GET,
                &format!("/api/v1/events?since={}&limit=1", seq - 1),
                TOKEN_A,
                None,
            )
            .await;
        r.json["events"][0]["id"].as_str().unwrap().to_owned()
    }

    async fn undo(&self, token: &str, seq: u64) -> Response {
        let expected = self.event_id(seq).await;
        self.undo_expecting(token, seq, &expected).await
    }

    async fn undo_expecting(&self, token: &str, seq: u64, expected: &str) -> Response {
        self.api(
            Method::POST,
            &format!("/api/v1/events/{seq}/undo"),
            token,
            Some(json!({ "command_id": id(), "expected_event_id": expected })),
        )
        .await
    }
}

struct Response {
    status: StatusCode,
    headers: HeaderMap,
    json: Value,
}

/// One HTTP response from the web routes.
#[derive(Debug)]
struct Page {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Page {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    fn header(&self, name: impl reqwest::header::AsHeaderName) -> &str {
        self.headers
            .get(name)
            .map_or("", |v| v.to_str().unwrap_or(""))
    }

    fn location(&self) -> &str {
        self.header(LOCATION)
    }

    fn set_cookie(&self) -> &str {
        self.header(SET_COOKIE)
    }
}

fn form_encode(fields: &[(&str, &str)]) -> String {
    fn enc(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                out.push(char::from(b));
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Text between `start` and the next `"` after it.
fn attr_after<'a>(body: &'a str, start: &str) -> Option<&'a str> {
    let i = body.find(start)? + start.len();
    let rest = &body[i..];
    Some(&rest[..rest.find('"')?])
}

/// A browser: no automatic redirects, a cookie jar of one, the CSRF token of the last page seen, and
/// a software passkey.
struct Browser {
    client: reqwest::Client,
    url: String,
    cookie: Option<String>,
    csrf: Option<String>,
    key: SoftPasskey,
    /// Sent with every request (e.g. `CF-Connecting-IP`, to act as another client address).
    extra: Vec<(&'static str, String)>,
}

enum Body {
    None,
    Json(Value),
    Form(String),
}

impl Browser {
    fn new(url: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            url: url.to_owned(),
            cookie: None,
            csrf: None,
            // `true`: the software key reports user verification (it has no PIN or biometric).
            key: SoftPasskey::new(true),
            extra: Vec::new(),
        }
    }

    fn site() -> Url {
        Url::parse(SITE).unwrap()
    }

    async fn send(&self, method: Method, path: &str, origin: Option<&str>, body: Body) -> Page {
        let mut req = self.client.request(method, format!("{}{path}", self.url));
        if let Some(c) = &self.cookie {
            req = req.header(COOKIE, format!("__Host-todo_session={c}"));
        }
        for (k, v) in &self.extra {
            req = req.header(*k, v);
        }
        if let Some(o) = origin {
            req = req.header(ORIGIN, o);
        }
        req = match body {
            Body::None => req,
            Body::Json(v) => {
                if let Some(c) = &self.csrf {
                    req = req.header("x-csrf-token", c);
                }
                req.json(&v)
            }
            Body::Form(f) => req
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(f),
        };
        let res = req.send().await.unwrap();
        Page {
            status: res.status(),
            headers: res.headers().clone(),
            body: res.text().await.unwrap(),
        }
    }

    async fn get(&self, path: &str) -> Page {
        self.send(Method::GET, path, None, Body::None).await
    }

    async fn post_json(&self, path: &str, body: Value) -> Page {
        self.send(Method::POST, path, Some(SITE), Body::Json(body))
            .await
    }

    /// Posts a form from this site's own page: the right `Origin` and the session's CSRF token.
    async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Page {
        let csrf = self.csrf.clone().unwrap_or_default();
        let mut all = vec![("csrf", csrf.as_str())];
        all.extend_from_slice(fields);
        self.send(
            Method::POST,
            path,
            Some(SITE),
            Body::Form(form_encode(&all)),
        )
        .await
    }

    /// Takes the session cookie from a response and the CSRF token from the admin page.
    async fn adopt(&mut self, p: &Page) {
        let c = p.set_cookie();
        let value = c
            .strip_prefix("__Host-todo_session=")
            .and_then(|r| r.split(';').next())
            .expect("a session cookie");
        self.cookie = Some(value.to_owned());
        self.refresh_csrf().await;
    }

    async fn refresh_csrf(&mut self) {
        let p = self.get("/admin").await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
        self.csrf = Some(
            attr_after(&p.body, r#"<meta name="csrf-token" content=""#)
                .unwrap()
                .to_owned(),
        );
    }

    fn register(&mut self, options: &Value) -> Value {
        let ccr: CreationChallengeResponse = serde_json::from_value(options.clone()).unwrap();
        let cred = self.key.do_registration(Self::site(), ccr).unwrap();
        serde_json::to_value(cred).unwrap()
    }

    fn assert(&mut self, options: &Value) -> Value {
        let rcr: RequestChallengeResponse = serde_json::from_value(options.clone()).unwrap();
        let cred = self.key.do_authentication(Self::site(), rcr).unwrap();
        serde_json::to_value(cred).unwrap()
    }

    /// Runs the setup ceremony; on success the browser is signed in.
    async fn setup(&mut self, code: &str) -> Page {
        let p = self
            .post_json("/auth/setup/start", json!({ "code": code }))
            .await;
        if p.status != StatusCode::OK {
            return p;
        }
        let v = p.json();
        let credential = self.register(&v["options"]);
        let p = self
            .post_json(
                "/auth/setup/finish",
                json!({ "ceremony": v["ceremony"], "credential": credential }),
            )
            .await;
        if p.status == StatusCode::OK {
            self.adopt(&p).await;
        }
        p
    }

    /// A sign-in assertion for a fresh `login/start`: `(ceremony, credential)`.
    async fn login_assertion(&mut self) -> (Value, Value) {
        let p = self.post_json("/auth/login/start", json!({})).await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
        let v = p.json();
        let credential = self.assert(&v["options"]);
        (v["ceremony"].clone(), credential)
    }

    async fn login(&mut self) -> Page {
        self.cookie = None;
        self.csrf = None;
        let (ceremony, credential) = self.login_assertion().await;
        let p = self
            .post_json(
                "/auth/login/finish",
                json!({ "ceremony": ceremony, "credential": credential, "next": "/pair" }),
            )
            .await;
        if p.status == StatusCode::OK {
            self.adopt(&p).await;
        }
        p
    }

    async fn step_up(&mut self) {
        let p = self.post_json("/auth/step-up/start", json!({})).await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
        let v = p.json();
        let credential = self.assert(&v["options"]);
        let p = self
            .post_json(
                "/auth/step-up/finish",
                json!({ "ceremony": v["ceremony"], "credential": credential }),
            )
            .await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    }

    /// Creates an agent token on the admin page; returns `(row id, token)`.
    async fn create_agent(
        &self,
        name: &str,
        default: &str,
        writes_per_hour: &str,
    ) -> (i64, String) {
        let p = self
            .post_form(
                "/admin/agents",
                &[
                    ("name", name),
                    ("default", default),
                    ("writes_per_hour", writes_per_hour),
                    ("expires_in_days", ""),
                ],
            )
            .await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
        let token = attr_after(&p.body, SECRET_VALUE).unwrap().to_owned();
        assert!(token.starts_with("tda_"), "{token}");
        let list = self.get("/admin/agents").await;
        let row = &list.body[list.body.find(&format!("<code>{name}</code>")).unwrap()..];
        let id = attr_after(row, r#"href="/admin/agents/"#).unwrap();
        (id.parse().unwrap(), token)
    }
}

fn assert_session_cookie(set_cookie: &str) {
    assert!(
        set_cookie.starts_with("__Host-todo_session="),
        "{set_cookie}"
    );
    let value = set_cookie
        .strip_prefix("__Host-todo_session=")
        .and_then(|r| r.split(';').next())
        .unwrap();
    // 256 bits, base64url without padding.
    assert_eq!(value.len(), 43, "{value}");
    let attrs: Vec<&str> = set_cookie.split(';').skip(1).map(str::trim).collect();
    for want in ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"] {
        assert!(attrs.contains(&want), "{want} missing in {set_cookie}");
    }
    assert!(
        !set_cookie.to_ascii_lowercase().contains("domain="),
        "{set_cookie}"
    );
}

async fn create_list(env: &Env, name: &str) -> String {
    let list = id();
    let r = env
        .command(
            TOKEN_A,
            json!({ "cmd": "create_list", "id": list, "name": name }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    list
}

async fn create_task(env: &Env, token: &str, list: &str, title: &str) -> (Response, String) {
    let task = id();
    let r = env
        .command(
            token,
            json!({ "cmd": "create_task", "id": task, "list": list, "title": title }),
        )
        .await;
    (r, task)
}

/// The actor recorded on event `seq`.
async fn actor_of(env: &Env, seq: u64) -> Value {
    let r = env
        .api(
            Method::GET,
            &format!("/api/v1/events?since={}&limit=1", seq - 1),
            TOKEN_A,
            None,
        )
        .await;
    r.json["events"][0]["actor"].clone()
}

// ---- setup ----------------------------------------------------------------------------------------

#[tokio::test]
async fn the_setup_code_is_single_use_and_setup_closes_for_good() {
    let env = Env::start().await;
    let code = env.take_code().expect("the server printed a setup code");
    assert_eq!(code.len(), 23, "20 characters in groups of five: {code}");

    // The page exists and never contains the code.
    let mut b = env.browser();
    let page = b.get("/setup").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.body.contains(&code));
    assert!(!page.body.contains(&code.replace('-', "")));

    // A wrong code is refused.
    let p = b.setup("AAAAA-AAAAA-AAAAA-AAAAA").await;
    assert_eq!(p.status, StatusCode::UNAUTHORIZED, "{}", p.body);
    assert_eq!(p.json()["error"]["code"], "invalid_setup_code");

    // Two ceremonies started with the right code: the first one to finish wins.
    let mut other = env.browser();
    let started = other
        .post_json("/auth/setup/start", json!({ "code": code }))
        .await;
    assert_eq!(started.status, StatusCode::OK);

    let p = b.setup(&code.to_lowercase()).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    assert_session_cookie(p.set_cookie());
    assert_eq!(p.json()["redirect"], "/admin");
    assert_eq!(b.get("/admin").await.status, StatusCode::OK);

    let v = started.json();
    let credential = other.register(&v["options"]);
    let late = other
        .post_json(
            "/auth/setup/finish",
            json!({ "ceremony": v["ceremony"], "credential": credential }),
        )
        .await;
    assert_eq!(late.status, StatusCode::NOT_FOUND);
    assert!(late.set_cookie().is_empty());

    // The code is spent and /setup is gone, also after a restart (which prints no new code).
    let again = env
        .browser()
        .post_json("/auth/setup/start", json!({ "code": code }))
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    assert_eq!(
        env.browser().get("/setup").await.status,
        StatusCode::NOT_FOUND
    );

    let env = env.restart().await;
    assert_eq!(env.take_code(), None);
    assert_eq!(
        env.browser().get("/setup").await.status,
        StatusCode::NOT_FOUND
    );
    // The passkey still signs in.
    b.url.clone_from(&env.t.url);
    let p = b.login().await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
}

#[tokio::test]
async fn the_setup_code_expires() {
    let env = Env::start().await;
    let code = env.take_code().unwrap();
    env.advance(16 * MINUTE);
    let p = env.browser().setup(&code).await;
    assert_eq!(p.status, StatusCode::UNAUTHORIZED, "{}", p.body);
}

/// A browser at another client address (needs `trust_cloudflare_headers`).
fn browser_at(env: &Env, ip: &str) -> Browser {
    let mut b = env.browser();
    b.extra.push(("cf-connecting-ip", ip.to_owned()));
    b
}

#[tokio::test]
async fn wrong_setup_codes_lock_out_their_source_but_never_spoil_the_code() {
    let env = Env::start_with(|c| c.trust_cloudflare_headers = true).await;
    let code = env.take_code().unwrap();
    let mut attacker = browser_at(&env, "203.0.113.7");
    let mut statuses = Vec::new();
    for _ in 0..12 {
        statuses.push(attacker.setup("WRONG-WRONG-WRONG-WRONG").await.status);
    }
    assert_eq!(statuses[0], StatusCode::UNAUTHORIZED);
    assert_eq!(
        *statuses.last().unwrap(),
        StatusCode::TOO_MANY_REQUESTS,
        "{statuses:?}"
    );
    // The attacker, now locked out, can't even try the right code.
    assert_eq!(
        attacker.setup(&code).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    // The owner, elsewhere, still can.
    let mut owner = browser_at(&env, "203.0.113.9");
    let p = owner.setup(&code).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
}

#[tokio::test]
async fn the_setup_code_is_written_to_an_owner_only_file_until_setup_is_done() {
    let env = Env::start().await;
    let code = env.take_code().unwrap();
    let file = env
        .t
        .config
        .data_dir
        .join(todo_server::auth::service::SETUP_CODE_FILE);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with(&code), "{text}");
    assert!(text.contains(&format!("{SITE}/setup")));
    todo_fs::verify_private(&file, todo_fs::Private::File).expect("owner-only");
    // A restart before setup writes a fresh code to the same file.
    let env = env.restart().await;
    let fresh = env.take_code().unwrap();
    assert_ne!(fresh, code);
    assert!(std::fs::read_to_string(&file).unwrap().starts_with(&fresh));
    // Setup removes it, and it does not come back.
    let p = env.browser().setup(&fresh).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    assert!(!file.exists());
    let env = env.restart().await;
    assert!(!file.exists());
    assert_eq!(env.take_code(), None);
}

#[tokio::test]
async fn one_address_cannot_fill_the_pairing_table() {
    let env = Env::start_with(|c| {
        c.trust_cloudflare_headers = true;
        c.auth.pairings_per_ip = 2;
    })
    .await;
    let start = |ip: &'static str| {
        let env = &env;
        async move {
            let res = env
                .t
                .client
                .post(format!("{}/api/v1/pair/start", env.t.url))
                .header("cf-connecting-ip", ip)
                .json(&json!({ "device_name": "Laptop" }))
                .send()
                .await
                .unwrap();
            let status = res.status();
            (status, res.json::<Value>().await.unwrap())
        }
    };
    for _ in 0..2 {
        assert_eq!(start("203.0.113.7").await.0, StatusCode::OK);
    }
    let (s, v) = start("203.0.113.7").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert_eq!(v["error"]["code"], "too_many_pairings");
    assert_eq!(start("203.0.113.9").await.0, StatusCode::OK);
}

#[tokio::test]
async fn without_a_public_origin_there_is_no_setup_and_no_pairing() {
    let t = TestServer::start().await;
    let b = Browser::new(&t.url);
    assert_eq!(b.get("/setup").await.status, StatusCode::NOT_FOUND);
    let (s, _, v) = t
        .request(
            Method::POST,
            "/api/v1/pair/start",
            None,
            Some(json!({ "device_name": "Laptop" })),
        )
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
}

// ---- sessions -------------------------------------------------------------------------------------

#[tokio::test]
async fn sign_in_sets_a_strict_host_cookie_and_sign_out_ends_the_session() {
    let env = Env::start().await;
    let mut b = env.owner().await;

    // Sign out: the cookie is cleared and the old value no longer works.
    let old = b.cookie.clone();
    let p = b.post_form("/auth/logout", &[]).await;
    assert_eq!(p.status, StatusCode::SEE_OTHER);
    assert_eq!(p.location(), "/login");
    assert!(p.set_cookie().contains("Max-Age=0"), "{}", p.set_cookie());
    b.cookie = old;
    let p = b.get("/admin").await;
    assert_eq!(p.status, StatusCode::SEE_OTHER);
    assert!(p.location().starts_with("/login?next="), "{}", p.location());

    // Sign in again with the passkey.
    let p = b.login().await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    assert_session_cookie(p.set_cookie());
    assert!(
        p.set_cookie().contains("Max-Age=43200"),
        "absolute timeout: {}",
        p.set_cookie()
    );
    assert_eq!(p.json()["redirect"], "/pair");
    let admin = b.get("/admin").await;
    assert_eq!(admin.status, StatusCode::OK);
    assert_eq!(admin.header(CACHE_CONTROL), "no-store");

    // An unknown cookie is no session.
    b.cookie = Some("A".repeat(43));
    assert_eq!(b.get("/admin").await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn sessions_end_after_the_idle_and_absolute_timeouts() {
    let env = Env::start().await;
    let mut b = env.owner().await;
    env.advance(59 * MINUTE);
    assert_eq!(b.get("/admin").await.status, StatusCode::OK);
    env.advance(59 * MINUTE);
    assert_eq!(
        b.get("/admin").await.status,
        StatusCode::OK,
        "activity keeps it alive"
    );
    env.advance(61 * MINUTE);
    assert_eq!(
        b.get("/admin").await.status,
        StatusCode::SEE_OTHER,
        "idle timeout"
    );

    // Absolute: active all the time, but 12 hours after sign-in it ends.
    b.login().await;
    for _ in 0..13 {
        env.advance(55 * MINUTE);
        assert_eq!(b.get("/admin").await.status, StatusCode::OK);
    }
    env.advance(6 * MINUTE);
    assert_eq!(
        b.get("/admin").await.status,
        StatusCode::SEE_OTHER,
        "absolute timeout"
    );
}

#[tokio::test]
async fn state_changing_requests_need_the_origin_and_the_csrf_token() {
    let env = Env::start().await;
    let b = env.owner().await;
    let csrf = b.csrf.clone().unwrap();
    let form = |csrf: &str| {
        form_encode(&[
            ("csrf", csrf),
            ("name", "sneaky"),
            ("default", "plan"),
            ("writes_per_hour", ""),
            ("expires_in_days", ""),
        ])
    };

    // No Origin, a foreign Origin, or `null`.
    for origin in [None, Some("https://evil.example.com"), Some("null")] {
        let p = b
            .send(
                Method::POST,
                "/admin/agents",
                origin,
                Body::Form(form(&csrf)),
            )
            .await;
        assert_eq!(p.status, StatusCode::FORBIDDEN, "{origin:?}: {}", p.body);
    }
    // The right Origin but a wrong or missing token.
    let p = b
        .send(
            Method::POST,
            "/admin/agents",
            Some(SITE),
            Body::Form(form("wrong")),
        )
        .await;
    assert_eq!(p.status, StatusCode::FORBIDDEN, "{}", p.body);
    let p = b
        .send(
            Method::POST,
            "/admin/agents",
            Some(SITE),
            Body::Form(form_encode(&[("name", "sneaky"), ("default", "plan")])),
        )
        .await;
    assert!(p.status.is_client_error(), "{}", p.status);
    // JSON ceremony endpoints: the header token.
    let mut no_header = Browser::new(&env.t.url);
    no_header.cookie = b.cookie.clone();
    let p = no_header.post_json("/auth/step-up/start", json!({})).await;
    assert_eq!(p.status, StatusCode::FORBIDDEN, "{}", p.body);
    // Sign-out is a state change too.
    let p = b
        .send(
            Method::POST,
            "/auth/logout",
            None,
            Body::Form(form_encode(&[("csrf", &csrf)])),
        )
        .await;
    assert_eq!(p.status, StatusCode::FORBIDDEN);
    // Unauthenticated ceremony endpoints still need the Origin.
    let p = env
        .browser()
        .send(
            Method::POST,
            "/auth/login/start",
            None,
            Body::Json(json!({})),
        )
        .await;
    assert_eq!(p.status, StatusCode::FORBIDDEN);

    // None of it created anything; the real form works.
    assert!(!b.get("/admin/agents").await.body.contains("sneaky"));
    let p = b
        .send(
            Method::POST,
            "/admin/agents",
            Some(SITE),
            Body::Form(form(&csrf)),
        )
        .await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    assert_eq!(
        b.get("/admin").await.status,
        StatusCode::OK,
        "still signed in"
    );
}

#[tokio::test]
async fn admin_actions_need_a_recent_passkey_verification() {
    let env = Env::start().await;
    let mut b = env.owner().await;
    let list = create_list(&env, "Errands").await;
    env.advance(6 * MINUTE);

    // Still signed in, but not fresh.
    let admin = b.get("/admin").await;
    assert_eq!(admin.status, StatusCode::OK);
    assert!(admin.body.contains(r#"data-action="step-up""#));
    let attempts: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "/admin/agents",
            vec![
                ("name", "late".into()),
                ("default", "read".into()),
                ("writes_per_hour", String::new()),
                ("expires_in_days", String::new()),
            ],
        ),
        ("/admin/shares", vec![("target", format!("list:{list}"))]),
        ("/admin/devices/some-device/revoke", vec![]),
        ("/admin/agents/1/revoke", vec![]),
        ("/admin/agents/1/grants", vec![("default", "plan".into())]),
        ("/admin/shares/1/rotate", vec![]),
        ("/admin/shares/1/revoke", vec![]),
        ("/pair/approve", vec![("code", "ABCD-EFGH".into())]),
    ];
    for (path, fields) in &attempts {
        let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let p = b.post_form(path, &fields).await;
        assert_eq!(p.status, StatusCode::FORBIDDEN, "{path}: {}", p.body);
        assert!(
            p.body.contains("data-action=\"step-up\""),
            "{path}: {}",
            p.body
        );
    }
    let p = b.post_json("/auth/passkeys/start", json!({})).await;
    assert_eq!(p.status, StatusCode::FORBIDDEN);
    assert_eq!(p.json()["error"]["code"], "step_up_required");

    // After a passkey verification they work, for five minutes.
    b.step_up().await;
    let (_, token) = b.create_agent("fresh", "read", "").await;
    assert!(token.starts_with("tda_"));
    let p = b.post_json("/auth/passkeys/start", json!({})).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    env.advance(5 * MINUTE + Duration::from_secs(1));
    let p = b
        .post_form("/admin/shares", &[("target", &format!("list:{list}"))])
        .await;
    assert_eq!(p.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn another_passkey_can_be_added_and_used() {
    let env = Env::start().await;
    let b = env.owner().await;
    let p = b.post_json("/auth/passkeys/start", json!({})).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    let v = p.json();
    assert_eq!(
        v["options"]["publicKey"]["authenticatorSelection"]["residentKey"],
        "preferred"
    );
    assert_eq!(
        v["options"]["publicKey"]["authenticatorSelection"]["userVerification"],
        "required"
    );
    // The first passkey is excluded, so a second authenticator registers.
    let mut phone = Browser::new(&env.t.url);
    phone.cookie = b.cookie.clone();
    phone.csrf = b.csrf.clone();
    let credential = phone.register(&v["options"]);
    let p = phone
        .post_json(
            "/auth/passkeys/finish",
            json!({ "ceremony": v["ceremony"], "credential": credential, "name": "Phone" }),
        )
        .await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    assert!(b.get("/admin").await.body.contains("Phone"));
    let p = phone.login().await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
}

#[tokio::test]
async fn failed_sign_ins_lock_the_address_out() {
    let env = Env::start().await;
    let mut b = env.owner().await;
    for _ in 0..5 {
        let (_, credential) = b.login_assertion().await;
        let p = b
            .post_json(
                "/auth/login/finish",
                json!({ "ceremony": "not-a-ceremony", "credential": credential }),
            )
            .await;
        assert_eq!(p.status, StatusCode::BAD_REQUEST, "{}", p.body);
        assert_eq!(p.json()["error"]["code"], "ceremony_expired");
    }
    let p = b.post_json("/auth/login/start", json!({})).await;
    assert_eq!(p.status, StatusCode::TOO_MANY_REQUESTS, "{}", p.body);
    assert_eq!(p.json()["error"]["code"], "locked_out");
    assert!(p.header(RETRY_AFTER).parse::<u32>().unwrap() > 0);
    // The session from before still works, and the audit log has it all.
    let audit = b.get("/admin/audit").await.body;
    assert!(audit.contains("lockout"));
    assert!(audit.contains("sign_in_failed"));
}

#[tokio::test]
async fn sign_in_and_pairing_routes_are_rate_limited_per_address() {
    let env = Env::start_with(|c| {
        c.auth.auth_requests_per_minute = 30;
        c.auth.auth_request_burst = 5;
    })
    .await;
    let b = env.browser();
    let mut limited = false;
    for _ in 0..10 {
        let p = b.get("/login").await;
        if p.status == StatusCode::TOO_MANY_REQUESTS {
            assert!(p.header(RETRY_AFTER).parse::<u32>().unwrap() > 0);
            limited = true;
            break;
        }
    }
    assert!(limited, "GET /login was never limited");
    let (s, _, _) = env
        .t
        .request(
            Method::POST,
            "/api/v1/pair/start",
            None,
            Some(json!({ "device_name": "Laptop" })),
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    // Share pages and assets are not in that budget.
    assert_eq!(b.get("/assets/app.css").await.status, StatusCode::OK);
}

#[tokio::test]
async fn api_requests_are_rate_limited_per_token() {
    let env = Env::start_with(|c| {
        c.auth.api_requests_per_minute = 60;
        c.auth.api_request_burst = 3;
    })
    .await;
    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(env.api(Method::GET, "/api/v1/lists", TOKEN_A, None).await);
    }
    let limited = statuses.last().unwrap();
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.json["error"]["code"], "rate_limited");
    assert!(limited.headers.get(RETRY_AFTER).is_some());
    // Another token has its own budget.
    let r = env
        .api(Method::GET, "/api/v1/lists", common::TOKEN_B, None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
}

async fn api_with(env: &Env, token: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut req = env
        .t
        .client
        .get(format!("{}/api/v1/lists", env.t.url))
        .header("authorization", format!("Bearer {token}"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.send().await.unwrap().status()
}

#[tokio::test]
async fn failed_tokens_are_turned_away_before_authentication() {
    let env = Env::start_with(|c| {
        c.auth.bearer_failures_per_minute = 1;
        c.auth.bearer_failure_burst = 3;
    })
    .await;
    for _ in 0..3 {
        assert_eq!(api_with(&env, "wrong", &[]).await, StatusCode::UNAUTHORIZED);
    }
    // The budget is spent: the address gets 429 whatever it sends, before any token is looked at.
    let r = env.api(Method::GET, "/api/v1/lists", "wrong", None).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(r.json["error"]["code"], "rate_limited");
    assert!(r.headers.get(RETRY_AFTER).is_some());
    assert_eq!(
        api_with(&env, TOKEN_A, &[]).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Forwarding headers don't give a new budget: without trust_cloudflare_headers they are ignored.
    for headers in [
        [("cf-connecting-ip", "203.0.113.7")],
        [("x-forwarded-for", "203.0.113.8")],
    ] {
        assert_eq!(
            api_with(&env, "wrong", &headers).await,
            StatusCode::TOO_MANY_REQUESTS,
            "{headers:?}"
        );
    }
}

#[tokio::test]
async fn cf_connecting_ip_is_used_only_when_trusted() {
    let env = Env::start_with(|c| {
        c.trust_cloudflare_headers = true;
        c.auth.bearer_failures_per_minute = 1;
        c.auth.bearer_failure_burst = 2;
    })
    .await;
    let a = [("cf-connecting-ip", "203.0.113.7")];
    let b = [("cf-connecting-ip", "203.0.113.9")];
    for _ in 0..2 {
        assert_eq!(api_with(&env, "wrong", &a).await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        api_with(&env, "wrong", &a).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Another client behind the tunnel has its own budget.
    assert_eq!(api_with(&env, TOKEN_A, &b).await, StatusCode::OK);
    // X-Forwarded-For is never trusted: this is the tunnel's own (peer) address.
    let xff = [("x-forwarded-for", "203.0.113.7")];
    assert_eq!(api_with(&env, TOKEN_A, &xff).await, StatusCode::OK);
}

// ---- pairing --------------------------------------------------------------------------------------

async fn pair_start(env: &Env, name: &str) -> Value {
    let (s, _, v) = env
        .t
        .request(
            Method::POST,
            "/api/v1/pair/start",
            None,
            Some(json!({ "device_name": name })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

async fn pair_poll(env: &Env, poll_token: &Value) -> (StatusCode, Value) {
    let (s, _, v) = env
        .t
        .request(
            Method::POST,
            "/api/v1/pair/poll",
            None,
            Some(json!({ "poll_token": poll_token })),
        )
        .await;
    (s, v)
}

#[tokio::test]
async fn a_device_pairs_end_to_end_and_its_token_works_until_revoked() {
    let env = Env::start().await;
    let mut owner = env.owner().await;

    let started = pair_start(&env, "Owner's Laptop (work)").await;
    let code = started["user_code"].as_str().unwrap().to_owned();
    assert_eq!(code.len(), 9, "{code}");
    assert_eq!(&code[4..5], "-");
    assert!(
        code.chars()
            .filter(|c| *c != '-')
            .all(|c| "ABCDEFGHJKMNPQRSTUVWXYZ23456789".contains(c)),
        "{code}"
    );
    let uri = started["verification_uri"].as_str().unwrap();
    assert_eq!(uri, format!("{SITE}/pair?code={code}"));
    assert!(started["poll_token"].as_str().unwrap().len() >= 43);
    assert_eq!(started["expires_in"], 300);
    let (s, v) = pair_poll(&env, &started["poll_token"]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["status"], "pending");

    // Signed out, the page shows nothing about the device.
    let path = uri.strip_prefix(SITE).unwrap();
    let anon = env.browser().get(path).await;
    assert_eq!(anon.status, StatusCode::OK);
    assert!(!anon.body.contains("Laptop"));
    assert!(anon.body.contains(r#"data-action="login""#));
    // Nor can anyone approve it.
    let p = env
        .browser()
        .send(
            Method::POST,
            "/pair/approve",
            Some(SITE),
            Body::Form(form_encode(&[("csrf", "x"), ("code", &code)])),
        )
        .await;
    assert_eq!(p.status, StatusCode::UNAUTHORIZED);

    // The signed-in owner sees the device's name, HTML-escaped, and approves.
    let page = owner.get(path).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body.contains("Owner&#39;s Laptop (work)"),
        "{}",
        page.body
    );
    assert!(page.body.contains(r#"action="/pair/approve""#));
    let p = owner.post_form("/pair/approve", &[("code", &code)]).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);

    // The device gets its token exactly once.
    let (s, v) = pair_poll(&env, &started["poll_token"]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "approved");
    let token = v["token"].as_str().unwrap().to_owned();
    let device = v["device_id"].as_str().unwrap().to_owned();
    assert!(token.starts_with("tdd_"), "{token}");
    assert_eq!(
        pair_poll(&env, &started["poll_token"]).await.0,
        StatusCode::NOT_FOUND
    );
    // The code is used up.
    let p = owner.post_form("/pair/approve", &[("code", &code)]).await;
    assert_eq!(p.status, StatusCode::NOT_FOUND);

    // The token authenticates as the owner on that device.
    let list = id();
    let r = env
        .command(
            &token,
            json!({ "cmd": "create_list", "id": list, "name": "From the laptop" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(
        actor_of(&env, first_seq(&r.json)).await,
        json!({ "type": "owner", "device": device })
    );
    let r = env.api(Method::GET, "/api/v1/lists", &token, None).await;
    assert_eq!(r.status, StatusCode::OK);

    // The admin page lists it, with a last-used time; revoking it is immediate.
    let devices = owner.get("/admin/devices").await;
    assert!(devices.body.contains(&device));
    assert!(!devices.body.contains(&token));
    owner.step_up().await;
    let p = owner
        .post_form(&format!("/admin/devices/{device}/revoke"), &[])
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let r = env.api(Method::GET, "/api/v1/lists", &token, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    owner.refresh_csrf().await;
    let audit = owner.get("/admin/audit").await.body;
    for kind in ["pairing_approved", "device_paired", "device_revoked"] {
        assert!(audit.contains(kind), "{kind}");
    }
    assert!(!audit.contains(&token));
}

#[tokio::test]
async fn pairing_codes_expire_and_cannot_be_reused() {
    let env = Env::start().await;
    let mut owner = env.owner().await;

    // Expired: neither approvable nor pollable.
    let a = pair_start(&env, "Old phone").await;
    env.advance(5 * MINUTE + Duration::from_secs(1));
    owner.step_up().await;
    let code = a["user_code"].as_str().unwrap();
    let page = owner.get(&format!("/pair?code={code}")).await;
    assert!(page.body.contains("no pending request"), "{}", page.body);
    let p = owner.post_form("/pair/approve", &[("code", code)]).await;
    assert_eq!(p.status, StatusCode::NOT_FOUND);
    assert_eq!(
        pair_poll(&env, &a["poll_token"]).await.0,
        StatusCode::NOT_FOUND
    );

    // Denied: the device learns once, and the code is spent.
    let b = pair_start(&env, "Unknown device").await;
    let code = b["user_code"].as_str().unwrap();
    let p = owner.post_form("/pair/deny", &[("code", code)]).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    let (s, v) = pair_poll(&env, &b["poll_token"]).await;
    assert_eq!((s, v["status"].as_str()), (StatusCode::OK, Some("denied")));
    assert_eq!(
        pair_poll(&env, &b["poll_token"]).await.0,
        StatusCode::NOT_FOUND
    );
    let p = owner.post_form("/pair/approve", &[("code", code)]).await;
    assert_eq!(p.status, StatusCode::NOT_FOUND);

    // Unknown poll tokens and codes.
    assert_eq!(
        pair_poll(&env, &json!("nope")).await.0,
        StatusCode::NOT_FOUND
    );
    let p = owner
        .post_form("/pair/approve", &[("code", "ABCD-EFGH")])
        .await;
    assert_eq!(p.status, StatusCode::NOT_FOUND);

    // Guessing codes on the page is limited per session.
    for _ in 0..10 {
        owner.get("/pair?code=ZZZZ-ZZZZ").await;
    }
    let c = pair_start(&env, "Real one").await;
    let page = owner
        .get(&format!("/pair?code={}", c["user_code"].as_str().unwrap()))
        .await;
    assert_eq!(page.status, StatusCode::TOO_MANY_REQUESTS);

    // Device names are validated.
    let (s, _, _) = env
        .t
        .request(
            Method::POST,
            "/api/v1/pair/start",
            None,
            Some(json!({ "device_name": "x".repeat(65) })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

// ---- agents ---------------------------------------------------------------------------------------

/// Lists with every level for agent `scout`, a task in each, and its token.
struct Fixture {
    env: Env,
    owner: Browser,
    agent: i64,
    token: String,
    /// list name → (list id, task id)
    lists: std::collections::BTreeMap<&'static str, (String, String)>,
}

async fn fixture(writes_per_hour: &str) -> Fixture {
    let env = Env::start().await;
    let owner = env.owner().await;
    let mut lists = std::collections::BTreeMap::new();
    for name in ["read", "capture", "plan", "hidden"] {
        let list = create_list(&env, name).await;
        let (r, task) = create_task(&env, TOKEN_A, &list, &format!("{name} task")).await;
        assert_eq!(r.status, StatusCode::OK);
        lists.insert(name, (list, task));
    }
    let (agent, token) = owner.create_agent("scout", "none", writes_per_hour).await;
    let p = owner
        .post_form(
            &format!("/admin/agents/{agent}/grants"),
            &[
                ("default", "none"),
                (&format!("l:{}", lists["read"].0), "read"),
                (&format!("l:{}", lists["capture"].0), "capture"),
                (&format!("l:{}", lists["plan"].0), "plan"),
                (&format!("l:{}", lists["hidden"].0), "inherit"),
            ],
        )
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    Fixture {
        env,
        owner,
        agent,
        token,
        lists,
    }
}

impl Fixture {
    fn hidden(&self) -> (&str, &str) {
        let (l, t) = &self.lists["hidden"];
        (l, t)
    }

    async fn get(&self, path: &str) -> Response {
        self.env.api(Method::GET, path, &self.token, None).await
    }
}

fn names(v: &Value) -> BTreeSet<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            x.get("name")
                .or_else(|| x.get("title"))
                .and_then(Value::as_str)
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[tokio::test]
async fn agents_read_only_what_they_are_granted() {
    let f = fixture("").await;
    let (hidden_list, hidden_task) = f.hidden();

    let r = f.get("/api/v1/lists").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(names(&r.json), set(&["read", "capture", "plan"]));

    let r = f.get(&format!("/api/v1/lists/{hidden_list}/tasks")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = f.get(&format!("/api/v1/tasks/{hidden_task}")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = f.get(&format!("/api/v1/tasks/{}", f.lists["read"].1)).await;
    assert_eq!(r.status, StatusCode::OK);

    let r = f.get("/api/v1/tasks?query=").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(
        names(&r.json),
        set(&["read task", "capture task", "plan task"])
    );
    let r = f.get("/api/v1/tasks?query=list%3Ahidden").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json["items"].as_array().unwrap().is_empty());
    let r = f.get("/api/v1/smart-lists/my_day/tasks").await;
    assert_eq!(r.status, StatusCode::OK);

    // The event log and activity feed leave out everything about the hidden list.
    let r = f.get("/api/v1/events?since=0").await;
    assert_eq!(r.status, StatusCode::OK);
    let events = r.json["events"].as_array().unwrap();
    assert_eq!(events.len(), 6, "three lists and three tasks");
    let text = r.json.to_string();
    assert!(!text.contains(hidden_list) && !text.contains(hidden_task));
    let r = f.get("/api/v1/activity").await;
    assert_eq!(r.status, StatusCode::OK);
    let text = r.json.to_string();
    assert!(!text.contains(hidden_list) && !text.contains(hidden_task));
    assert!(!text.contains("hidden task"));

    // The owner still sees all four.
    let (s, v) = f.env.t.get("/api/v1/lists").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(names(&v).len(), 4);
}

#[tokio::test]
async fn agents_write_and_undo_only_within_their_levels() {
    let f = fixture("").await;
    let env = &f.env;
    let (hidden_list, hidden_task) = f.hidden();
    let (read_list, read_task) = f.lists["read"].clone();
    let (capture_list, capture_task) = f.lists["capture"].clone();
    let (_, plan_task) = f.lists["plan"].clone();

    // Creating tasks needs capture.
    let (r, _) = create_task(env, &f.token, &read_list, "nope").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.json);
    let (r, _) = create_task(env, &f.token, hidden_list, "nope").await;
    assert!(
        matches!(r.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{}",
        r.status
    );
    let (r, _) = create_task(env, &f.token, &capture_list, "captured").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(
        actor_of(env, first_seq(&r.json)).await,
        json!({ "type": "agent", "agent_id": "scout" })
    );

    // Completing needs plan.
    for task in [&read_task, &capture_task, &hidden_task.to_owned()] {
        let r = env
            .command(&f.token, json!({ "cmd": "complete_task", "id": task }))
            .await;
        assert!(
            matches!(r.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{task}: {}",
            r.json
        );
    }
    let r = env
        .command(&f.token, json!({ "cmd": "complete_task", "id": plan_task }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let own = first_seq(&r.json);

    // Structure is owner-only.
    let r = env
        .command(
            &f.token,
            json!({ "cmd": "create_list", "id": id(), "name": "mine" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Moving a task into a list it can't write to.
    let r = env
        .command(
            &f.token,
            json!({ "cmd": "move_task", "id": plan_task, "list": read_list }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.json);

    // Undo: its own change in a plan list yes; the owner's change to the hidden list no.
    let hidden_seq = {
        let r = env
            .api(Method::GET, "/api/v1/events?since=0", TOKEN_A, None)
            .await;
        r.json["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e.to_string().contains(hidden_task))
            .unwrap()["seq"]
            .as_u64()
            .unwrap()
    };
    let r = env.undo(&f.token, hidden_seq).await;
    assert!(
        matches!(r.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "{}",
        r.json
    );
    // A wrong expected ID is refused without naming the event that is there.
    let real = env.event_id(hidden_seq).await;
    let r = env
        .undo_expecting(
            &f.token,
            hidden_seq,
            &todo_core::ids::EventId::new_v7(1, [0; 10]).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "event_mismatch");
    assert!(
        r.json["error"]["details"].get("found").is_none(),
        "{}",
        r.json
    );
    assert!(!r.json.to_string().contains(&real), "{}", r.json);
    // The owner still gets it.
    let r = env
        .undo_expecting(
            TOKEN_A,
            hidden_seq,
            &todo_core::ids::EventId::new_v7(1, [0; 10]).to_string(),
        )
        .await;
    assert_eq!(
        r.json["error"]["details"]["found"],
        real.as_str(),
        "{}",
        r.json
    );
    let r = env.undo(&f.token, own).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
}

#[tokio::test]
async fn agent_event_streams_carry_only_visible_events() {
    let f = fixture("").await;
    let env = &f.env;
    let head = env.t.server.engine.head_seq();
    let mut stream = env.t.sse(&f.token, Some(head)).await;
    let (hidden_list, _) = f.hidden();
    let (r, _) = create_task(env, TOKEN_A, hidden_list, "secret plans").await;
    assert_eq!(r.status, StatusCode::OK);
    let (r, visible) = create_task(env, TOKEN_A, &f.lists["read"].0, "visible").await;
    assert_eq!(r.status, StatusCode::OK);
    let frame = stream.next_event(Duration::from_secs(5)).await.unwrap();
    let ev = frame.json();
    assert!(ev.to_string().contains(&visible), "{ev}");
    assert!(!ev.to_string().contains("secret plans"));

    // A replay from the start leaves the hidden events out as well.
    let mut replay = env.t.sse(&f.token, Some(0)).await;
    let mut seen = 0;
    while let Some(frame) = replay.next_event(Duration::from_millis(500)).await {
        assert!(!frame.data.contains(hidden_list), "{}", frame.data);
        seen += 1;
    }
    assert_eq!(seen, 7);

    // Changing grants applies at once: hide the read list.
    let mut owner = Browser::new(&env.t.url);
    owner.cookie = f.owner.cookie.clone();
    owner.csrf = f.owner.csrf.clone();
    let p = owner
        .post_form(
            &format!("/admin/agents/{}/grants", f.agent),
            &[("default", "none")],
        )
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let r = f.get("/api/v1/lists").await;
    assert!(r.json["items"].as_array().unwrap().is_empty(), "{}", r.json);
    let (r, _) = create_task(env, TOKEN_A, &f.lists["read"].0, "now hidden").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        stream
            .next_event(Duration::from_millis(500))
            .await
            .is_none()
    );
}

/// Everything an agent can learn about history: JSON event pages, an SSE replay, the activity feed.
async fn agent_history(f: &Fixture) -> String {
    let mut out = String::new();
    let r = f.get("/api/v1/events?since=0&limit=1000").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    out.push_str(&r.json.to_string());
    let r = f.get("/api/v1/activity?limit=200").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    out.push_str(&r.json.to_string());
    let mut replay = f.env.t.sse(&f.token, Some(0)).await;
    while let Some(frame) = replay.next_event(Duration::from_millis(500)).await {
        out.push_str(&frame.data);
    }
    out
}

impl Fixture {
    async fn restart_with(self, f: impl FnOnce(&std::path::Path)) -> Self {
        let Self {
            env,
            owner,
            agent,
            token,
            lists,
        } = self;
        Self {
            env: env.restart_with(f).await,
            owner,
            agent,
            token,
            lists,
        }
    }
}

#[tokio::test]
async fn history_from_a_private_list_stays_hidden_after_a_task_moves_out() {
    let f = fixture("").await;
    let (hidden_list, hidden_task) = f.hidden();
    let (hidden_list, hidden_task) = (hidden_list.to_owned(), hidden_task.to_owned());
    let read_list = f.lists["read"].0.clone();
    let step = id();
    for c in [
        json!({ "cmd": "update_task", "id": hidden_task, "patch": { "notes": "NOTES-SECRET" } }),
        json!({ "cmd": "update_task", "id": hidden_task, "patch": { "notes": "" } }),
        json!({ "cmd": "update_task", "id": hidden_task, "patch": { "title": "TITLE-SECRET" } }),
        json!({ "cmd": "update_task", "id": hidden_task, "patch": { "title": "Harmless" } }),
        json!({ "cmd": "create_step", "id": step, "task": hidden_task, "title": "STEP-SECRET" }),
        json!({ "cmd": "update_step", "id": step, "title": "fine step" }),
        json!({ "cmd": "add_link", "id": id(), "task": hidden_task,
                "kind": { "type": "web", "url": "https://example.com/LINK-SECRET", "title": null } }),
        json!({ "cmd": "move_task", "id": hidden_task, "list": read_list }),
    ] {
        let r = f.env.command(TOKEN_A, c.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{c}: {}", r.json);
    }
    // The task is readable now, as it is today.
    let r = f.get(&format!("/api/v1/tasks/{hidden_task}")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    assert_eq!(r.json["task"]["title"], "Harmless");
    // But nothing written while it was in the private list shows through the log.
    let history = agent_history(&f).await;
    for leak in [
        "NOTES-SECRET",
        "TITLE-SECRET",
        "STEP-SECRET",
        "LINK-SECRET",
        "hidden task",
        hidden_list.as_str(),
    ] {
        assert!(!history.contains(leak), "{leak} leaked");
    }
    // Changes after the move are visible.
    let r = f
        .env
        .command(
            TOKEN_A,
            json!({ "cmd": "update_task", "id": hidden_task, "patch": { "title": "Renamed in view" } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(agent_history(&f).await.contains("Renamed in view"));

    // The scopes are stored: the same holds after a restart.
    let f = f.restart_with(|_| {}).await;
    let history = agent_history(&f).await;
    assert!(!history.contains("NOTES-SECRET") && !history.contains("STEP-SECRET"));
    assert!(history.contains("Renamed in view"));
}

#[tokio::test]
async fn history_stays_hidden_after_a_list_moves_into_a_readable_collection() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let (private_col, open_col) = (id(), id());
    for (c, name) in [(&private_col, "Private"), (&open_col, "Shared")] {
        let r = env
            .command(
                TOKEN_A,
                json!({ "cmd": "create_collection", "id": c, "name": name }),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK);
    }
    let list = id();
    let r = env
        .command(
            TOKEN_A,
            json!({ "cmd": "create_list", "id": list, "name": "LIST-SECRET", "collection": private_col }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let (r, task) = create_task(&env, TOKEN_A, &list, "TASK-SECRET").await;
    assert_eq!(r.status, StatusCode::OK);
    for c in [
        json!({ "cmd": "update_task", "id": task, "patch": { "title": "Plain" } }),
        json!({ "cmd": "update_list", "id": list, "patch": { "name": "Plain list" } }),
        json!({ "cmd": "move_list", "id": list, "collection": open_col }),
    ] {
        let r = env.command(TOKEN_A, c.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{c}: {}", r.json);
    }
    let (agent, token) = owner.create_agent("scout", "none", "").await;
    let p = owner
        .post_form(
            &format!("/admin/agents/{agent}/grants"),
            &[("default", "none"), (&format!("c:{open_col}"), "read")],
        )
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let f = Fixture {
        env,
        owner,
        agent,
        token,
        lists: std::collections::BTreeMap::new(),
    };
    let r = f.get(&format!("/api/v1/tasks/{task}")).await;
    assert_eq!(r.status, StatusCode::OK, "the list is readable today");
    let history = agent_history(&f).await;
    for leak in ["LIST-SECRET", "TASK-SECRET", private_col.as_str()] {
        assert!(!history.contains(leak), "{leak} leaked");
    }
}

#[tokio::test]
async fn scopes_are_backfilled_for_logs_written_before_they_existed() {
    let f = fixture("").await;
    let (_, hidden_task) = f.hidden();
    let hidden_task = hidden_task.to_owned();
    let read_list = f.lists["read"].0.clone();
    for c in [
        json!({ "cmd": "update_task", "id": hidden_task, "patch": { "notes": "OLD-SECRET" } }),
        json!({ "cmd": "move_task", "id": hidden_task, "list": read_list }),
    ] {
        let r = f.env.command(TOKEN_A, c.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{c}: {}", r.json);
    }
    // Forget every scope, as in a database written before the migration.
    let f = f
        .restart_with(|db| {
            let conn = rusqlite::Connection::open(db).unwrap();
            conn.execute_batch(
                "DROP TRIGGER event_scopes_append_only_delete; DELETE FROM event_scopes;",
            )
            .unwrap();
        })
        .await;
    let history = agent_history(&f).await;
    assert!(!history.contains("OLD-SECRET"), "backfilled scopes hide it");
    assert!(
        history.contains("read task"),
        "readable history is still there"
    );
    let conn = rusqlite::Connection::open(f.env.t.db_path()).unwrap();
    let missing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE seq NOT IN (SELECT seq FROM event_scopes)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(missing, 0);
}

#[tokio::test]
async fn agents_cannot_manage_tokens_devices_shares_or_grants() {
    let f = fixture("").await;
    // Agent tokens are bearer tokens: no session, so every admin page and action is closed.
    for (method, path) in [
        (Method::GET, "/admin"),
        (Method::GET, "/admin/agents"),
        (Method::POST, "/admin/agents"),
        (Method::POST, "/admin/shares"),
        (Method::POST, "/pair/approve"),
    ] {
        let res = f
            .env
            .t
            .client
            .request(method.clone(), format!("{}{path}", f.env.t.url))
            .header("authorization", format!("Bearer {}", f.token))
            .header(ORIGIN, SITE)
            .send()
            .await
            .unwrap();
        assert!(
            matches!(
                res.status(),
                StatusCode::UNAUTHORIZED | StatusCode::SEE_OTHER | StatusCode::FORBIDDEN
            ) || res.url().path() == "/login",
            "{method} {path}: {}",
            res.status()
        );
    }
}

#[tokio::test]
async fn the_write_quota_applies_at_once_and_counts_undos() {
    let f = fixture("3").await;
    let env = &f.env;
    let capture = &f.lists["capture"].0;
    let mut last = 0;
    for i in 0..3 {
        let (r, _) = create_task(env, &f.token, capture, &format!("t{i}")).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.json);
        last = first_seq(&r.json);
    }
    let (r, _) = create_task(env, &f.token, capture, "one too many").await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "quota_exceeded");
    assert!(r.headers.get(RETRY_AFTER).is_some());
    let r = env.undo(&f.token, last).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // Reads are not writes; the owner is not limited.
    assert_eq!(f.get("/api/v1/lists").await.status, StatusCode::OK);
    let (r, _) = create_task(env, TOKEN_A, capture, "owner").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(env.t.server.engine.head_seq(), last + 1);
}

#[tokio::test]
async fn revoking_an_agent_token_is_immediate_for_the_api_and_open_streams() {
    let f = fixture("").await;
    let mut owner = Browser::new(&f.env.t.url);
    owner.cookie = f.owner.cookie.clone();
    owner.csrf = f.owner.csrf.clone();
    let mut stream = f.env.t.sse(&f.token, None).await;
    assert_eq!(f.get("/api/v1/lists").await.status, StatusCode::OK);

    let p = owner
        .post_form(&format!("/admin/agents/{}/revoke", f.agent), &[])
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    assert_eq!(
        f.get("/api/v1/lists").await.status,
        StatusCode::UNAUTHORIZED
    );
    let (r, _) = create_task(&f.env, &f.token, &f.lists["capture"].0, "after").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(
        stream.ended(Duration::from_secs(5)).await,
        "the stream stayed open"
    );

    // The name is free again; the old token stays dead.
    owner.refresh_csrf().await;
    let (_, token) = owner.create_agent("scout", "read", "").await;
    assert_ne!(token, f.token);
    assert_eq!(
        f.env
            .api(Method::GET, "/api/v1/lists", &token, None)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        f.get("/api/v1/lists").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn agent_tokens_expire() {
    let env = Env::start_with(|c| c.limits.sse_heartbeat_ms = 100).await;
    let owner = env.owner().await;
    let p = owner
        .post_form(
            "/admin/agents",
            &[
                ("name", "brief"),
                ("default", "read"),
                ("writes_per_hour", ""),
                ("expires_in_days", "1"),
            ],
        )
        .await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    let token = attr_after(&p.body, SECRET_VALUE).unwrap().to_owned();
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", &token, None)
            .await
            .status,
        StatusCode::OK
    );
    let mut stream = env.t.sse(&token, None).await;
    env.advance(Duration::from_secs(86_400 + 1));
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", &token, None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert!(stream.ended(Duration::from_secs(5)).await);
}

#[tokio::test]
async fn config_owner_tokens_are_off_unless_allowed() {
    let t = TestServer::start_with(|c| c.auth.allow_config_owner_tokens = false).await;
    let (s, _) = t.get("/api/v1/lists").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(!Config::default().auth.allow_config_owner_tokens);
}

// ---- share links ----------------------------------------------------------------------------------

#[tokio::test]
async fn share_pages_show_titles_completion_due_dates_and_steps_only() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let list = create_list(&env, "Groceries & <stuff>").await;
    let other = create_list(&env, "Private").await;
    let (_, milk) = create_task(&env, TOKEN_A, &list, "Milk").await;
    let (_, bread) = create_task(&env, TOKEN_A, &list, "Bread").await;
    let (_, gone) = create_task(&env, TOKEN_A, &list, "Deleted thing").await;
    let tag = id();
    create_task(&env, TOKEN_A, &other, "Other list task").await;
    for c in [
        json!({ "cmd": "update_task", "id": milk, "patch": { "due": "2026-12-01" } }),
        json!({ "cmd": "complete_task", "id": bread }),
        json!({ "cmd": "create_step", "id": id(), "task": milk, "title": "Oat, not cow" }),
        json!({ "cmd": "add_note", "task": milk, "text": "NOTE-SECRET" }),
        json!({ "cmd": "add_link", "id": id(), "task": milk,
                "kind": { "type": "web", "url": "https://example.com/LINK-SECRET", "title": "LINK-TITLE" } }),
        json!({ "cmd": "create_tag", "id": tag, "name": "TAG-SECRET" }),
        json!({ "cmd": "add_tag", "task": milk, "tag": tag }),
        json!({ "cmd": "delete", "id": { "kind": "task", "id": gone } }),
    ] {
        let r = env.command(TOKEN_A, c.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{c}: {}", r.json);
    }

    let p = owner
        .post_form("/admin/shares", &[("target", &format!("list:{list}"))])
        .await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    let link = attr_after(&p.body, SECRET_VALUE).unwrap().to_owned();
    assert!(link.starts_with(&format!("{SITE}/s/")), "{link}");
    let path = link.strip_prefix(SITE).unwrap().to_owned();
    let token = path.strip_prefix("/s/").unwrap().to_owned();
    assert_eq!(token.len(), 22, "128 bits: {token}");
    // The link is shown once: the list page doesn't have it.
    assert!(!owner.get("/admin/shares").await.body.contains(&token));

    let anon = env.browser();
    let page = anon.get(&path).await;
    assert_eq!(page.status, StatusCode::OK);
    let body = &page.body;
    for want in [
        "Groceries &amp; &lt;stuff&gt;",
        "Milk",
        "Bread",
        "2026-12-01",
        "Oat, not cow",
    ] {
        assert!(body.contains(want), "{want} missing:\n{body}");
    }
    for leak in [
        "NOTE-SECRET",
        "LINK-SECRET",
        "LINK-TITLE",
        "TAG-SECRET",
        "Deleted thing",
        "Private",
        "Other list task",
        &milk,
        &list,
        "<script",
        "<form",
        "agent",
        "owner",
    ] {
        assert!(!body.contains(leak), "{leak} leaked:\n{body}");
    }
    assert_eq!(
        page.header(CONTENT_SECURITY_POLICY),
        todo_server::web::share::SHARE_CSP
    );
    assert!(
        page.header(CONTENT_SECURITY_POLICY)
            .starts_with("default-src 'none'")
    );
    assert_eq!(page.header("x-robots-tag"), "noindex, nofollow");
    assert!(body.contains(r#"<meta name="robots" content="noindex, nofollow">"#));
    assert_eq!(page.header(CACHE_CONTROL), "no-store");
    assert_eq!(page.header(REFERRER_POLICY), "no-referrer");
    assert_eq!(page.header(X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert_eq!(page.header(X_FRAME_OPTIONS), "DENY");
    assert_eq!(
        page.header(STRICT_TRANSPORT_SECURITY),
        todo_server::web::HSTS
    );
    assert!(page.headers.get(SET_COOKIE).is_none());
    let header_text = format!("{:?}", page.headers);
    assert!(!header_text.contains(&list) && !header_text.contains("Groceries"));

    // Unknown, rotated-away and revoked tokens all get the same 404.
    let unknown = anon.get("/s/AAAAAAAAAAAAAAAAAAAAAA").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let shares = owner.get("/admin/shares").await;
    let share_id = attr_after(&shares.body, r#"action="/admin/shares/"#)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    let p = owner
        .post_form(&format!("/admin/shares/{share_id}/rotate"), &[])
        .await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    let rotated = attr_after(&p.body, SECRET_VALUE).unwrap().to_owned();
    assert_ne!(rotated, link);
    let old = anon.get(&path).await;
    assert_eq!((old.status, &old.body), (unknown.status, &unknown.body));
    let new_path = rotated.strip_prefix(SITE).unwrap().to_owned();
    assert_eq!(anon.get(&new_path).await.status, StatusCode::OK);
    let p = owner
        .post_form(&format!("/admin/shares/{share_id}/revoke"), &[])
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let revoked = anon.get(&new_path).await;
    assert_eq!(
        (revoked.status, &revoked.body),
        (unknown.status, &unknown.body)
    );
}

#[tokio::test]
async fn collection_shares_show_its_lists() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let col = id();
    let r = env
        .command(
            TOKEN_A,
            json!({ "cmd": "create_collection", "id": col, "name": "Home" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let inside = id();
    let r = env
        .command(
            TOKEN_A,
            json!({ "cmd": "create_list", "id": inside, "name": "Garden", "collection": col }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let outside = create_list(&env, "Work").await;
    create_task(&env, TOKEN_A, &inside, "Mow").await;
    create_task(&env, TOKEN_A, &outside, "Report").await;
    let p = owner
        .post_form("/admin/shares", &[("target", &format!("collection:{col}"))])
        .await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    let link = attr_after(&p.body, SECRET_VALUE).unwrap().to_owned();
    let page = env.browser().get(link.strip_prefix(SITE).unwrap()).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body.contains("Home") && page.body.contains("Garden") && page.body.contains("Mow")
    );
    assert!(!page.body.contains("Work") && !page.body.contains("Report"));
}

#[tokio::test]
async fn share_requests_are_rate_limited_per_address_before_the_lookup() {
    let env = Env::start_with(|c| {
        c.trust_cloudflare_headers = true;
        c.auth.share_requests_per_minute = 1;
        c.auth.share_request_burst = 3;
    })
    .await;
    let owner = env.owner().await;
    let list = create_list(&env, "Shared").await;
    let p = owner
        .post_form("/admin/shares", &[("target", &format!("list:{list}"))])
        .await;
    let link = attr_after(&p.body, SECRET_VALUE).unwrap().to_owned();
    let path = link.strip_prefix(SITE).unwrap().to_owned();

    // Guessing tokens spends the same budget as real ones: the limit comes before the lookup.
    let guesser = browser_at(&env, "203.0.113.7");
    let mut statuses = Vec::new();
    for _ in 0..3 {
        statuses.push(guesser.get("/s/AAAAAAAAAAAAAAAAAAAAAA").await.status);
    }
    assert_eq!(statuses, [StatusCode::NOT_FOUND; 3]);
    let limited = guesser.get(&path).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header(RETRY_AFTER).parse::<u32>().unwrap() >= 1);
    assert!(!limited.body.contains("Shared"));
    assert_eq!(
        limited.header(CONTENT_SECURITY_POLICY),
        todo_server::web::share::SHARE_CSP
    );
    // Malformed tokens are turned away without a lookup, but still count.
    let other = browser_at(&env, "203.0.113.9");
    assert_eq!(
        other.get(&format!("/s/{}", "x".repeat(200))).await.status,
        StatusCode::NOT_FOUND
    );
    // Another address has its own budget.
    assert_eq!(other.get(&path).await.status, StatusCode::OK);
}

// ---- headers and the allowlist --------------------------------------------------------------------

#[tokio::test]
async fn pages_have_strict_security_headers() {
    let env = Env::start().await;
    let owner = env.owner().await;
    for (b, path) in [
        (&owner, "/admin"),
        (&owner, "/admin/agents"),
        (&env.browser(), "/login"),
    ] {
        let p = b.get(path).await;
        assert_eq!(p.status, StatusCode::OK, "{path}");
        assert_eq!(
            p.header(CONTENT_SECURITY_POLICY),
            todo_server::web::CSP,
            "{path}"
        );
        for part in [
            "default-src 'self'",
            "frame-ancestors 'none'",
            "base-uri 'none'",
            "form-action 'self'",
        ] {
            assert!(p.header(CONTENT_SECURITY_POLICY).contains(part));
        }
        assert_eq!(p.header(X_CONTENT_TYPE_OPTIONS), "nosniff");
        assert_eq!(p.header(REFERRER_POLICY), "no-referrer");
        assert_eq!(p.header(CACHE_CONTROL), "no-store", "{path}");
        assert_eq!(
            p.header(STRICT_TRANSPORT_SECURITY),
            todo_server::web::HSTS,
            "{path}"
        );
        // No external resources.
        for bad in ["http://", "https://", "//cdn"] {
            assert!(
                !p.body.contains(&format!("src=\"{bad}"))
                    && !p.body.contains(&format!("href=\"{bad}")),
                "{path} loads {bad}"
            );
        }
    }
    let js = env.browser().get("/assets/auth.js").await;
    assert_eq!(js.status, StatusCode::OK);
    assert!(js.header(CONTENT_TYPE).starts_with("text/javascript"));
    assert_eq!(js.header(X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert!(!js.body.contains("http://") && !js.body.contains("https://"));
    assert_eq!(
        env.browser().get("/assets/nope.js").await.status,
        StatusCode::NOT_FOUND
    );
}

/// Exactly these routes answer without a session or token.
const PUBLIC: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("POST", "/api/v1/pair/start"),
    ("POST", "/api/v1/pair/poll"),
    ("GET", "/login"),
    ("GET", "/setup"),
    ("GET", "/pair"),
    ("POST", "/auth/login/start"),
    ("POST", "/auth/login/finish"),
    ("POST", "/auth/setup/start"),
    ("POST", "/auth/setup/finish"),
    ("GET", "/assets/{file}"),
    ("GET", "/s/{token}"),
];

fn concrete(path: &str) -> String {
    path.replace("{id}", "1")
        .replace("{file}", "app.css")
        .replace("{token}", "AAAAAAAAAAAAAAAAAAAAAA")
}

#[tokio::test]
async fn the_unauthenticated_allowlist_is_exact() {
    let public: BTreeSet<(String, String)> =
        todo_server::app::public_routes().into_iter().collect();
    let expected: BTreeSet<(String, String)> = PUBLIC
        .iter()
        .map(|(m, p)| ((*m).to_owned(), (*p).to_owned()))
        .collect();
    assert_eq!(public, expected);

    let env = Env::start().await;
    let anon = env.browser();
    // Every other web route turns an anonymous visitor away: page loads go to sign-in, everything
    // else gets 401 (or 403 first, without the Origin).
    let routes = todo_server::web::routes();
    assert!(routes.len() > PUBLIC.len());
    for (method, path, gate) in &routes {
        let key = (method.to_string(), (*path).to_owned());
        assert_eq!(*gate == Gate::Public, expected.contains(&key), "{key:?}");
        if *gate == Gate::Public {
            continue;
        }
        let url = concrete(path);
        if *method == Method::GET {
            let p = anon.get(&url).await;
            assert_eq!(p.status, StatusCode::SEE_OTHER, "GET {path}");
            assert!(p.location().starts_with("/login?next="), "GET {path}");
        } else {
            let p = anon
                .send(method.clone(), &url, Some(SITE), Body::Form(String::new()))
                .await;
            assert_eq!(
                p.status,
                StatusCode::UNAUTHORIZED,
                "{method} {path}: {}",
                p.body
            );
            let p = anon.send(method.clone(), &url, None, Body::None).await;
            assert!(
                matches!(p.status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED),
                "{method} {path}: {}",
                p.status
            );
        }
    }

    // The public ones answer without credentials (with something other than "sign in first").
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
                let mut req = anon.client.post(format!("{}{url}", env.t.url));
                req = req.header(ORIGIN, SITE).json(&body);
                let res = req.send().await.unwrap();
                Page {
                    status: res.status(),
                    headers: res.headers().clone(),
                    body: res.text().await.unwrap(),
                }
            }
        };
        assert_ne!(
            p.status,
            StatusCode::UNAUTHORIZED,
            "{method} {path}: {}",
            p.body
        );
        assert!(!p.location().starts_with("/login"), "{method} {path}");
        assert_ne!(p.status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
    }

    // Unknown paths outside the API are 404, not an open door.
    assert_eq!(anon.get("/admin/nope").await.status, StatusCode::NOT_FOUND);
    // API routes need a token (the full list is checked in tests/http.rs).
    for path in ["/api/v1/lists", "/api/v1/events", "/api/v1/openapi.json"] {
        assert_eq!(
            anon.get(path).await.status,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
}
