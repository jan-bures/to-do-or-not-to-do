//! Owner device invitations and approval through the public router.
#![allow(clippy::unwrap_used)]

mod common;

use common::web::{Body, Env, SITE};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;

const INVITES: &str = "/api/v1/pair/invites";
const REDEEM: &str = "/api/v1/pair/redeem";
const REVIEW: &str = "/api/v1/pair/review";

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap()
}

#[tokio::test]
async fn paired_only_invites_and_approval_with_csrf_for_sessions() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let (_, agent) = owner.create_agent("helper", "read", "").await;
    for token in [&agent, common::TOKEN_A] {
        for path in [INVITES, REVIEW] {
            let r = env
                .api(
                    Method::POST,
                    path,
                    token,
                    Some(json!({ "code": "AAAA-AAAA" })),
                )
                .await;
            assert_eq!(r.status, StatusCode::NOT_FOUND);
        }
    }
    let r = owner
        .send(Method::POST, INVITES, Some(SITE), Body::None)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = owner
        .send(
            Method::POST,
            INVITES,
            Some("https://other.example.com"),
            Body::Json(json!({})),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let created = owner.post_json(INVITES, json!({})).await;
    assert_eq!(created.status, StatusCode::OK);
    assert_eq!(created.headers["cache-control"], "no-store, no-transform");
    let invite = created.json();
    let code = string(&invite, "code");
    let secret = string(&invite, "link").split_once('#').unwrap().1;
    assert_eq!(secret.len(), 43);
    let anon = env.browser();
    let redeemed = anon
        .post_json(
            REDEEM,
            json!({ "secret": code, "device_name": "New phone" }),
        )
        .await;
    assert_eq!(redeemed.status, StatusCode::OK, "{}", redeemed.body);
    let credential = redeemed.json();
    let token = string(&credential, "token");
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", token, None)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        anon.post_json(
            REDEEM,
            json!({ "secret": secret, "device_name": "Duplicate" })
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let path = format!("{INVITES}/{}", string(&invite, "id"));
    let used = owner
        .send(Method::GET, &path, None, Body::Json(json!({})))
        .await;
    assert_eq!(
        used.json(),
        json!({ "status": "used", "device_name": "New phone" })
    );
    assert!(owner.get("/admin/devices").await.body.contains("New phone"));
    assert!(owner.get("/admin/audit").await.body.contains("issuer"));

    let pending = anon
        .post_json("/api/v1/pair/start", json!({ "device_name": "Desktop" }))
        .await
        .json();
    let code = string(&pending, "user_code");
    let preview = env
        .api(Method::POST, REVIEW, token, Some(json!({ "code": code })))
        .await;
    assert_eq!(preview.status, StatusCode::OK);
    assert_eq!(preview.json["device_name"], "Desktop");
    let at = preview.json["created_at"].as_i64().unwrap();
    assert_eq!(
        env.api(
            Method::POST,
            REVIEW,
            token,
            Some(json!({ "code": code, "approve_at": at + 1 }))
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        env.api(
            Method::POST,
            REVIEW,
            token,
            Some(json!({ "code": code, "approve_at": at }))
        )
        .await
        .status,
        StatusCode::OK
    );
    let polled = anon
        .post_json(
            "/api/v1/pair/poll",
            json!({ "poll_token": pending["poll_token"] }),
        )
        .await;
    assert_eq!(polled.json()["status"], "approved");
}

#[tokio::test]
async fn browser_registration_is_single_use_and_cancellable_until_finish() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let mut new_browser = env.browser();
    for cancel in [true, false] {
        let invite = owner.post_json(INVITES, json!({})).await.json();
        let secret = string(&invite, "link").split_once('#').unwrap().1;
        let payload = json!({ "secret": secret, "device_name": "Another browser" });
        let wrong_origin = new_browser
            .send(
                Method::POST,
                "/auth/invite/start",
                Some("https://other.example.com"),
                Body::Json(payload.clone()),
            )
            .await;
        assert_eq!(wrong_origin.status, StatusCode::FORBIDDEN);
        let start = new_browser
            .post_json("/auth/invite/start", payload.clone())
            .await;
        assert_eq!(start.status, StatusCode::OK, "{}", start.body);
        let v = start.json();
        let credential = new_browser.register(&v["options"]);
        let body = json!({ "ceremony": v["ceremony"], "credential": credential });
        if cancel {
            let path = format!("{INVITES}/{}", string(&invite, "id"));
            let cancelled = owner
                .send(Method::DELETE, &path, Some(SITE), Body::Json(json!({})))
                .await;
            assert_eq!(cancelled.status, StatusCode::NO_CONTENT);
        }
        let finished = new_browser
            .post_json("/auth/invite/finish", body.clone())
            .await;
        if cancel {
            assert_eq!(finished.status, StatusCode::BAD_REQUEST);
        } else {
            assert_eq!(finished.status, StatusCode::OK, "{}", finished.body);
            new_browser.adopt(&finished).await;
            assert!(
                owner
                    .get("/admin/devices")
                    .await
                    .body
                    .contains("Another browser")
            );
            assert_eq!(new_browser.login().await.status, StatusCode::OK);
        }
        assert_eq!(
            new_browser
                .post_json("/auth/invite/finish", body)
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            new_browser
                .post_json("/auth/invite/start", payload)
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn expiring_an_invite_also_expires_its_browser_ceremony_and_requires_step_up_to_replace_it() {
    let env = Env::start().await;
    let mut issuer = env.owner().await;
    let mut browser = env.browser();
    let invite = issuer.post_json(INVITES, json!({})).await.json();
    let secret = string(&invite, "link").split_once('#').unwrap().1;
    let p = browser
        .post_json(
            "/auth/invite/start",
            json!({ "secret": secret, "device_name": "Browser" }),
        )
        .await;
    assert_eq!(p.status, StatusCode::OK);
    let start = p.json();
    let credential = browser.register(&start["options"]);
    env.advance(Duration::from_secs(300));
    let p = browser
        .post_json(
            "/auth/invite/finish",
            json!({ "ceremony": start["ceremony"], "credential": credential }),
        )
        .await;
    assert_eq!(p.status, StatusCode::BAD_REQUEST);
    assert!(p.set_cookie().is_empty());
    let p = browser
        .post_json(REDEEM, json!({ "secret": secret, "device_name": "Phone" }))
        .await;
    assert_eq!(p.status, StatusCode::NOT_FOUND);
    for path in [INVITES, REVIEW] {
        let stale = issuer.post_json(path, json!({ "code": "AAAA-AAAA" })).await;
        assert_eq!(stale.status, StatusCode::FORBIDDEN);
        assert_eq!(stale.json()["error"]["code"], "step_up_required");
    }
    issuer.step_up().await;
    let created = issuer.post_json(INVITES, json!({})).await;
    assert_eq!(created.status, StatusCode::OK);
    let new_invite = created.json();
    let path = format!("{INVITES}/{}", string(&new_invite, "id"));
    assert_eq!(
        browser
            .post_json(
                REDEEM,
                json!({ "secret": string(&new_invite, "code"), "device_name": " " })
            )
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        issuer
            .send(Method::GET, &path, None, Body::Json(json!({})))
            .await
            .json()["status"],
        "pending"
    );
    assert!(
        issuer
            .post_form("/auth/logout", &[])
            .await
            .status
            .is_redirection()
    );
    assert_eq!(
        browser
            .post_json(
                REDEEM,
                json!({ "secret": string(&new_invite, "code"), "device_name": "Phone" })
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn anonymous_redemption_is_rate_limited_per_address() {
    let env = Env::start_with(|cfg| {
        cfg.auth.auth_requests_per_minute = 1;
        cfg.auth.auth_request_burst = 2;
        cfg.trust_cloudflare_headers = true;
    })
    .await;
    let mut anon = env.browser();
    anon.extra.push(("CF-Connecting-IP", "203.0.113.7".into()));
    let body = json!({ "secret": "wrong", "device_name": "Test" });
    for _ in 0..2 {
        assert_eq!(
            anon.post_json(REDEEM, body.clone()).await.status,
            StatusCode::NOT_FOUND
        );
    }
    let limited = anon.post_json(REDEEM, body.clone()).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers.contains_key("retry-after"));
    anon.extra[0].1 = "203.0.113.8".into();
    assert_eq!(
        anon.post_json(REDEEM, body).await.status,
        StatusCode::NOT_FOUND
    );
}
