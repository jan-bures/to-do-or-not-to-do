//! Assistant endpoints use real owner authorization over the public router and a local model stub.
#![allow(clippy::unwrap_used)]

mod common;

use std::{io::Write, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body as StreamBody,
    extract::State,
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use common::web::{Body, Env, SITE};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use todo_server::assistant::PrivateConfig;
use tokio::{net::TcpListener, sync::Notify};

const STATUS: &str = "/api/v1/assistant/status";
const PRIVATE: &str = "/api/v1/assistant/private";
const KEY: &str = "model-fixture-value";

#[tokio::test]
async fn only_real_owner_sessions_and_paired_tokens_can_use_the_assistant() {
    let env = Env::start().await;
    let browser = env.owner().await;
    let (_, agent) = browser.create_agent("example-agent", "read", "").await;
    let response = env.api(Method::GET, STATUS, &agent, None).await;
    let missing = env
        .api(Method::GET, "/api/v1/no-such-route", &agent, None)
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.json, missing.json);
    let response = env.api(Method::GET, STATUS, common::TOKEN_A, None).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(response.json["error"]["code"], "assistant_owner_required");
    let response = env
        .browser()
        .send(Method::GET, STATUS, None, Body::None)
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    let response = browser.send(Method::GET, STATUS, None, Body::None).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN, "missing CSRF");
    let response = browser
        .send(Method::GET, STATUS, None, Body::Json(json!({})))
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.json()["private_models"], false);
    assert_eq!(response.headers["cache-control"], "no-store, no-transform");
    let invite = browser
        .post_json("/api/v1/pair/invites", json!({}))
        .await
        .json();
    let paired = env
        .browser()
        .post_json(
            "/api/v1/pair/redeem",
            json!({"device_name":"Test device", "secret":invite["code"]}),
        )
        .await
        .json();
    let token = paired["token"].as_str().unwrap();
    assert_eq!(
        env.api(Method::GET, STATUS, token, None).await.status,
        StatusCode::OK
    );
    assert_eq!(
        env.api(Method::GET, &format!("{PRIVATE}/models"), token, None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        env.api(Method::DELETE, "/api/v1/pair/device", token, None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        env.api(Method::GET, STATUS, token, None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn assistant_writes_require_both_csrf_and_origin() {
    let env = Env::start().await;
    let browser = env.owner().await;
    let id = uuid::Uuid::new_v4();
    for (method, path, payload) in [
        (
            Method::POST,
            "/api/v1/assistant/chatgpt/start".into(),
            json!({"flow":"device"}),
        ),
        (
            Method::PUT,
            "/api/v1/assistant/access".into(),
            json!({"access":{"lists":{}}, "access_version":id}),
        ),
        (
            Method::DELETE,
            format!("/api/v1/assistant/chatgpt/accounts/{id}"),
            Value::Null,
        ),
    ] {
        for (origin, body) in [
            (Some(SITE), Body::None),
            (None, Body::Json(payload.clone())),
            (
                Some("https://other.example.com"),
                Body::Json(payload.clone()),
            ),
        ] {
            let denied = browser.send(method.clone(), &path, origin, body).await;
            assert_eq!(denied.status, StatusCode::FORBIDDEN, "{method} {path}");
            assert_eq!(denied.json()["error"]["code"], "csrf");
        }
        let accepted = browser
            .send(method, &path, Some(SITE), Body::Json(payload))
            .await;
        assert_eq!(
            accepted.json()["error"]["code"],
            "assistant_disabled",
            "{path}: {}",
            accepted.body
        );
    }
}

#[tokio::test]
async fn bad_vault_or_model_key_disables_only_the_assistant() {
    for private in [false, true] {
        let env = Env::start_with(|cfg| {
            let key = cfg.data_dir.join("fixture-bad-secret");
            todo_fs::create_private_file(&key)
                .unwrap()
                .write_all(b"not a key")
                .unwrap();
            if private {
                cfg.assistant.private = Some(PrivateConfig {
                    base_url: "http://127.0.0.1:8030".into(),
                    key_file: key,
                });
            } else {
                cfg.assistant.chatgpt = true;
                cfg.assistant.key_file = Some(key);
            }
        })
        .await;
        let browser = env.owner().await;
        let status = browser
            .send(Method::GET, STATUS, None, Body::Json(json!({})))
            .await;
        assert_eq!(status.status, StatusCode::OK);
        let json = status.json();
        assert_eq!(json["chatgpt"], false);
        assert_eq!(json["private_models"], false);
        assert!(
            json["disabled_reason"]
                .as_str()
                .unwrap()
                .contains(if private {
                    "private model key"
                } else {
                    "vault"
                })
        );
        assert!(!status.body.contains("fixture-bad-secret"));
        assert!(!status.body.contains("not a key"));
        assert_eq!(
            browser
                .send(Method::GET, "/health", None, Body::None)
                .await
                .status,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn private_relay_checks_destination_and_streams_without_forwarding_headers() {
    let finish = Arc::new(Notify::new());
    let model =
        Router::new()
            .route(
                "/v1/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
                    Json(json!({"data": [{"id":"fixture-model"}]}))
                }),
            )
            .route(
                "/v1/chat/completions",
                post(
                    |State(finish): State<Arc<Notify>>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
                        assert!(
                            !headers.contains_key("cookie")
                                && !headers.contains_key("x-csrf-token")
                        );
                        assert_eq!(body["model"], "fixture-model");
                        assert!(body.get("lists").is_none() && body.get("destination").is_none());
                        let stream = futures_util::stream::unfold(
                            (finish, 0usize),
                            |(finish, step)| async move {
                                match step {
                                    0 => Some((
                                        Ok::<_, std::io::Error>("data: first\n\n"),
                                        (finish, 1),
                                    )),
                                    1 => {
                                        finish.notified().await;
                                        Some((Ok("data: second\n\n"), (finish, 2)))
                                    }
                                    _ => None,
                                }
                            },
                        );
                        Response::builder()
                            .header("content-type", "text/event-stream")
                            .header("connection", "fixture-hop")
                            .header("fixture-hop", "not forwarded")
                            .header("set-cookie", "not-forwarded=fixture")
                            .body(StreamBody::from_stream(stream))
                            .unwrap()
                    },
                ),
            )
            .with_state(finish.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move { axum::serve(listener, model).await.unwrap() });
    let env = Env::start_with(|cfg| {
        cfg.assistant.key_file = Some(cfg.data_dir.join("assistant.secret"));
        let path = cfg.data_dir.join("model.secret");
        todo_fs::create_private_file(&path)
            .unwrap()
            .write_all(KEY.as_bytes())
            .unwrap();
        cfg.assistant.private = Some(PrivateConfig {
            base_url: format!("http://{addr}/v1"),
            key_file: path,
        });
    })
    .await;
    let browser = env.owner().await;
    let initial = browser
        .send(Method::GET, STATUS, None, Body::Json(json!({})))
        .await
        .json();
    let private = env.t.create_list("Private list").await;
    let hidden = env.t.create_list("Hidden list").await;
    let request = |list: &str, destination: &str| json!({"destination":destination,"lists":[list],"request":{"model":"fixture-model","messages":[{"role":"user","content":"Hello"}],"stream":true}});
    let rules = json!({"access":{"lists": {private.clone(): "Private"}}, "access_version":initial["access_version"]});
    let update = browser
        .send(
            Method::PUT,
            "/api/v1/assistant/access",
            Some(SITE),
            Body::Json(rules.clone()),
        )
        .await;
    assert_eq!(update.status, StatusCode::OK, "{}", update.body);
    assert_ne!(update.json()["access_version"], initial["access_version"]);
    assert_eq!(
        browser
            .send(
                Method::PUT,
                "/api/v1/assistant/access",
                Some(SITE),
                Body::Json(rules)
            )
            .await
            .status,
        StatusCode::CONFLICT
    );
    let models = browser
        .send(
            Method::GET,
            &format!("{PRIVATE}/models"),
            None,
            Body::Json(json!({})),
        )
        .await;
    assert_eq!(
        models.json(),
        json!([{"id":"fixture-model","label":"fixture-model"}])
    );
    let chat = format!("{PRIVATE}/chat");
    let denied = browser.post_json(&chat, request(&private, "cloud")).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);
    assert_eq!(denied.json()["error"]["code"], "assistant_scope");
    let mut streamed = common::http()
        .build()
        .unwrap()
        .post(format!("{}{chat}", browser.url))
        .header(
            "cookie",
            format!("__Host-todo_session={}", browser.cookie.as_deref().unwrap()),
        )
        .header("x-csrf-token", browser.csrf.as_deref().unwrap())
        .header("origin", SITE)
        // The relay does not trust or interpret this client-supplied list hint.
        .json(&request(&hidden, "local"))
        .send()
        .await
        .unwrap();
    assert_eq!(streamed.status(), StatusCode::OK);
    assert!(!streamed.headers().contains_key("fixture-hop"));
    assert!(!streamed.headers().contains_key("set-cookie"));
    let first = tokio::time::timeout(Duration::from_secs(3), streamed.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first.as_ref(), b"data: first\n\n");
    finish.notify_one();
    assert_eq!(streamed.text().await.unwrap(), "data: second\n\n");
    model_server.abort();
}
