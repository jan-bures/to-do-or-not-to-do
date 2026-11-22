//! The web harness: a server with a public origin, a movable clock and the setup code captured, and a
//! browser with a cookie jar of one, CSRF handling and a software passkey.

use super::{TOKEN_A, TestServer, id};
use reqwest::header::{CONTENT_TYPE, COOKIE, HeaderMap, LOCATION, ORIGIN, SET_COOKIE};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use todo_core::time::Timestamp;
use todo_server::{Config, Options};
use webauthn_authenticator_rs::WebauthnAuthenticator;
use webauthn_authenticator_rs::prelude::Url;
use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse};

/// The public origin the tests configure (a placeholder; nothing resolves it).
pub const SITE: &str = "https://todo.example.com";

pub const MINUTE: Duration = Duration::from_secs(60);

/// What precedes a value shown once (an agent token or a share link).
pub const SECRET_VALUE: &str = r#"<input class="secret" readonly value=""#;

// ---- harness --------------------------------------------------------------------------------------

/// A server with `public_origin` set, a clock the test can move forward, and the setup code captured.
pub struct Env {
    pub t: TestServer,
    pub offset: Arc<AtomicI64>,
    pub code: Arc<Mutex<Option<String>>>,
}

impl Env {
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    /// Generous per-IP limits, so only the rate-limit tests hit them; `f` can change anything.
    pub async fn start_with(f: impl FnOnce(&mut Config)) -> Self {
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

    pub fn advance(&self, by: Duration) {
        self.offset
            .fetch_add(i64::try_from(by.as_millis()).unwrap(), Ordering::SeqCst);
    }

    /// Restarts the server on the same data directory (with the same clock and code sink).
    pub async fn restart(self) -> Self {
        self.restart_with(|_| {}).await
    }

    /// Restarts, running `f` on the stopped server's database file first.
    pub async fn restart_with(self, f: impl FnOnce(&std::path::Path)) -> Self {
        let Self { t, offset, code } = self;
        Self {
            t: t.restart_with(f).await,
            offset,
            code,
        }
    }

    pub fn take_code(&self) -> Option<String> {
        self.code.lock().unwrap().take()
    }

    pub fn browser(&self) -> Browser {
        Browser::new(&self.t.url)
    }

    /// Completes setup and returns the signed-in owner's browser.
    pub async fn owner(&self) -> Browser {
        let mut b = self.browser();
        let code = self.take_code().expect("a setup code");
        let p = b.setup(&code).await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
        b
    }

    /// Owner-token API call (the config fallback the shared helpers use).
    pub async fn api(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Response {
        let (status, headers, json) = self.t.request(method, path, Some(token), body).await;
        Response {
            status,
            headers,
            json,
        }
    }

    pub async fn command(&self, token: &str, command: Value) -> Response {
        self.api(
            Method::POST,
            "/api/v1/commands",
            token,
            Some(json!({ "command_id": id(), "command": command })),
        )
        .await
    }

    /// The ID of the event at `seq` (read with an owner token).
    pub async fn event_id(&self, seq: u64) -> String {
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

    pub async fn undo(&self, token: &str, seq: u64) -> Response {
        let expected = self.event_id(seq).await;
        self.undo_expecting(token, seq, &expected).await
    }

    pub async fn undo_expecting(&self, token: &str, seq: u64, expected: &str) -> Response {
        self.api(
            Method::POST,
            &format!("/api/v1/events/{seq}/undo"),
            token,
            Some(json!({ "command_id": id(), "expected_event_id": expected })),
        )
        .await
    }
}

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub json: Value,
}

/// One HTTP response from the web routes.
#[derive(Debug)]
pub struct Page {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Page {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    pub fn header(&self, name: impl reqwest::header::AsHeaderName) -> &str {
        self.headers
            .get(name)
            .map_or("", |v| v.to_str().unwrap_or(""))
    }

    pub fn location(&self) -> &str {
        self.header(LOCATION)
    }

    pub fn set_cookie(&self) -> &str {
        self.header(SET_COOKIE)
    }
}

pub fn form_encode(fields: &[(&str, &str)]) -> String {
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
pub fn attr_after<'a>(body: &'a str, start: &str) -> Option<&'a str> {
    let i = body.find(start)? + start.len();
    let rest = &body[i..];
    Some(&rest[..rest.find('"')?])
}

/// A browser: no automatic redirects, a cookie jar of one, the CSRF token of the last page seen, and
/// a software passkey.
pub struct Browser {
    pub client: reqwest::Client,
    pub url: String,
    pub cookie: Option<String>,
    pub csrf: Option<String>,
    pub key: SoftPasskey,
    /// Sent with every request (e.g. `CF-Connecting-IP`, to act as another client address).
    pub extra: Vec<(&'static str, String)>,
    /// Every `Set-Cookie` header this browser has received.
    pub set_cookies: Mutex<Vec<String>>,
}

pub enum Body {
    None,
    Json(Value),
    Form(String),
}

impl Browser {
    pub fn new(url: &str) -> Self {
        Self {
            client: super::http()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            url: url.to_owned(),
            cookie: None,
            csrf: None,
            // `true`: the software key reports user verification (it has no PIN or biometric).
            key: SoftPasskey::new(true),
            extra: Vec::new(),
            set_cookies: Mutex::new(Vec::new()),
        }
    }

    pub fn site() -> Url {
        Url::parse(SITE).unwrap()
    }

    pub async fn send(&self, method: Method, path: &str, origin: Option<&str>, body: Body) -> Page {
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
        self.set_cookies.lock().unwrap().extend(
            res.headers()
                .get_all(SET_COOKIE)
                .iter()
                .map(|v| v.to_str().unwrap_or("<not text>").to_owned()),
        );
        Page {
            status: res.status(),
            headers: res.headers().clone(),
            body: res.text().await.unwrap(),
        }
    }

    pub async fn get(&self, path: &str) -> Page {
        self.send(Method::GET, path, None, Body::None).await
    }

    pub async fn post_json(&self, path: &str, body: Value) -> Page {
        self.send(Method::POST, path, Some(SITE), Body::Json(body))
            .await
    }

    /// Posts a form from this site's own page: the right `Origin` and the session's CSRF token.
    pub async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Page {
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
    pub async fn adopt(&mut self, p: &Page) {
        let c = p.set_cookie();
        let value = c
            .strip_prefix("__Host-todo_session=")
            .and_then(|r| r.split(';').next())
            .expect("a session cookie");
        self.cookie = Some(value.to_owned());
        self.refresh_csrf().await;
    }

    pub async fn refresh_csrf(&mut self) {
        let p = self.get("/admin").await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
        self.csrf = Some(
            attr_after(&p.body, r#"<meta name="csrf-token" content=""#)
                .unwrap()
                .to_owned(),
        );
    }

    pub fn register(&mut self, options: &Value) -> Value {
        let ccr: CreationChallengeResponse = serde_json::from_value(options.clone()).unwrap();
        let cred = self.key.do_registration(Self::site(), ccr).unwrap();
        serde_json::to_value(cred).unwrap()
    }

    pub fn assert(&mut self, options: &Value) -> Value {
        let rcr: RequestChallengeResponse = serde_json::from_value(options.clone()).unwrap();
        let cred = self.key.do_authentication(Self::site(), rcr).unwrap();
        serde_json::to_value(cred).unwrap()
    }

    /// Runs the setup ceremony; on success the browser is signed in.
    pub async fn setup(&mut self, code: &str) -> Page {
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
    pub async fn login_assertion(&mut self) -> (Value, Value) {
        let p = self.post_json("/auth/login/start", json!({})).await;
        assert_eq!(p.status, StatusCode::OK, "{}", p.body);
        let v = p.json();
        let credential = self.assert(&v["options"]);
        (v["ceremony"].clone(), credential)
    }

    pub async fn login(&mut self) -> Page {
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

    pub async fn step_up(&mut self) {
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
    pub async fn create_agent(
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

pub fn assert_session_cookie(set_cookie: &str) {
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
