use super::*;
use crate::app::{Options, Server};
use crate::auth::store::{SessionRow, web_push::Registered};
use crate::config::Config;
use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
use p256::ecdsa::signature::Verifier;
use serde_json::json;
use std::sync::atomic::{AtomicI64, Ordering};
use todo_core::{Actor, Command};
use uuid::Uuid;

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

// Decrypt as a browser would: ECDH with the receiver's private key, with the sender key and
// salt taken from the RFC 8188 header. Do not reuse the sender's encryption implementation.
fn decrypt(body: &[u8], receiver: &SecretKey, auth: &[u8]) -> serde_json::Value {
    let salt = &body[..16];
    assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 4096);
    assert_eq!(body[20], 65);
    let sender = PublicKey::from_sec1_bytes(&body[21..86]).unwrap();
    let shared = diffie_hellman(receiver.to_nonzero_scalar(), sender.as_affine());
    let info = [
        b"WebPush: info\0".as_slice(),
        receiver.public_key().to_encoded_point(false).as_bytes(),
        &body[21..86],
    ]
    .concat();
    let mut ikm = [0; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&info, &mut ikm)
        .unwrap();
    let hkdf = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut key = [0; 16];
    let mut nonce = [0; 12];
    hkdf.expand(b"Content-Encoding: aes128gcm\0", &mut key)
        .unwrap();
    hkdf.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .unwrap();
    let mut plaintext = Aes128Gcm::new(&key.into())
        .decrypt(&Nonce::from(nonce), &body[86..])
        .unwrap();
    assert_eq!(plaintext.pop(), Some(2));
    serde_json::from_slice(&plaintext).unwrap()
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
    server_with(Options::default()).await
}

async fn server_with(options: Options) -> (tempfile::TempDir, Server) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: dir.path().to_owned(),
        bind: "127.0.0.1:0".parse().unwrap(),
        ..Config::default()
    };
    let server = Server::bind(&config, options).await.unwrap();
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
    let idle = 2 * 60 * 60 * 1000;
    let session = store.session("fixture").unwrap().unwrap();
    store.purge_sessions(idle, 0).unwrap();
    assert!(store.session("fixture").unwrap().is_none());
    let ids = store.unclaimed_web_push("key", "task", idle, idle).unwrap();
    assert_eq!(ids.len() as i64, MAX_SUBSCRIPTIONS);
    assert!(
        store
            .claim_web_push(ids[0].0, "task", idle, idle)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .claim_web_push(ids[0].0, "task", idle, idle)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .unclaimed_web_push("key", "task", 0, idle)
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .unclaimed_web_push("key", "task", idle, idle)
            .unwrap()
            .contains(&ids[0])
    );
    auth.end_session(&session, None).unwrap();
    assert!(
        store
            .unclaimed_web_push("key", "task", idle, idle)
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
            .has_web_push("fixture", sub.endpoint.as_str(), key, 1)
            .unwrap()
    );
    mock.abort();
}

#[tokio::test]
async fn committed_alarm_is_sent_when_due_and_not_resent_after_worker_restart() {
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
    let receiver = secret_key().unwrap();
    let mut sub = subscription(0);
    sub.p256dh
        .copy_from_slice(receiver.public_key().to_encoded_point(false).as_bytes());
    assert!(matches!(
        store
            .register_web_push("live", sender.public_key(), &sub, now)
            .unwrap(),
        Registered::Stored
    ));
    store.expire_session("live").unwrap();

    let at = Timestamp::from_millisecond(now + 250).unwrap();
    let wake = engine.enable_web_push();
    let (stop, shutdown) = watch::channel(false);
    let worker = tokio::spawn(sender.clone().run(
        engine.clone(),
        auth.clone(),
        wake.clone(),
        shutdown,
    ));
    let expected_title = "é".repeat(200);
    let task = commit_alarm(&engine, at, &format!("{expected_title}x"));
    let message = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        decrypt(&message, &receiver, &sub.auth),
        json!({
            "task": task, "at": at.as_millisecond(), "title": expected_title,
        })
    );
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

