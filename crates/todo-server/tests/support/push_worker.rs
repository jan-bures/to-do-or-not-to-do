use super::*;
use crate::app::{Options, Server};
use crate::config::Config;
use crate::engine::Engine;
use axum::extract::State as AxumState;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::sync::atomic::{AtomicUsize, Ordering};
use todo_core::{Actor, Command};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid::Uuid;

const PROJECT: &str = "test-firebase";
const TOKEN_A: &str = "test-registration-a";
const TOKEN_B: &str = "test-registration-b";
const UNREGISTERED: &str = "test-registration-gone";
const INVALID: &str = "test-registration-invalid";
const QUOTA: &str = "test-registration-quota";
const UNAVAILABLE: &str = "test-registration-unavailable";

#[derive(Clone)]
struct Fake {
    messages: mpsc::UnboundedSender<Value>,
    oauth_calls: Arc<AtomicUsize>,
}

async fn token(AxumState(fake): AxumState<Fake>, headers: HeaderMap) -> impl IntoResponse {
    assert_eq!(headers["Metadata-Flavor"], "Google");
    fake.oauth_calls.fetch_add(1, Ordering::Relaxed);
    (
        [("Metadata-Flavor", "Google")],
        Json(json!({"access_token":"fake-oauth", "expires_in":3600})),
    )
}

async fn message(
    AxumState(fake): AxumState<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(headers["authorization"], "Bearer fake-oauth");
    let registration = body["message"]["token"].as_str().unwrap().to_owned();
    fake.messages.send(body).unwrap();
    match registration.as_str() {
        UNREGISTERED => (
            StatusCode::NOT_FOUND,
            Json(json!({"error":{"details":[{"errorCode":"UNREGISTERED"}]}})),
        )
            .into_response(),
        INVALID => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"status":"INVALID_ARGUMENT"}})),
        )
            .into_response(),
        QUOTA => (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "75")]).into_response(),
        UNAVAILABLE => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        _ => StatusCode::OK.into_response(),
    }
}

async fn fake_fcm() -> (Sender, Fake, mpsc::UnboundedReceiver<Value>, JoinHandle<()>) {
    let (messages, rx) = mpsc::unbounded_channel();
    let fake = Fake {
        messages,
        oauth_calls: Arc::default(),
    };
    let router = Router::new()
        .route("/metadata", get(token))
        .route("/send", post(message))
        .with_state(fake.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let sender = Sender {
        project: PROJECT.into(),
        endpoint: Url::parse(&format!("{base}/send")).unwrap(),
        client: client(true).unwrap(),
        oauth: OAuth {
            credentials: Credentials::Metadata {
                client: client(true).unwrap(),
                url: format!("{base}/metadata"),
            },
            cached: None,
        },
    };
    (sender, fake, rx, task)
}

async fn server() -> (tempfile::TempDir, Server) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: dir.path().to_owned(),
        bind: "127.0.0.1:0".parse().unwrap(),
        ..Config::default()
    };
    let server = Server::bind(&cfg, Options::default()).await.unwrap();
    (dir, server)
}

fn execute(engine: &Engine, actor: &Actor, body: Value) -> u64 {
    engine
        .execute(
            actor,
            Uuid::new_v4(),
            &serde_json::from_value::<Command>(body).unwrap(),
        )
        .unwrap()
        .result
        .events
        .last()
        .unwrap()
        .seq
}

