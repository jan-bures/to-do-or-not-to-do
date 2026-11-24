use super::*;
use crate::app::{Options, Server};
use crate::auth::store::{SessionRow, web_push::Registered};
use crate::config::Config;
use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
use p256::ecdsa::signature::Verifier;
use serde_json::json;
use todo_core::ids::ReminderId;

const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
const RECEIVER: &str = concat!(
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcx",
    "aOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
);
const EXPECTED: &str = concat!(
    "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml",
    "mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT",
    "pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
);

#[test]
fn rfc8291_example() {
    let sender = SecretKey::from_slice(
        &B64.decode("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")
            .unwrap(),
    )
    .unwrap();
    let receiver = PublicKey::from_sec1_bytes(&B64.decode(RECEIVER).unwrap()).unwrap();
    let salt = B64
        .decode("DGv6ra1nlYgDCS1FRnbzlw")
        .unwrap()
        .try_into()
        .unwrap();
    let body = b"When I grow up, I want to be a watermelon";
    let encrypted = encrypt(body, &sender, &receiver, &B64.decode(AUTH).unwrap(), &salt).unwrap();
    assert_eq!(B64.encode(encrypted), EXPECTED);
}

#[test]
fn key_generation_is_private_and_never_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vapid.secret");
    write_vapid_key(&path).unwrap();
    let first = Sender::load(&path, None).unwrap();
    assert!(write_vapid_key(&path).is_err());
    assert_eq!(
        first.public_key(),
        Sender::load(&path, None).unwrap().public_key()
    );
}

#[test]
fn vapid_claims_and_signature_are_bound_to_the_push_service_origin() {
    let sender = Sender::new(
        secret_key().unwrap(),
        Some("https://todo.example.com".into()),
    )
    .unwrap();
    let endpoint = Url::parse("https://push.example.net/messages/opaque?token=private").unwrap();
    let now = 1_800_000_000;
    let header = sender.authorization(&endpoint, now);
    let (token, key) = header
        .strip_prefix("vapid t=")
        .unwrap()
        .split_once(", k=")
        .unwrap();
    let (input, signature) = token.rsplit_once('.').unwrap();
    let signature = Signature::from_slice(&B64.decode(signature).unwrap()).unwrap();
    let verifying_key =
        p256::ecdsa::VerifyingKey::from_sec1_bytes(&B64.decode(key).unwrap()).unwrap();
    verifying_key.verify(input.as_bytes(), &signature).unwrap();
    let (alg, claims) = input.split_once('.').unwrap();
    let decode =
        |part| serde_json::from_slice::<serde_json::Value>(&B64.decode(part).unwrap()).unwrap();
    assert_eq!(decode(alg), json!({"alg":"ES256", "typ":"JWT"}));
    assert_eq!(
        decode(claims),
        json!({"aud":"https://push.example.net", "sub":"https://todo.example.com", "exp": now + 43_200})
    );
}

#[test]
fn only_supported_https_push_hosts_with_default_ports() {
    for raw in [
        "https://fcm.googleapis.com/fcm/send/opaque",
        "https://updates.push.services.mozilla.com/wpush/v2/opaque",
        "https://web.push.apple.com/QOpaque",
        "https://wns2-par02p.notify.windows.com/w/?token=opaque",
        "https://fcm.googleapis.com:443/fcm/send/opaque",
    ] {
        assert!(endpoint(raw).is_some(), "{raw}");
    }
    for raw in [
        "http://fcm.googleapis.com/path",
        "https://localhost/path",
        "https://[::1]/x",
        "https://169.254.169.254/x",
        "https://2130706433/x",
        "https://fcm.googleapis.com:444/path",
        "https://fcm.googleapis.com.evil.example.com/path",
        "https://evilpush.apple.com/path",
        "https://fcm.googleapis.com@127.0.0.1/path",
        "https://someone@fcm.googleapis.com/path",
        "https://@fcm.googleapis.com/path",
        "https://fcm.googleapis.com\\@127.0.0.1/path",
        "https://updates.push.services.mozilla.com./path",
        "https://fcm.googleapis.com/path#fragment",
        "https://%66cm.googleapis.com/path",
        "https://push.apple.com/path",
        "https://fcm.googleapis.com/",
    ] {
        assert!(endpoint(raw).is_none(), "{raw}");
    }
    assert!(endpoint(&format!("https://fcm.googleapis.com/{}", "a".repeat(4096))).is_none());
}