fn commit_alarm(engine: &Engine, at: Timestamp, title: &str) -> Uuid {
    let (list, task, reminder) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let actor = Actor::owner("test").unwrap();
    for body in [
        json!({"cmd":"create_list", "id":list, "name":"Test list"}),
        json!({"cmd":"create_task", "id":task, "list":list, "title":title, "notes":"Never in a push"}),
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
    task
}

#[tokio::test]
async fn stale_logout_without_cookie_cannot_revoke_a_live_session() {
    let (_dir, server) = server().await;
    let auth = server.auth_service();
    let store = auth.store();
    let mut row = store.session("fixture").unwrap().unwrap();
    row.id_hash = "logout-fixture".into();
    row.created_at = auth.now();
    row.last_seen_at = row.created_at;
    store.create_session(&row, 1).unwrap();
    let sub = subscription(0);
    store
        .register_web_push(&row.id_hash, "key", &sub, row.created_at)
        .unwrap();
    assert!(!auth.end_expired_browser(&row.csrf, None, None).unwrap());
    assert!(
        store
            .has_web_push(&row.id_hash, sub.endpoint.as_str(), "key", row.created_at)
            .unwrap()
    );
    store.expire_session(&row.id_hash).unwrap();
    assert!(!auth.end_expired_browser("wrong-token", None, None).unwrap());
    assert!(auth.end_expired_browser(&row.csrf, None, None).unwrap());
    assert!(
        !store
            .has_web_push(&row.id_hash, sub.endpoint.as_str(), "key", row.created_at)
            .unwrap()
    );
}

#[tokio::test]
async fn confirmation_refreshes_browser_activity_after_a_new_sign_in() {
    let (_dir, server) = server().await;
    let auth = server.auth_service();
    let store = auth.store();
    let sub = subscription(0);
    let now = 42;
    let mut new_session = store.session("fixture").unwrap().unwrap();
    store
        .register_web_push("fixture", "key", &sub, now)
        .unwrap();
    let id = store.web_push_browsers("fixture", now).unwrap()[0].id;
    store.expire_session("fixture").unwrap();
    let seen = now + INACTIVITY_MS - 1;
    new_session.id_hash = "new-login".into();
    store.create_session(&new_session, 1).unwrap();
    let mut wrong = subscription(0);
    wrong.auth[0] ^= 1;
    assert!(
        !store
            .confirm_web_push("new-login", "key", &wrong, seen)
            .unwrap()
    );
    assert!(
        store
            .confirm_web_push("new-login", "key", &sub, seen)
            .unwrap()
    );
    let rows = store.web_push_browsers("new-login", seen).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    assert!(rows[0].current);
    assert_eq!(rows[0].last_seen_at, seen);
    assert!(
        store
            .claim_web_push(id, "task", seen, seen)
            .unwrap()
            .is_some()
    );
    store.prune_web_push(seen + INACTIVITY_MS - 1, 0).unwrap();
    assert!(
        store
            .has_web_push(
                "new-login",
                sub.endpoint.as_str(),
                "key",
                seen + INACTIVITY_MS - 1
            )
            .unwrap()
    );
    store.prune_web_push(seen + INACTIVITY_MS, 0).unwrap();
    assert!(
        !store
            .confirm_web_push("new-login", "key", &sub, seen + INACTIVITY_MS)
            .unwrap()
    );
    assert!(
        !auth.revoke_web_push_browser(id, None).unwrap(),
        "pruning removes the browser row and cascades to its subscription"
    );
}

#[tokio::test]
async fn retries_503_and_429_without_exceeding_ttl() {
    const START: i64 = 1_800_000_000_000;
    let clock = Arc::new(AtomicI64::new(START));
    let reading = clock.clone();
    let (_dir, server) = server_with(Options {
        clock: Arc::new(move || {
            Timestamp::from_millisecond(reading.load(Ordering::Relaxed)).unwrap()
        }),
        ..Options::default()
    })
    .await;
    let auth = server.auth_service();
    let engine = server.engine();
    let mut sender = Sender::new(secret_key().unwrap(), None).unwrap();
    sender.client = Client::builder().no_proxy().build().unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let router = Router::new().fallback(post(move |headers: HeaderMap| async move {
        let n = count.fetch_add(1, Ordering::Relaxed);
        match n {
            0 => {
                assert_eq!(headers["ttl"], "900");
                StatusCode::SERVICE_UNAVAILABLE
            }
            1 => {
                assert_eq!(headers["ttl"], "885");
                return (StatusCode::TOO_MANY_REQUESTS, [(RETRY_AFTER, "90")]).into_response();
            }
            2 => {
                assert_eq!(headers["ttl"], "795");
                StatusCode::CREATED
            }
            3 => StatusCode::SERVICE_UNAVAILABLE,
            _ => panic!("unexpected Web Push retry"),
        }
        .into_response()
    }));
    use axum::response::IntoResponse as _;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    sender.test_destination =
        Some(Url::parse(&format!("http://{}/send", listener.local_addr().unwrap())).unwrap());
    let mock = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    auth.store()
        .register_web_push("fixture", sender.public_key(), &subscription(0), START)
        .unwrap();
    let task = commit_alarm(
        &engine,
        Timestamp::from_millisecond(START).unwrap(),
        "Retry me",
    );
    let mut finished = BTreeSet::new();
    for (offset, call_count, next) in [
        (0, 1, Some(15_000)), // 503 -> first backoff
        (14_999, 1, Some(15_000)),
        (15_000, 2, Some(105_000)), // 429 -> Retry-After beats exponential backoff
        (104_999, 2, Some(105_000)),
        (105_000, 3, None),
        (106_000, 3, None),
    ] {
        clock.store(START + offset, Ordering::Relaxed);
        assert_eq!(
            sender.rescan(&engine, &auth, &mut finished).await,
            next.map(|at| START + at)
        );
        assert_eq!(calls.load(Ordering::Relaxed), call_count);
    }
    assert!(finished.contains(&(START, TaskId::from_uuid(task))));
    let second_at = START + TTL_MS;
    commit_alarm(
        &engine,
        Timestamp::from_millisecond(second_at).unwrap(),
        "Do not retry late",
    );
    clock.store(second_at, Ordering::Relaxed);
    assert_eq!(
        sender.rescan(&engine, &auth, &mut finished).await,
        Some(second_at + 15_000)
    );
    clock.store(second_at + TTL_MS, Ordering::Relaxed);
    assert_eq!(sender.rescan(&engine, &auth, &mut finished).await, None);
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    assert_eq!(next_retry(START, START, 1, RetryAfter::parse("900")), None);
    let date =
        httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs((START / 1000 + 60) as u64));
    assert_eq!(
        next_retry(START, START, 1, RetryAfter::parse(&date)),
        Some(START + 60_000)
    );
    mock.abort();
}

#[test]
fn warns_at_most_once_per_interval_on_auth_failure() {
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);
    impl io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let output = Log::default();
    let log = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || log.clone())
        .finish();
    let sender = Sender::new(secret_key().unwrap(), None).unwrap();
    tracing::subscriber::with_default(subscriber, || {
        assert_eq!(
            sender.rejected(StatusCode::UNAUTHORIZED, Some("do not log this")),
            Delivery::Failed
        );
        assert_eq!(
            sender.rejected(StatusCode::FORBIDDEN, None),
            Delivery::Failed
        );
    });
    let raw = output.0.lock().unwrap();
    let line = std::str::from_utf8(&raw).unwrap();
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(
        line.contains("WARN") && line.contains("status=401"),
        "{line}"
    );
    assert!(!line.contains("do not log this"));
}
