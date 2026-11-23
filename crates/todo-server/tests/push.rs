//! Paired bearer registration and configuration. The server needs no live Firebase project.
#![allow(clippy::unwrap_used)]

mod common;

use common::{TOKEN_A, TestServer};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use todo_server::config::PushConfig;

const PATH: &str = "/api/v1/pair/push";

#[tokio::test]
async fn push_off_by_default_and_exposed_only_to_paired_bearers() {
    let t = TestServer::start().await;
    let url = format!("{}{PATH}", t.url);
    let (_, token) = t.server.authsvc.create_device("Phone", None).unwrap();
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        for (bearer, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(TOKEN_A), StatusCode::NOT_FOUND),
        ] {
            let mut r = t
                .client
                .request(method.clone(), &url)
                .json(&json!({ "token": "invented-registration" }));
            if let Some(key) = bearer {
                r = r.bearer_auth(key);
            }
            assert_eq!(r.send().await.unwrap().status(), expected);
        }
    }
    assert_eq!(
        t.client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        t.client
            .put(&url)
            .bearer_auth(&token)
            .json(&json!({"token":"invented-registration"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        t.client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    t.stop().await;
}

async fn configured() -> TestServer {
    TestServer::start_with(|cfg| {
        cfg.push = Some(PushConfig {
            project_id: "test-firebase".into(),
            application_id: "1:123:android:abcdef".into(),
            api_key: "not-a-real-api-key".into(),
            gcm_sender_id: "123".into(),
            gce_metadata: true,
            service_account_file: None,
        });
    })
    .await
}

#[tokio::test]
async fn a_second_active_device_cannot_take_another_devices_push_token() {
    let t = configured().await;
    let url = format!("{}{PATH}", t.url);
    let (first_id, first) = t.server.authsvc.create_device("First", None).unwrap();
    let (second_id, second) = t.server.authsvc.create_device("Second", None).unwrap();
    let first_token = "first-registration";
    let second_token = "second-registration";
    let put = |bearer: &str, token: &str| {
        t.client
            .put(&url)
            .bearer_auth(bearer)
            .json(&json!({"token":token}))
            .send()
    };
    assert_eq!(
        put(&first, first_token).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put(&second, second_token).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put(&second, first_token).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let db = rusqlite::Connection::open(t.config.database_path()).unwrap();
    for (id, expected) in [(first_id, first_token), (second_id, second_token)] {
        let token: String = db
            .query_row(
                "SELECT token FROM device_push WHERE device = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(token, expected);
    }
    drop(db);
    t.stop().await;
}

#[tokio::test]
async fn registration_is_bounded_replaceable_and_removed_with_the_pairing() {
    let t = configured().await;
    let url = format!("{}{PATH}", t.url);
    let (id, bearer) = t.server.authsvc.create_device("Phone", None).unwrap();
    let response = t
        .client
        .get(&url)
        .bearer_auth(&bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({
            "projectId":"test-firebase", "applicationId":"1:123:android:abcdef", "apiKey":"not-a-real-api-key", "gcmSenderId":"123"
        })
    );
    let register = |token: &str| {
        t.client
            .put(&url)
            .bearer_auth(&bearer)
            .json(&json!({"token":token}))
            .send()
    };
    let registrations = || {
        rusqlite::Connection::open(t.config.database_path())
            .unwrap()
            .query_row(
                "SELECT token FROM device_push WHERE device = ?1",
                [id.as_str()],
                |r| r.get::<_, String>(0),
            )
            .ok()
    };
    assert_eq!(
        register("invented registration").await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        register(&"x".repeat(4097)).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        register(&"x".repeat(10_000)).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        register("invented-registration").await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        register("new-registration:with_chars-123")
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        registrations().as_deref(),
        Some("new-registration:with_chars-123")
    );
    assert_eq!(
        t.client
            .delete(&url)
            .bearer_auth(&bearer)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(registrations().is_none());
    assert_eq!(
        register("invented-registration").await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        t.client
            .delete(format!("{}/api/v1/pair/device", t.url))
            .bearer_auth(&bearer)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(registrations().is_none());
    assert_eq!(
        register("invented-registration").await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    t.stop().await;
}