#[test]
fn payload_contains_only_task_and_reminder_time() {
    let task = TaskId::from_uuid(uuid::Uuid::from_u128(1));
    let alarm = Alarm {
        task,
        reminder: ReminderId::from_uuid(uuid::Uuid::from_u128(2)),
        at: Timestamp::from_millisecond(8123).unwrap(),
    };
    let body: serde_json::Value = serde_json::from_slice(&reminder_body(alarm).unwrap()).unwrap();
    assert_eq!(body, json!({ "task": task, "at": 8123 }));
}

fn subscription(i: i64) -> Subscription {
    serde_json::from_value::<Registration>(json!({
        "endpoint": format!("https://fcm.googleapis.com/fcm/send/fixture-{i}"),
        "keys": { "p256dh": RECEIVER, "auth": AUTH },
    }))
    .unwrap()
    .validate()
    .unwrap()
}

async fn server() -> (tempfile::TempDir, Server) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: dir.path().to_owned(),
        bind: "127.0.0.1:0".parse().unwrap(),
        ..Config::default()
    };
    let server = Server::bind(&config, Options::default()).await.unwrap();
    // Only the database foreign key matters here; no WebAuthn ceremony or deployment credentials.
    let connection = rusqlite::Connection::open(config.database_path()).unwrap();
    connection.execute("INSERT INTO passkeys (id, cred_id, name, passkey, created_at) VALUES (1, 'fixture', 'Fixture', '{}', 0)", []).unwrap();
    server
        .auth_service()
        .store()
        .create_session(
            &SessionRow {
                id_hash: "fixture".into(),
                csrf: "fixture-csrf".into(),
                created_at: 1,
                last_seen_at: 1,
                verified_at: 1,
            },
            1,
        )
        .unwrap();
    (dir, server)
}

#[tokio::test]
async fn cap_durable_claims_and_session_revocation() {
    let (_dir, server) = server().await;
    let auth = server.auth_service();
    let store = auth.store();
    for i in 0..MAX_SUBSCRIPTIONS {
        assert!(matches!(
            store
                .register_web_push("fixture", "key", &subscription(i), 1)
                .unwrap(),
            Registered::Stored
        ));
    }
    assert!(matches!(
        store
            .register_web_push("fixture", "key", &subscription(0), 2)
            .unwrap(),
        Registered::Stored
    ));
    assert!(matches!(
        store
            .register_web_push("fixture", "key", &subscription(MAX_SUBSCRIPTIONS), 1)
            .unwrap(),
        Registered::AtCapacity
    ));
    let ids = store.unclaimed_web_push("key", "task", 100).unwrap();
    assert_eq!(ids.len() as i64, MAX_SUBSCRIPTIONS);
    assert!(
        store
            .claim_web_push(ids[0], "task", 100, 0, 0)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .claim_web_push(ids[0], "task", 100, 0, 0)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .claim_web_push(ids[1], "task", 100, 1, 0)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .unclaimed_web_push("key", "task", 0)
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .unclaimed_web_push("key", "task", 100)
            .unwrap()
            .contains(&ids[0])
    );
    let session = store.session("fixture").unwrap().unwrap();
    auth.end_session(&session, None).unwrap();
    assert!(
        store
            .unclaimed_web_push("key", "task", 100)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store
            .register_web_push("fixture", "key", &subscription(0), 1)
            .unwrap(),
        Registered::MissingSession
    ));
}

