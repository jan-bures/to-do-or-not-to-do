//! Owner device invitations and approval through the public router.
#![allow(clippy::unwrap_used)]

mod common;

use common::web::{Body, Browser, Env, SITE, form_encode};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;

const INVITES: &str = "/api/v1/pair/invites";
const REDEEM: &str = "/api/v1/pair/redeem";
const REVIEW: &str = "/api/v1/pair/review";

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap()
}

async fn join_browser(env: &Env, invite: &Value, name: &str) -> Browser {
    let secret = string(invite, "link").split_once('#').unwrap().1;
    let mut browser = env.browser();
    let start = browser
        .post_json(
            "/auth/invite/start",
            json!({ "secret": secret, "device_name": name }),
        )
        .await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.body);
    let start = start.json();
    let credential = browser.register(&start["options"]);
    let finish = browser
        .post_json(
            "/auth/invite/finish",
            json!({
                "ceremony": start["ceremony"], "credential": credential
            }),
        )
        .await;
    assert_eq!(finish.status, StatusCode::OK, "{}", finish.body);
    browser.adopt(&finish).await;
    browser
}

fn audit_details(env: &Env, kind: &str) -> Vec<Value> {
    todo_server::auth::store::AuthStore::open(&env.t.db_path())
        .unwrap()
        .audit_page(200)
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == kind)
        .map(|row| serde_json::from_str(&row.detail).unwrap())
        .collect()
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
    assert_eq!(preview.json["source"], "127.0.0.1");
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
    let devices = owner.get("/admin/devices").await.body;
    assert!(devices.contains("Issued by"));
    assert!(devices.contains("Browser session "));
    assert!(devices.contains(&format!("Device {}", string(&credential, "device_id"))));
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
    let audit = owner.get("/admin/audit").await.body;
    assert!(audit.contains("invite_created"));
    assert!(audit.contains("invite_cancelled"));
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
async fn passkey_revocation_ends_both_kinds_of_sessions_and_their_live_invites() {
    let env = Env::start().await;
    let mut owner = env.owner().await;
    let refused = owner.post_form("/auth/passkeys/1/revoke", &[]).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(refused.body.contains("This is your first passkey"));
    assert!(refused.body.contains("owner recovery"));
    assert!(!refused.body.contains("Verify with an older passkey"));
    let invite = owner.post_json(INVITES, json!({})).await.json();
    let mut invited = join_browser(&env, &invite, "Invited browser").await;
    let revoke = "/auth/passkeys/2/revoke";
    let list = owner.get("/admin/devices").await.body;
    assert!(list.contains(revoke) && list.contains("Browser session "));
    assert_eq!(
        invited.post_json(INVITES, json!({})).await.json()["error"]["code"],
        "step_up_required"
    );
    assert_eq!(
        invited.post_form(revoke, &[]).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        owner
            .send(
                Method::POST,
                revoke,
                Some(SITE),
                Body::Form("csrf=wrong".into())
            )
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        owner
            .send(
                Method::POST,
                revoke,
                Some("https://other.example.com"),
                Body::Form(form_encode(&[("csrf", owner.csrf.as_deref().unwrap())]))
            )
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    invited.step_up().await;
    let refused = invited.post_form("/auth/passkeys/1/revoke", &[]).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(refused.body.contains("This is your first passkey"));
    let newer_view = invited.get("/admin/devices").await.body;
    assert!(!newer_view.contains("/auth/passkeys/1/revoke"));
    assert!(
        newer_view
            .contains("A newer passkey cannot remove an older one, even by revoking a device.")
    );
    let joined = invited.post_json(INVITES, json!({})).await.json();
    let redeem_body =
        |invite: &Value| json!({ "secret": string(invite, "code"), "device_name": "Phone" });
    let device = env
        .browser()
        .post_json(REDEEM, redeem_body(&joined))
        .await
        .json();
    let native_invite = env
        .api(Method::POST, INVITES, string(&device, "token"), None)
        .await;
    assert_eq!(native_invite.status, StatusCode::OK);
    let grandchild = join_browser(&env, &native_invite.json, "Derived browser").await;
    let browser_invite = invited.post_json(INVITES, json!({})).await.json();
    let child = join_browser(&env, &browser_invite, "Second browser").await;

    let anon = env.browser();
    let pending = anon
        .post_json(
            "/api/v1/pair/start",
            json!({ "device_name": "Approved phone" }),
        )
        .await
        .json();
    let approval = invited
        .post_form("/pair/approve", &[("code", string(&pending, "user_code"))])
        .await;
    assert_eq!(approval.status, StatusCode::OK, "{}", approval.body);
    let approved = anon
        .post_json(
            "/api/v1/pair/poll",
            json!({"poll_token": pending["poll_token"]}),
        )
        .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.body);
    let approved = approved.json();

    let live = invited.post_json(INVITES, json!({})).await.json();
    let mut original_session = env.browser();
    original_session.cookie.clone_from(&invited.cookie);
    assert_eq!(invited.login().await.status, StatusCode::OK);
    let login_session = invited.post_json(INVITES, json!({})).await.json();
    assert_eq!(owner.login().await.status, StatusCode::OK);
    let body = owner.get("/admin/devices").await.body;
    let (_, form) = body.split_once(&format!("action=\"{revoke}\"")).unwrap();
    assert!(
        form.split_once("</form>")
            .unwrap()
            .0
            .contains("Also removes: 2 devices, 2 passkeys, 4 browser sessions.")
    );
    // A fresh sign-in with the oldest key can revoke immediately, without a second step-up.
    assert!(owner.post_form(revoke, &[]).await.status.is_redirection());
    for browser in [&invited, &original_session, &child, &grandchild] {
        assert!(browser.get("/admin/devices").await.status.is_redirection());
    }
    for invite in [&live, &login_session] {
        assert_eq!(
            env.browser()
                .post_json(REDEEM, redeem_body(invite))
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    for device in [&device, &approved] {
        assert_eq!(
            env.api(Method::GET, "/api/v1/lists", string(device, "token"), None)
                .await
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
    let mut descendants = [string(&device, "device_id"), string(&approved, "device_id")];
    descendants.sort_unstable();
    assert_eq!(
        audit_details(&env, "passkey_revoked"),
        vec![json!({
            "passkey_id": 2, "descendant_devices": descendants, "passkeys_removed": 3, "sessions_removed": 4,
        })]
    );
    let devices = owner.get("/admin/devices").await.body;
    assert!(!devices.contains("Invited browser"));
    assert!(!devices.contains("Derived browser"));
    assert_eq!(
        owner.post_form("/auth/passkeys/1/revoke", &[]).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn device_revocation_protects_older_passkeys_but_the_admin_token_is_exempt() {
    let env = Env::start_with(common::admin_on).await;
    let owner = env.owner().await;
    let invite = owner.post_json(INVITES, json!({})).await.json();
    let device = env
        .browser()
        .post_json(
            REDEEM,
            json!({"secret": invite["code"], "device_name": "Issuer"}),
        )
        .await
        .json();
    let invite = env
        .api(Method::POST, INVITES, string(&device, "token"), None)
        .await
        .json;
    let older = join_browser(&env, &invite, "Older browser").await;
    let invite = owner.post_json(INVITES, json!({})).await.json();
    let mut newer = join_browser(&env, &invite, "Newer browser").await;
    newer.step_up().await;
    let id = string(&device, "device_id");
    let refused = newer
        .post_form(&format!("/admin/devices/{id}/revoke"), &[])
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(refused.body.contains("Verify with an older passkey"));
    let verify = newer.get("/auth/verify?next=/admin/devices").await;
    assert_eq!(verify.status, StatusCode::OK);
    assert!(verify.body.contains("Verify to revoke"));
    assert_eq!(older.get("/admin/devices").await.status, StatusCode::OK);
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", string(&device, "token"), None)
            .await
            .status,
        StatusCode::OK
    );
    assert!(audit_details(&env, "device_revoked").is_empty());

    let admin = env.t.server.admin_url().unwrap();
    let result = env
        .t
        .client
        .post(format!("{admin}/api/v1/admin/devices/{id}/revoke"))
        .bearer_auth(common::ADMIN_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), StatusCode::NO_CONTENT);
    assert!(older.get("/admin/devices").await.status.is_redirection());
    assert_eq!(newer.get("/admin/devices").await.status, StatusCode::OK);
    assert_eq!(
        audit_details(&env, "device_revoked"),
        vec![json!({
            "device": id, "actor": "admin API", "descendant_devices": [], "passkeys_removed": 1, "sessions_removed": 1,
        })]
    );
}

#[tokio::test]
async fn only_a_paired_device_can_discard_its_own_leaf_credential() {
    const DISCARD: &str = "/api/v1/pair/device";
    let env = Env::start().await;
    let owner = env.owner().await;
    let invite = owner.post_json(INVITES, json!({})).await.json();
    let issuer = env
        .browser()
        .post_json(
            REDEEM,
            json!({"secret": invite["code"], "device_name": "Issuer"}),
        )
        .await
        .json();
    let issuer_token = string(&issuer, "token");
    let invite = env
        .api(Method::POST, INVITES, issuer_token, None)
        .await
        .json;
    let leaf = env
        .browser()
        .post_json(
            REDEEM,
            json!({"secret": invite["code"], "device_name": "Leaf"}),
        )
        .await
        .json();
    let leaf_token = string(&leaf, "token");
    let pending_invite = env.api(Method::POST, INVITES, leaf_token, None).await.json;

    let (status, _, _) = env.t.request(Method::DELETE, DISCARD, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        env.api(Method::DELETE, DISCARD, common::TOKEN_A, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        owner
            .send(Method::DELETE, DISCARD, Some(SITE), Body::Json(json!({})))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let refused = env.api(Method::DELETE, DISCARD, issuer_token, None).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.json["error"]["code"], "device_has_descendants");
    assert!(audit_details(&env, "device_revoked").is_empty());

    // No caller-provided ID can choose another token as the target.
    assert_eq!(
        env.api(
            Method::DELETE,
            DISCARD,
            leaf_token,
            Some(json!({"device_id": issuer["device_id"]}))
        )
        .await
        .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", leaf_token, None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", issuer_token, None)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        env.browser()
            .post_json(
                REDEEM,
                json!({"secret": pending_invite["code"], "device_name": "Other"})
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        audit_details(&env, "device_revoked"),
        vec![json!({
            "device": leaf["device_id"], "actor": "device self-revoke", "descendant_devices": [], "passkeys_removed": 0, "sessions_removed": 0,
        })]
    );
}

#[tokio::test]
async fn revoked_device_cannot_authorize_redemption() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let issuer_invite = owner.post_json(INVITES, json!({})).await.json();
    let device = env
        .browser()
        .post_json(
            REDEEM,
            json!({ "secret": string(&issuer_invite, "code"), "device_name": "Issuer" }),
        )
        .await
        .json();
    let invite = env
        .api(Method::POST, INVITES, string(&device, "token"), None)
        .await;
    assert_eq!(invite.status, StatusCode::OK);
    assert!(
        owner
            .post_form(
                &format!("/admin/devices/{}/revoke", string(&device, "device_id")),
                &[]
            )
            .await
            .status
            .is_redirection()
    );
    assert_eq!(
        env.browser()
            .post_json(
                REDEEM,
                json!({ "secret": string(&invite.json, "code"), "device_name": "New device" })
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn revocation_covers_descendants_and_prevents_finishing_approved_pairings() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let installer = env.browser();
    let mut invite = owner.post_json(INVITES, json!({})).await.json();
    let mut devices = Vec::new();
    for name in ["Root device", "Child device", "Grandchild device"] {
        let paired = installer
            .post_json(
                REDEEM,
                json!({"secret": invite["code"], "device_name": name}),
            )
            .await;
        assert_eq!(paired.status, StatusCode::OK);
        let device = paired.json();
        let created = env
            .api(Method::POST, INVITES, string(&device, "token"), None)
            .await;
        assert_eq!(created.status, StatusCode::OK);
        invite = created.json;
        devices.push(device);
    }
    let mut browser = env.browser();
    let started = browser.post_json("/auth/invite/start", json!({
        "secret": string(&invite, "link").split_once('#').unwrap().1, "device_name": "Derived browser"
    })).await.json();
    let attestation = browser.register(&started["options"]);
    let registered = browser
        .post_json(
            "/auth/invite/finish",
            json!({"ceremony": started["ceremony"], "credential": attestation}),
        )
        .await;
    assert_eq!(registered.status, StatusCode::OK);
    browser.adopt(&registered).await;
    browser.step_up().await;
    let browser_invite = browser.post_json(INVITES, json!({})).await.json();
    let from_browser = installer
        .post_json(
            REDEEM,
            json!({"secret": browser_invite["code"], "device_name": "Browser descendant"}),
        )
        .await;
    assert_eq!(from_browser.status, StatusCode::OK);
    devices.push(from_browser.json());
    assert!(
        browser
            .post_form("/auth/logout", &[])
            .await
            .status
            .is_redirection()
    );
    assert_eq!(browser.login().await.status, StatusCode::OK);

    let pending = installer
        .post_json(
            "/api/v1/pair/start",
            json!({"device_name": "Waiting device"}),
        )
        .await
        .json();
    let grandchild = string(&devices[2], "token");
    let seen = env
        .api(
            Method::POST,
            REVIEW,
            grandchild,
            Some(json!({"code": pending["user_code"]})),
        )
        .await;
    assert_eq!(seen.status, StatusCode::OK);
    let approved = env
        .api(
            Method::POST,
            REVIEW,
            grandchild,
            Some(json!({"code": pending["user_code"], "approve_at": seen.json["created_at"]})),
        )
        .await;
    assert_eq!(approved.status, StatusCode::OK);
    assert!(
        owner
            .get("/admin/devices")
            .await
            .body
            .contains("Also removes: 3 devices, 1 passkey, 1 browser session.")
    );
    let root = string(&devices[0], "device_id");
    assert!(
        owner
            .post_form(&format!("/admin/devices/{root}/revoke"), &[])
            .await
            .status
            .is_redirection()
    );
    let mut descendants = devices[1..]
        .iter()
        .map(|d| string(d, "device_id"))
        .collect::<Vec<_>>();
    descendants.sort_unstable();
    assert_eq!(
        audit_details(&env, "device_revoked"),
        vec![json!({
            "device": root, "descendant_devices": descendants, "passkeys_removed": 1, "sessions_removed": 1,
        })]
    );
    for device in devices {
        assert_eq!(
            env.api(Method::GET, "/api/v1/lists", string(&device, "token"), None)
                .await
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(browser.get("/admin/devices").await.status.is_redirection());
    let listing = owner.get("/admin/devices").await.body;
    assert!(!listing.contains("Derived browser"));
    assert!(listing.contains("First passkey"));
    let poll = installer
        .post_json(
            "/api/v1/pair/poll",
            json!({"poll_token": pending["poll_token"]}),
        )
        .await;
    assert_eq!(poll.status, StatusCode::NOT_FOUND);
    assert_eq!(poll.json()["error"]["code"], "pairing_not_found");
}

#[tokio::test]
async fn exactly_one_concurrent_redemption_wins_in_the_last_second_and_status_outlives_expiry() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let invite = owner.post_json(INVITES, json!({})).await.json();
    let path = format!("{INVITES}/{}", string(&invite, "id"));
    env.advance(Duration::from_millis(299_001));
    let browser = env.browser();
    let a = browser.post_json(
        REDEEM,
        json!({ "secret": string(&invite, "code"), "device_name": "Phone" }),
    );
    let b = browser.post_json(REDEEM, json!({ "secret": string(&invite, "link").split_once('#').unwrap().1, "device_name": "Laptop" }));
    let (a, b) = tokio::join!(a, b);
    let expected = match (a.status, b.status) {
        (StatusCode::OK, StatusCode::NOT_FOUND) => "Phone",
        (StatusCode::NOT_FOUND, StatusCode::OK) => "Laptop",
        _ => panic!("expected one winner: {:?}, {:?}", a.status, b.status),
    };
    env.advance(Duration::from_millis(999));
    assert_eq!(
        owner
            .send(Method::GET, &path, None, Body::Json(json!({})))
            .await
            .json(),
        json!({ "status": "used", "device_name": expected })
    );
    env.advance(Duration::from_secs(60));
    assert_eq!(
        owner
            .send(Method::GET, &path, None, Body::Json(json!({})))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn review_and_pair_page_share_the_same_wrong_code_budget() {
    let env = Env::start().await;
    let owner = env.owner().await;
    // Use a malformed code that cannot coincidentally match a real pairing.
    for _ in 0..5 {
        assert_eq!(
            owner.get("/pair?code=0000-0000").await.status,
            StatusCode::OK
        );
        assert_eq!(
            owner
                .post_json(REVIEW, json!({ "code": "0000-0000" }))
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    let pending = env
        .browser()
        .post_json("/api/v1/pair/start", json!({ "device_name": "Phone" }))
        .await
        .json();
    let code = string(&pending, "user_code");
    let locked = owner.post_json(REVIEW, json!({ "code": code })).await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(locked.json()["error"]["code"], "too_many_codes");
    assert_eq!(
        owner.get(&format!("/pair?code={code}")).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn native_review_device_code_budget_resets_after_cooldown() {
    let env = Env::start().await;
    let owner = env.owner().await;
    let browser = env.browser();
    let invite = owner.post_json(INVITES, json!({})).await.json();
    let device = browser
        .post_json(
            REDEEM,
            json!({"secret": string(&invite, "code"), "device_name": "Issuer"}),
        )
        .await
        .json();
    let token = string(&device, "token");
    for _ in 0..10 {
        assert_eq!(
            env.api(
                Method::POST,
                REVIEW,
                token,
                Some(json!({"code": "0000-0000"}))
            )
            .await
            .status,
            StatusCode::NOT_FOUND
        );
    }
    let submit = || {
        env.api(
            Method::POST,
            REVIEW,
            token,
            Some(json!({"code": "0000-0000"})),
        )
    };
    assert_eq!(submit().await.json["error"]["code"], "too_many_codes");
    env.advance(Duration::from_secs(300));
    assert_eq!(submit().await.status, StatusCode::NOT_FOUND);
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
