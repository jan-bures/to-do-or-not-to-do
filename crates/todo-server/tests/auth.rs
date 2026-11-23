//! Authentication and authorization end to end, against an in-process server: owner setup, passkey
//! sign-in and step-up (driven by a software passkey), browser sessions and CSRF, device pairing,
//! agent tokens and their access levels, write quotas, revocation, share pages and rate limits. The
//! route allowlist, security headers and cookie attributes are checked in `tests/security.rs`.
#![allow(clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use common::web::{
    Body, Browser, Env, MINUTE, Response, SECRET_VALUE, SITE, assert_session_cookie, attr_after,
    form_encode,
};
use common::{TOKEN_A, TestServer, first_seq, id};
use reqwest::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, ORIGIN, REFERRER_POLICY, RETRY_AFTER, SET_COOKIE,
    STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::time::Duration;
use todo_server::Config;

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
    assert_eq!(admin.header(CACHE_CONTROL), "no-store, no-transform");

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
        // The button posts to /auth/step-up with the page's CSRF token; without the token in the page,
        // the CSRF check refused it and the button could never work.
        let csrf = b.csrf.clone().expect("signed in");
        assert!(
            p.body
                .contains(&format!(r#"<meta name="csrf-token" content="{csrf}">"#)),
            "{path}: the step-up page must carry the session's CSRF token: {}",
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
    assert!(
        b.get("/admin")
            .await
            .body
            .contains("Changes allowed for about 5 more minutes.")
    );
    let agents = b.get("/admin/agents").await.body;
    assert!(agents.contains("Enabled"));
    assert!(agents.contains("Not used yet"));
    assert!(!agents.contains("Never active"));
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", &token, None)
            .await
            .status,
        StatusCode::OK
    );
    assert!(
        b.get("/admin/agents")
            .await
            .body
            .contains("Last used <time")
    );
    let p = b.post_json("/auth/passkeys/start", json!({})).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    env.advance(5 * MINUTE + Duration::from_secs(1));
    let p = b
        .post_form("/admin/shares", &[("target", &format!("list:{list}"))])
        .await;
    assert_eq!(p.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_audit_log_groups_events_by_utc_day_with_short_times_and_exact_titles() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let svc = &env.t.server.authsvc;
    let day = 86_400_000;
    let midnight = svc.now().div_euclid(day) * day;
    let previous = midnight - 1_000;
    let older = jiff::Timestamp::from_millisecond(midnight - 3 * day).unwrap();
    let store = svc.store();
    store
        .audit(older.as_millisecond(), "fixture_old", None, "{}")
        .unwrap();
    store
        .audit(previous, "fixture_before_midnight", Some("192.0.2.4"), "{}")
        .unwrap();
    store
        .audit(midnight, "fixture_at_midnight", None, "{}")
        .unwrap();

    let audit = owner.get("/admin/audit").await;
    assert_eq!(audit.status, StatusCode::OK);
    let (_, today) = audit.body.split_once(">Today</h2>").unwrap();
    let (today, yesterday) = today.split_once(">Yesterday</h2>").unwrap();
    let (yesterday, older_group) = yesterday.split_once("</section><section").unwrap();
    assert!(today.contains("Fixture at midnight") && today.contains(">00:00</time>"));
    assert!(!today.contains("Fixture before midnight"));
    assert!(yesterday.contains("Fixture before midnight"));
    assert!(yesterday.contains(">23:59</time>"));
    let exact = jiff::Timestamp::from_millisecond(previous).unwrap();
    assert!(yesterday.contains(&format!(
        r#"datetime="{exact}" title="{}""#,
        exact.strftime("%Y-%m-%d 23:59:59 UTC")
    )));
    assert!(yesterday.contains("192.0.2.4"));
    assert!(older_group.contains("Fixture old"));
    assert!(older_group.contains(&older.strftime("%a %d.%m.").to_string()));
    assert!(today.contains(r#"<span class="audit-toggle">Details</span>"#));
}

/// The email allowlist and list keys: owner-only, step-up, audited, and in effect at once.
#[tokio::test]
async fn email_allowlist_inbox_and_list_keys_need_step_up_and_are_audited() {
    const INGEST_SECRET: &str = "test-only-ingest-secret-0123456789abcdef";
    let env = Env::start_with(|cfg| {
        let path = cfg.data_dir.join("ingest.secret");
        let mut f = todo_fs::create_private_file(&path).unwrap();
        std::io::Write::write_all(&mut f, INGEST_SECRET.as_bytes()).unwrap();
        cfg.ingest.email_hmac_secret_file = Some(path);
    })
    .await;
    let mut b = env.owner().await;
    let inbox = create_list(&env, "Inbox").await;
    let errands = create_list(&env, "Errands").await;
    let ingest = |forwarded_by: &str, list: Option<&str>| {
        let body = json!({
            "version": 1, "message_id": format!("<{}@mail.example.org>", id()), "list": list,
            "title": "EMAIL-TITLE", "subject": "EMAIL-SUBJECT", "keep": true,
            "original_sender": { "address": "sender-secret@example.org", "name": null, "source": "from" },
            "forwarded_by": forwarded_by, "received_at": "2025-11-21T08:00:00.000Z",
            "body": { "text": "KEPT-BODY", "source": "text", "truncated": false },
            "raw_truncated": false,
        })
        .to_string();
        // The server's clock, which the test moves forward.
        let ms = todo_core::time::Timestamp::now().as_millisecond()
            + env.offset.load(std::sync::atomic::Ordering::SeqCst);
        let ts = (ms / 1000).to_string();
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let mut m =
            <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(INGEST_SECRET.as_bytes())
                .unwrap();
        hmac::Mac::update(&mut m, format!("{ts}\n{nonce}\n{body}").as_bytes());
        let sig = hex::encode(hmac::Mac::finalize(m).into_bytes());
        let req = env
            .t
            .client
            .post(format!("{}/api/v1/ingest/email", env.t.url))
            .header("x-todo-timestamp", ts)
            .header("x-todo-nonce", nonce)
            .header("x-todo-signature", sig)
            .body(body);
        async move {
            let res = req.send().await.unwrap();
            let status = res.status();
            (status, res.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let list_of = |r: Value| {
        let env = &env;
        async move {
            let d = env
                .api(
                    Method::GET,
                    &format!("/api/v1/tasks/{}", r["task_id"].as_str().unwrap()),
                    TOKEN_A,
                    None,
                )
                .await;
            d.json["task"]["list"].as_str().unwrap().to_owned()
        }
    };
    assert_eq!(
        ingest("you@example.com", None).await.0,
        StatusCode::FORBIDDEN
    );

    // Not fresh: every change needs a passkey check first.
    env.advance(6 * MINUTE);
    let page = b.get("/admin/ingest").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("<strong>on</strong>"), "{}", page.body);
    assert!(page.body.contains("No Inbox is set."), "{}", page.body);
    let attempts: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "/admin/ingest/senders",
            vec![("address", "you@example.com")],
        ),
        (
            "/admin/ingest/senders/remove",
            vec![("address", "you@example.com")],
        ),
        ("/admin/ingest/inbox", vec![("list", inbox.as_str())]),
        (
            "/admin/ingest/keys",
            vec![("key", "shop"), ("list", errands.as_str())],
        ),
        ("/admin/ingest/keys/remove", vec![("key", "shop")]),
    ];
    for (path, fields) in &attempts {
        let p = b.post_form(path, fields).await;
        assert_eq!(p.status, StatusCode::FORBIDDEN, "{path}: {}", p.body);
        assert!(p.body.contains("data-action=\"step-up\""), "{path}");
    }
    // With a wrong CSRF token, even when fresh.
    b.step_up().await;
    for (path, fields) in &attempts {
        let mut form = vec![("csrf", "wrong")];
        form.extend(fields.iter().copied());
        let p = b
            .send(
                Method::POST,
                path,
                Some(SITE),
                Body::Form(form_encode(&form)),
            )
            .await;
        assert_eq!(p.status, StatusCode::FORBIDDEN, "{path}: {}", p.body);
    }

    // Fresh: add a sender (normalized), pick the Inbox and add a list key; all take effect at once.
    let p = b
        .post_form("/admin/ingest/senders", &[("address", " You@Example.com ")])
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let p = b
        .post_form("/admin/ingest/senders", &[("address", "not an address")])
        .await;
    assert_eq!(p.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        ingest("you@example.com", None).await.0,
        StatusCode::SERVICE_UNAVAILABLE,
        "no Inbox yet"
    );
    let p = b
        .post_form("/admin/ingest/inbox", &[("list", inbox.as_str())])
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let p = b
        .post_form(
            "/admin/ingest/keys",
            &[("key", "shop"), ("list", errands.as_str())],
        )
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    for (key, list) in [
        ("shop", errands.as_str()),
        ("inbox", errands.as_str()),
        ("Bad Key", errands.as_str()),
        ("other", "not-a-list"),
        ("other", ""),
    ] {
        let p = b
            .post_form("/admin/ingest/keys", &[("key", key), ("list", list)])
            .await;
        assert_eq!(p.status, StatusCode::UNPROCESSABLE_ENTITY, "{key} {list}");
    }
    let page = b.get("/admin/ingest").await.body;
    assert!(page.contains("<h3>you@example.com</h3>"), "{page}");
    assert!(
        page.contains("The Inbox is <strong>Inbox</strong>."),
        "{page}"
    );
    assert!(
        page.contains(
            "Agents that can read this list see the email's subject, sender and any kept body."
        ),
        "{page}"
    );
    assert!(page.contains("<h3>shop</h3>"), "{page}");
    assert!(page.contains("<span>Errands</span>"), "{page}");
    let (s, r) = ingest("you@example.com", Some("shop")).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(list_of(r).await, errands);
    let (s, r) = ingest("you@example.com", None).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(list_of(r).await, inbox);

    // A share page of the list shows the email task's title only: no subject, sender or body.
    let p = b
        .post_form("/admin/shares", &[("target", &format!("list:{errands}"))])
        .await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    let link = attr_after(&p.body, SECRET_VALUE).unwrap().to_owned();
    let share = env.browser().get(link.strip_prefix(SITE).unwrap()).await;
    assert_eq!(share.status, StatusCode::OK);
    assert!(share.body.contains("EMAIL-TITLE"), "{}", share.body);
    for leak in [
        "EMAIL-SUBJECT",
        "sender-secret",
        "KEPT-BODY",
        "you@example.com",
        "Forwarded",
        "mail.example.org",
    ] {
        assert!(!share.body.contains(leak), "{leak} leaked:\n{}", share.body);
    }

    // The keyed list is deleted: the page flags the key, and its mail goes to the Inbox.
    let r = env
        .command(
            TOKEN_A,
            json!({ "cmd": "delete", "id": { "kind": "list", "id": errands } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let page = b.get("/admin/ingest").await.body;
    assert!(page.contains("<strong>list deleted</strong>"), "{page}");
    let (s, r) = ingest("you@example.com", Some("shop")).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(list_of(r).await, inbox);
    let p = b
        .post_form("/admin/ingest/keys/remove", &[("key", "shop")])
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let p = b
        .post_form("/admin/ingest/keys/remove", &[("key", "shop")])
        .await;
    assert_eq!(p.status, StatusCode::NOT_FOUND);

    // Removing the sender shuts it out again.
    let p = b
        .post_form(
            "/admin/ingest/senders/remove",
            &[("address", "you@example.com")],
        )
        .await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    let p = b
        .post_form(
            "/admin/ingest/senders/remove",
            &[("address", "you@example.com")],
        )
        .await;
    assert_eq!(p.status, StatusCode::NOT_FOUND);
    assert_eq!(
        ingest("you@example.com", None).await.0,
        StatusCode::FORBIDDEN
    );

    // A deleted Inbox is flagged; unsetting it is a change too.
    let r = env
        .command(
            TOKEN_A,
            json!({ "cmd": "delete", "id": { "kind": "list", "id": inbox } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let page = b.get("/admin/ingest").await.body;
    assert!(page.contains("The Inbox list was deleted."), "{page}");
    let p = b.post_form("/admin/ingest/inbox", &[("list", "")]).await;
    assert_eq!(p.status, StatusCode::SEE_OTHER, "{}", p.body);
    assert!(
        b.get("/admin/ingest")
            .await
            .body
            .contains("No Inbox is set.")
    );

    let audit = b.get("/admin/audit").await.body;
    for kind in [
        "ingest_sender_added",
        "ingest_sender_removed",
        "list_key_added",
        "list_key_removed",
        "ingest_inbox_set",
    ] {
        assert!(audit.contains(kind), "{kind} missing:\n{audit}");
    }
    assert_eq!(audit.matches("ingest_inbox_set").count(), 2, "{audit}");
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
    assert!(!devices.body.contains(&device));
    assert!(!devices.body.contains(&token));
    assert!(devices.body.contains("Device ID"));
    assert!(devices.body.contains(&device[..8]));
    assert!(devices.body.contains("Last active <time datetime="));
    assert!(
        devices
            .body
            .contains(r#"href="/admin/devices" aria-current="page""#)
    );
    assert!(devices.body.contains("Used to sign in here"));
    let (path, form) = devices
        .body
        .split_once(r#"action="/admin/devices/"#)
        .unwrap()
        .1
        .split_once('"')
        .unwrap();
    assert!(form.split_once("</form>").unwrap().0.contains(&format!(
        r#"name="csrf" value="{}""#,
        owner.csrf.as_deref().unwrap()
    )));
    owner.step_up().await;
    // Use the exact encoded path emitted by the HTML form.
    let p = owner
        .post_form(&format!("/admin/devices/{path}"), &[])
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
    assert!(!audit.contains(&device));
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
    // A hidden event answers exactly like a missing seq (see `api::hidden`).
    let r = env.undo(&f.token, hidden_seq).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "event_not_found");
    // So does a wrong expected ID: neither a mismatch nor the event that is there is revealed.
    let real = env.event_id(hidden_seq).await;
    let r = env
        .undo_expecting(
            &f.token,
            hidden_seq,
            &todo_core::ids::EventId::new_v7(1, [0; 10]).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "event_not_found");
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
    // On a visible event, a wrong expected ID is a mismatch, still without naming the event there.
    let own_id = env.event_id(own).await;
    let r = env
        .undo_expecting(
            &f.token,
            own,
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
    assert!(!r.json.to_string().contains(&own_id), "{}", r.json);
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
async fn imported_history_follows_the_same_rules_and_agents_cannot_import() {
    let f = fixture("").await;
    let (read_list, plan_list) = (f.lists["read"].0.clone(), f.lists["plan"].0.clone());
    let (hidden_list, _) = f.hidden();
    let hidden_list = hidden_list.to_owned();
    let import = |task: &str, list: &str, title: &str| {
        let stamp =
            json!({ "at": "2020-01-02T03:04:05Z", "by": { "type": "owner", "device": "alpha" } });
        json!({ "cmd": "import_entity", "entity": { "kind": "task", "data": {
            "id": task, "list": list, "title": title, "notes": format!("{title}-NOTES"),
            "due": null, "priority": "none", "my_day": null, "completed_at": null,
            "recurrence": null, "tags": [], "position": "a", "created": stamp, "updated": stamp,
            "deleted": null, "merged_into": null
        } } })
    };
    // Agents may not import, not even into a list they can plan in.
    let r = f
        .env
        .command(&f.token, import(&id(), &plan_list, "AGENT-IMPORT"))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.json);
    // An import into a readable list is visible while the task stays there, and hidden once it is
    // in a private list.
    let (stays, moves) = (id(), id());
    for c in [
        import(&stays, &read_list, "IMPORT-STAYS"),
        import(&moves, &read_list, "IMPORT-MOVES"),
        json!({ "cmd": "move_task", "id": moves, "list": hidden_list }),
    ] {
        let r = f.env.command(TOKEN_A, c.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{c}: {}", r.json);
    }
    let history = agent_history(&f).await;
    assert!(history.contains("IMPORT-STAYS-NOTES"));
    assert!(!history.contains("IMPORT-MOVES") && !history.contains(moves.as_str()));
    assert!(!history.contains("AGENT-IMPORT"));
}

#[tokio::test]
async fn undo_conflicts_name_only_later_events_the_agent_could_read() {
    let f = fixture("").await;
    let (plan_list, task) = f.lists["plan"].clone();
    let (hidden_list, _) = f.hidden();
    let hidden_list = hidden_list.to_owned();
    let r = f
        .env
        .command(
            &f.token,
            json!({ "cmd": "update_task", "id": task, "patch": { "title": "agent title" } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let own = r.json["events"][0]["seq"].as_u64().unwrap();
    // The owner edits the same field while the task is in a private list.
    let mut private = Vec::new();
    for c in [
        json!({ "cmd": "move_task", "id": task, "list": hidden_list }),
        json!({ "cmd": "update_task", "id": task, "patch": { "title": "private title" } }),
        json!({ "cmd": "move_task", "id": task, "list": plan_list }),
    ] {
        let r = f.env.command(TOKEN_A, c.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{c}: {}", r.json);
        private.push(r.json["events"][0].clone());
    }
    let private_edit = &private[1];
    let (private_id, private_seq) = (
        private_edit["id"].as_str().unwrap().to_owned(),
        private_edit["seq"].as_u64().unwrap(),
    );
    // The conflict is reported, without naming the private edit.
    let r = f.env.undo(&f.token, own).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.json);
    assert_eq!(r.json["error"]["code"], "undo_conflict");
    let text = r.json.to_string();
    assert!(!text.contains(&private_id), "{text}");
    assert!(
        r.json["error"]["details"].get("conflicts").is_none(),
        "{text}"
    );
    assert!(!text.contains(&format!("\"seq\":{private_seq}")), "{text}");
    // The owner sees it.
    let r = f.env.undo(TOKEN_A, own).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.json);
    assert!(r.json.to_string().contains(&private_id), "{}", r.json);

    // A later edit the agent can read is named; the private one still is not.
    let r = f
        .env
        .command(
            TOKEN_A,
            json!({ "cmd": "update_task", "id": task, "patch": { "title": "visible title" } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let visible_id = r.json["events"][0]["id"].as_str().unwrap().to_owned();
    let r = f.env.undo(&f.token, own).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.json);
    let later = &r.json["error"]["details"]["conflicts"][0]["later_events"];
    assert_eq!(later.as_array().unwrap().len(), 1, "{}", r.json);
    assert_eq!(later[0]["id"], visible_id.as_str());
    assert!(!r.json.to_string().contains(&private_id), "{}", r.json);
}

#[tokio::test]
async fn a_creation_in_a_readable_list_is_hidden_once_the_task_is_private() {
    let f = fixture("").await;
    let read_list = f.lists["read"].0.clone();
    let (hidden_list, _) = f.hidden();
    let hidden_list = hidden_list.to_owned();
    let (task, step, link) = (id(), id(), id());
    for c in [
        json!({ "cmd": "create_task", "id": task, "list": read_list, "title": "TITLE-LATER-PRIVATE",
                "notes": "NOTES-LATER-PRIVATE" }),
        json!({ "cmd": "create_step", "id": step, "task": task, "title": "STEP-LATER-PRIVATE" }),
        json!({ "cmd": "add_link", "id": link, "task": task,
                "kind": { "type": "web", "url": "https://example.com/LINK-LATER-PRIVATE", "title": null } }),
        json!({ "cmd": "move_task", "id": task, "list": hidden_list }),
    ] {
        let r = f.env.command(TOKEN_A, c.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{c}: {}", r.json);
    }
    // JSON pages, the activity feed and an SSE replay.
    let history = agent_history(&f).await;
    for leak in [
        "TITLE-LATER-PRIVATE",
        "NOTES-LATER-PRIVATE",
        "STEP-LATER-PRIVATE",
        "LINK-LATER-PRIVATE",
        task.as_str(),
        step.as_str(),
        link.as_str(),
    ] {
        assert!(
            !history.contains(leak),
            "{leak} leaked:
{history}"
        );
    }

    // Live frames: the creation is taken back (forced past the later changes) while the task is in
    // the private list. The withdrawal carries where the task was made; it was in the private list.
    let head = f.env.t.server.engine.head_seq();
    let mut live = f.env.t.sse(&f.token, Some(head)).await;
    // Owner's view of the log: the seq of the event mentioning `needle`.
    let seq_of = async |needle: &str| {
        let r = f
            .env
            .api(
                Method::GET,
                "/api/v1/events?since=0&limit=1000",
                TOKEN_A,
                None,
            )
            .await;
        r.json["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| {
                let text = e.to_string();
                text.contains(needle) && text.contains(r#""op":"created""#)
            })
            .unwrap()["seq"]
            .as_u64()
            .unwrap()
    };
    // The link and the step first: the task cannot be withdrawn while they exist.
    for needle in [
        "LINK-LATER-PRIVATE",
        "STEP-LATER-PRIVATE",
        "NOTES-LATER-PRIVATE",
    ] {
        let seq = seq_of(needle).await;
        let expected = f.env.event_id(seq).await;
        let r = f
            .env
            .api(
                Method::POST,
                &format!("/api/v1/events/{seq}/undo"),
                TOKEN_A,
                Some(json!({ "command_id": id(), "expected_event_id": expected, "force": true })),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{needle}: {}", r.json);
    }
    assert!(f.env.t.server.engine.head_seq() > head);
    // A visible change afterwards, so the stream has something to deliver.
    let r = f
        .env
        .command(
            TOKEN_A,
            json!({ "cmd": "update_task", "id": f.lists["read"].1, "patch": { "title": "LIVE-VISIBLE" } }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let mut seen = String::new();
    while let Some(frame) = live.next_event(Duration::from_millis(500)).await {
        seen.push_str(&frame.data);
    }
    assert!(seen.contains("LIVE-VISIBLE"), "{seen}");
    for leak in ["TITLE-LATER-PRIVATE", "NOTES-LATER-PRIVATE", task.as_str()] {
        assert!(
            !seen.contains(leak),
            "{leak} leaked live:
{seen}"
        );
    }
    // The task, step and link are gone, and their last known place was the private list: still
    // hidden, also after a restart (which rebuilds last known places from the log).
    let gone = [task.as_str(), step.as_str(), link.as_str(), "LATER-PRIVATE"];
    let history = agent_history(&f).await;
    assert!(gone.iter().all(|g| !history.contains(g)), "{history}");
    let f = f.restart_with(|_| {}).await;
    let history = agent_history(&f).await;
    assert!(gone.iter().all(|g| !history.contains(g)), "{history}");
    assert!(history.contains("LIVE-VISIBLE"));
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
    assert_eq!(page.header(CACHE_CONTROL), "no-store, no-transform");
    assert_eq!(page.header(REFERRER_POLICY), "same-origin");
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