#[tokio::test]
async fn gone_response_removes_the_subscription() {
    let (_dir, server) = server().await;
    let auth = server.auth_service();
    let mut sender = Sender::new(secret_key().unwrap(), None).unwrap();
    sender.client = Client::builder().no_proxy().build().unwrap();
    let router = Router::new().fallback(post(|headers: HeaderMap, body: Bytes| async move {
        assert_eq!(headers["ttl"], "60");
        assert_eq!(headers["urgency"], "high");
        assert_eq!(headers["content-encoding"], "aes128gcm");
        assert!(
            headers["authorization"]
                .to_str()
                .unwrap()
                .starts_with("vapid t=")
        );
        assert!(body.len() > 86);
        StatusCode::GONE
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    sender.test_destination =
        Some(Url::parse(&format!("http://{}/send", listener.local_addr().unwrap())).unwrap());
    let mock = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let sub = subscription(0);
    let store = auth.store();
    let key = sender.public_key();
    assert!(matches!(
        store.register_web_push("fixture", key, &sub, 1).unwrap(),
        Registered::Stored
    ));
    let target = store
        .claim_web_push_test("fixture", sub.endpoint.as_str(), key, 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        sender.deliver(&auth, target, b"null", 60).await,
        Delivery::Gone
    );
    assert!(
        !store
            .has_web_push("fixture", sub.endpoint.as_str(), key)
            .unwrap()
    );
    mock.abort();
}

#[tokio::test]
async fn committed_alarm_is_sent_when_due_and_not_resent_after_worker_restart() {
    use todo_core::{Actor, Command};
    use uuid::Uuid;

    let (_dir, server) = server().await;
    let auth = server.auth_service();
    let engine = server.engine();
    let mut sender = Sender::new(secret_key().unwrap(), None).unwrap();
    sender.client = Client::builder().no_proxy().build().unwrap();
    let (messages, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let router = Router::new().fallback(post(move |body: Bytes| async move {
        messages.send(body).unwrap();
        StatusCode::CREATED
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    sender.test_destination =
        Some(Url::parse(&format!("http://{}/send", listener.local_addr().unwrap())).unwrap());
    let service = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let sender = Arc::new(sender);
    let now = auth.now();
    let store = auth.store();
    let mut row = store.session("fixture").unwrap().unwrap();
    row.id_hash = "live".into();
    row.created_at = now;
    row.last_seen_at = now;
    store.create_session(&row, 1).unwrap();
    assert!(matches!(
        store
            .register_web_push("live", sender.public_key(), &subscription(0), now)
            .unwrap(),
        Registered::Stored
    ));

    let (list, task, reminder) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let at = Timestamp::from_millisecond(now + 250).unwrap();
    let actor = Actor::owner("test").unwrap();
    let wake = engine.enable_web_push();
    let (stop, shutdown) = watch::channel(false);
    let worker = tokio::spawn(sender.clone().run(
        engine.clone(),
        auth.clone(),
        wake.clone(),
        shutdown,
    ));
    for body in [
        json!({"cmd":"create_list", "id":list, "name":"Test list"}),
        json!({"cmd":"create_task", "id":task, "list":list, "title":"Test title"}),
        json!({"cmd":"add_reminder", "id":reminder, "task":task, "at":at}),
    ] {
        engine
            .execute(
                &actor,
                Uuid::new_v4(),
                &serde_json::from_value::<Command>(body).unwrap(),
            )
            .unwrap();
    }
    let message = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(message.len() > 86);
    assert!(engine.now() >= at);
    stop.send(true).unwrap();
    worker.await.unwrap();

    let (stop, shutdown) = watch::channel(false);
    let worker = tokio::spawn(sender.run(engine, auth, wake, shutdown));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), requests.recv())
            .await
            .is_err()
    );
    stop.send(true).unwrap();
    worker.await.unwrap();
    service.abort();
    server.spawn().unwrap().shutdown().await.unwrap();
}