#[tokio::test]
async fn committed_reminder_changes_exclude_origin_and_collapse_into_one_payload() {
    let (_dir, server) = server().await;
    let engine = server.engine();
    let auth = server.auth_service();
    let (a, _) = auth.create_device("First", None).unwrap();
    let (b, _) = auth.create_device("Second", None).unwrap();
    auth.store()
        .register_push(a.as_str(), PROJECT, TOKEN_A)
        .unwrap();
    auth.store()
        .register_push(b.as_str(), PROJECT, TOKEN_B)
        .unwrap();
    let actor = Actor::owner(a.as_str()).unwrap();
    let (sender, fake, mut messages, fake_task) = fake_fcm().await;
    let (shutdown, signal) = watch::channel(false);
    let events = engine.enable_push();
    let worker = tokio::spawn(sender.run(auth, events, signal));
    let list = Uuid::new_v4();
    let task = Uuid::new_v4();
    let reminder = Uuid::new_v4();
    execute(
        &engine,
        &actor,
        json!({"cmd":"create_list", "id":list, "name":"a list"}),
    );
    execute(
        &engine,
        &actor,
        json!({"cmd":"create_task", "id":task, "list":list, "title":"a title"}),
    );
    execute(
        &engine,
        &actor,
        json!({"cmd":"add_reminder", "id":reminder, "task":task, "at":"2090-01-01T09:00:00Z"}),
    );
    let seq = execute(
        &engine,
        &actor,
        json!({"cmd":"update_reminder", "id":reminder, "at":"2090-01-01T10:00:00Z"}),
    );
    let body = timeout(Duration::from_secs(6), messages.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        body,
        json!({"message":{
            "token":TOKEN_B, "data":{"t":"sync", "seq":seq.to_string()},
            "android":{"priority":"HIGH", "collapse_key":"reminder-sync", "ttl":"3600s"}
        }})
    );
    assert!(
        timeout(Duration::from_millis(200), messages.recv())
            .await
            .is_err()
    );
    assert_eq!(fake.oauth_calls.load(Ordering::Relaxed), 1);
    shutdown.send(true).unwrap();
    worker.await.unwrap();
    fake_task.abort();
    server.spawn().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn selection_filters_unrelated_changes_and_includes_undo_and_visibility() {
    let (_dir, server) = server().await;
    let engine = server.engine();
    let a = Actor::owner("origin").unwrap();
    let wakes = engine.enable_push();
    let (list, collection, other, task, reminder) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    for body in [
        json!({"cmd":"create_collection", "id":collection, "name":"a collection"}),
        json!({"cmd":"create_list", "id":list, "name":"a list", "collection":collection}),
        json!({"cmd":"create_list", "id":other, "name":"other list"}),
        json!({"cmd":"create_task", "id":task, "list":list, "title":"a title"}),
        json!({"cmd":"complete_task", "id":task}),
        json!({"cmd":"uncomplete_task", "id":task}),
    ] {
        execute(&engine, &a, body);
        assert!(wakes.take().best.is_none());
    }
    let run = |body| {
        let seq = execute(&engine, &a, body);
        let wake = wakes.take().best.unwrap();
        assert_eq!(wake.seq, seq);
        assert_eq!(wake.origin.unwrap().as_str(), "origin");
        seq
    };
    run(json!({"cmd":"add_reminder", "id":reminder, "task":task, "at":"2090-01-01T09:00:00Z"}));
    execute(
        &engine,
        &a,
        json!({"cmd":"update_task", "id":task, "patch":{"title":"new title"}}),
    );
    assert!(wakes.take().best.is_none());
    for body in [
        json!({"cmd":"complete_task", "id":task}),
        json!({"cmd":"uncomplete_task", "id":task}),
        json!({"cmd":"delete", "id":{"kind":"task", "id":task}}),
        json!({"cmd":"restore", "id":{"kind":"task", "id":task}}),
        json!({"cmd":"move_task", "id":task, "list":other}),
        json!({"cmd":"move_task", "id":task, "list":list}),
        json!({"cmd":"delete", "id":{"kind":"collection", "id":collection}}),
        json!({"cmd":"restore", "id":{"kind":"collection", "id":collection}}),
        json!({"cmd":"delete", "id":{"kind":"list", "id":list}}),
        json!({"cmd":"restore", "id":{"kind":"list", "id":list}}),
    ] {
        run(body);
    }
    let deleted = run(json!({"cmd":"delete", "id":{"kind":"reminder", "id":reminder}}));
    let undone = engine.undo(&a, deleted, false, None).unwrap();
    assert_eq!(
        wakes.take().best.unwrap().seq,
        undone.result.events.last().unwrap().seq
    );
    server.spawn().unwrap().shutdown().await.unwrap();
}

#[test]
fn coalesced_origins_only_exclude_devices_from_their_own_changes() {
    let mut batch = Batch::default();
    for (origin, seq) in [
        (Some("a"), 1),
        (Some("a"), 2),
        (None, 3),
        (Some("a"), 4),
        (Some("b"), 5),
        (Some("b"), 6),
    ] {
        batch.add(Wake {
            origin: origin.map(|d| DeviceId::new(d).unwrap()),
            seq,
        });
    }
    assert_eq!(batch.seq_for("a"), Some(6));
    assert_eq!(batch.seq_for("b"), Some(4));
    assert_eq!(batch.seq_for("c"), Some(6));
}

#[tokio::test]
async fn invalid_tokens_are_removed_and_transient_errors_wait_without_blocking_other_devices() {
    let (_dir, server) = server().await;
    let auth = server.auth_service();
    let (mut sender, fake, mut messages, fake_task) = fake_fcm().await;
    let mut pending = BTreeMap::new();
    let now = Instant::now();
    let mut ids = Vec::new();
    for (i, token) in [INVALID, UNREGISTERED, QUOTA, UNAVAILABLE, TOKEN_B]
        .into_iter()
        .enumerate()
    {
        let (id, _) = auth.create_device(&format!("Device {i}"), None).unwrap();
        assert!(
            auth.store()
                .register_push(id.as_str(), PROJECT, token)
                .unwrap()
        );
        ids.push(id.to_string());
        pending.insert(
            id.to_string(),
            Pending {
                seq: Some(1),
                at: now,
                attempts: 0,
            },
        );
    }
    let targets = || {
        auth.store()
            .push_targets(PROJECT)
            .unwrap()
            .into_iter()
            .collect::<BTreeMap<_, _>>()
    };
    sender.deliver(&auth, targets(), &mut pending, now).await;
    assert!(!targets().contains_key(&ids[0]));
    assert!(!targets().contains_key(&ids[1]));
    assert_eq!(pending.len(), 2);
    assert!(pending[&ids[2]].at >= now + Duration::from_secs(75));
    assert!(pending[&ids[3]].at >= now + Duration::from_secs(10));
    for _ in 0..5 {
        messages.try_recv().unwrap();
    }
    sender.deliver(&auth, targets(), &mut pending, now).await;
    assert!(messages.try_recv().is_err());
    for p in pending.values_mut() {
        p.at = now;
    }
    sender.deliver(&auth, targets(), &mut pending, now).await;
    assert_eq!(
        fake.oauth_calls.load(Ordering::Relaxed),
        1,
        "retries reuse live credentials"
    );
    assert_eq!(pending[&ids[3]].attempts, 2);
    assert!(pending[&ids[3]].at >= now + Duration::from_secs(20));
    auth.store()
        .register_push(&ids[2], PROJECT, TOKEN_A)
        .unwrap();
    auth.store()
        .remove_invalid_push(&ids[2], PROJECT, QUOTA)
        .unwrap();
    assert_eq!(
        targets()[&ids[2]],
        TOKEN_A,
        "a stale response cannot remove the replacement"
    );
    auth.revoke_device(&ids[2], None).unwrap();
    assert!(!targets().contains_key(&ids[2]));
    assert!(
        !auth
            .store()
            .register_push(&ids[2], PROJECT, TOKEN_A)
            .unwrap()
    );
    fake_task.abort();
    server.spawn().unwrap().shutdown().await.unwrap();
}
