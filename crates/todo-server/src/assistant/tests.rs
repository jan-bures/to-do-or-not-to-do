use super::*;
use axum::{http::HeaderMap, response::IntoResponse};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
};
use todo_tools::assistant::access::ListAccess;
use tokio::{
    net::TcpListener,
    sync::{Notify, oneshot},
    task::JoinHandle,
};

const STATUS: &str = "/api/v1/assistant/status";
const ACCESS: &str = "/api/v1/assistant/access";
const CHATGPT: &str = "/api/v1/assistant/chatgpt";
const CLIENT: &str = "fixture-client";

fn config(dir: &Path) -> Config {
    Config {
        chatgpt: true,
        key_file: Some(dir.join("assistant.secret")),
        ..Config::default()
    }
}

fn service(openai: OpenAi, dir: &Path) -> Arc<Assistant> {
    let mut service = Assistant::open(config(dir), dir).unwrap().unwrap();
    Arc::get_mut(&mut service).unwrap().openai = Some(openai);
    service
}

fn auth() -> Credential {
    Credential {
        flow: Flow::Device,
        client_id: CLIENT.into(),
        subject: "fixture-subject".into(),
        workspace: Some("fixture-workspace".into()),
        label: "Example account".into(),
        access: "fixture-access".into(),
        refresh: "fixture-refresh".into(),
        expires_at: openai::now() + 3600,
        scope: None,
        approved: false,
        approved_lists: BTreeSet::new(),
    }
}

async fn upstream(router: Router) -> (OpenAi, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (OpenAi::at_fixture(issuer, Some(CLIENT.into())), task)
}

#[tokio::test]
async fn cancelled_refresh_still_commits_once_and_does_not_block_other_records() {
    let dir = tempfile::tempdir().unwrap();
    let entered = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new().route("/oauth/token", post({
        let (entered, finish, calls) = (entered.clone(), finish.clone(), calls.clone());
        move |Json(body): Json<Value>| async move {
            assert_eq!(body["refresh_token"], "fixture-refresh");
            assert_eq!(calls.fetch_add(1, Ordering::SeqCst), 0);
            entered.notify_one();
            finish.notified().await;
            Json(json!({"access_token":"fixture-new-access","refresh_token":"fixture-rotated","expires_in":3600}))
        }
    }));
    let (openai, mock) = upstream(router).await;
    let service = service(openai, dir.path());
    let mut auth = auth();
    auth.expires_at = 0;
    let id = service.insert(auth.clone()).await.unwrap().id;
    let caller = tokio::spawn({
        let service = service.clone();
        async move { service.credential(id, None).await }
    });
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    // Neither an account edit nor a status read should wait for the upstream exchange.
    tokio::time::timeout(Duration::from_secs(3), async {
        auth.subject = "other-account".into();
        service.insert(auth).await.unwrap();
        assert_eq!(service.status().await.accounts.len(), 2);
        let guard = service.records.clone().lock_owned().await;
        let mut records = guard.clone();
        records.accounts.get_mut(&id).unwrap().approved = true;
        service.commit(guard, records).await.unwrap();
    })
    .await
    .unwrap();
    let concurrent = tokio::spawn({
        let service = service.clone();
        async move { service.credential(id, None).await }
    });
    finish.notify_one();
    // Another caller waits for the same rotation rather than spending the refresh token twice.
    let auth = tokio::time::timeout(Duration::from_secs(3), concurrent)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(auth.refresh, "fixture-rotated");
    let (_, restarted) = Vault::open(dir.path(), &dir.path().join("assistant.secret")).unwrap();
    assert_eq!(restarted.accounts[&id].refresh, "fixture-rotated");
    assert!(restarted.accounts[&id].approved);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    mock.abort();
}

struct Api {
    url: String,
    client: reqwest::Client,
    owner: String,
    other: String,
    stop: oneshot::Sender<()>,
    server: JoinHandle<()>,
}

impl Api {
    async fn start(service: Arc<Assistant>, dir: &Path) -> Self {
        let config = crate::Config {
            data_dir: dir.into(),
            bind: "127.0.0.1:0".parse().unwrap(),
            public_origin: Some("https://todo.example.com".into()),
            ..Default::default()
        };
        let options = crate::Options {
            assistant: Some(service),
            ..Default::default()
        };
        let server = crate::Server::bind(&config, options).await.unwrap();
        let (_, owner) = server
            .auth_service()
            .create_device("First device", None)
            .unwrap();
        let (_, other) = server
            .auth_service()
            .create_device("Second device", None)
            .unwrap();
        let url = format!("http://{}", server.local_addr().unwrap());
        let (stop, signal) = oneshot::channel();
        let server = tokio::spawn(async move {
            server
                .run_until(async {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        Self {
            url,
            client: upstream::client().build().unwrap(),
            owner,
            other,
            stop,
            server,
        }
    }

    async fn request(&self, verb: Method, path: &str, body: Value) -> reqwest::Response {
        self.as_owner(&self.owner, verb, path, body).await
    }

    async fn as_owner(
        &self,
        token: &str,
        verb: Method,
        path: &str,
        body: Value,
    ) -> reqwest::Response {
        self.client
            .request(verb, format!("{}{path}", self.url))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn status(&self) -> Status {
        self.request(Method::GET, STATUS, Value::Null)
            .await
            .json()
            .await
            .unwrap()
    }

    async fn stop(self) {
        self.stop.send(()).unwrap();
        self.server.await.unwrap();
    }
}

#[tokio::test]
async fn chat_uses_server_consent_refreshes_on_401_without_replaying_and_drops_consent_on_signout()
{
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/oauth/token", post(|Json(body): Json<Value>| async move {
            assert_eq!(body["refresh_token"], "fixture-refresh");
            Json(json!({"access_token":"fixture-new-access","refresh_token":"fixture-rotated","expires_in":3600}))
        }))
        .route("/oauth/revoke", post(|Json(body): Json<Value>| async move {
            assert_eq!(body["token"], "fixture-rotated");
            StatusCode::NO_CONTENT
        }))
        .route("/codex/responses", post({
            let calls = calls.clone();
            move |headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(body["store"], false);
                assert!(body.get("lists").is_none());
                assert_eq!(headers["chatgpt-account-id"], "fixture-workspace");
                match calls.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        assert_eq!(headers["authorization"], "Bearer fixture-access");
                        StatusCode::UNAUTHORIZED.into_response()
                    }
                    _ => {
                        assert_eq!(headers["authorization"], "Bearer fixture-new-access");
                        ([("content-type", "text/event-stream")], "data: fixture\n\n").into_response()
                    }
                }
            }
        }));
    let (openai, mock) = upstream(router).await;
    let service = service(openai, dir.path());
    let id = service.insert(auth()).await.unwrap().id;
    let api = Api::start(service, dir.path()).await;
    let list = Uuid::new_v4();
    assert!(api.request(Method::POST, "/api/v1/commands", json!({"command_id":Uuid::new_v4(),"command":{"cmd":"create_list","id":list,"name":"Sample"}})).await.status().is_success());
    let account = format!("{CHATGPT}/accounts/{id}");
    let chat = format!("{account}/chat");
    let consent = format!("{account}/consent");
    let request = |destination| json!({"destination":destination,"lists":[],"request":{"model":"fixture-model","input":[{"role":"user","content":"hello"}]}});
    let set_access = |version, access| json!({"access_version":version,"access":{"lists":{list.to_string():access}}});
    let initial = api.status().await;
    let update = api
        .request(
            Method::PUT,
            ACCESS,
            set_access(initial.access_version, ListAccess::AllModels),
        )
        .await;
    assert_eq!(update.status(), StatusCode::OK);
    let promoted: Status = update.json().await.unwrap();
    assert_eq!(
        api.request(Method::POST, &chat, request("local"))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let denied = api.request(Method::POST, &chat, request("cloud")).await;
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    assert_eq!(
        denied.json::<Value>().await.unwrap()["error"]["code"],
        "assistant_consent"
    );
    let approve = |version, lists: Vec<Uuid>| json!({"access_version":version,"lists":lists});
    assert_eq!(
        api.request(
            Method::PUT,
            &consent,
            approve(promoted.access_version, vec![])
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        api.request(
            Method::PUT,
            &consent,
            approve(promoted.access_version, vec![list])
        )
        .await
        .status(),
        StatusCode::OK
    );
    let denied = api.request(Method::POST, &chat, request("cloud")).await;
    assert_eq!(denied.status(), StatusCode::FAILED_DEPENDENCY);
    assert_eq!(
        denied.json::<Value>().await.unwrap()["error"]["code"],
        "assistant_upstream_auth"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no automatic replay of inference"
    );
    let ok = api.request(Method::POST, &chat, request("cloud")).await;
    assert_eq!(ok.text().await.unwrap(), "data: fixture\n\n");

    let demoted: Status = api
        .request(
            Method::PUT,
            ACCESS,
            set_access(promoted.access_version, ListAccess::PrivateOnly),
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(demoted.accounts[0].approved_lists.is_empty());
    let promoted: Status = api
        .request(
            Method::PUT,
            ACCESS,
            set_access(demoted.access_version, ListAccess::AllModels),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        api.request(Method::POST, &chat, request("cloud"))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        api.request(
            Method::PUT,
            &consent,
            approve(promoted.access_version, vec![list])
        )
        .await
        .status(),
        StatusCode::OK
    );
    let status = api
        .request(Method::GET, STATUS, Value::Null)
        .await
        .text()
        .await
        .unwrap();
    for secret in [
        "fixture-subject",
        "fixture-client",
        "fixture-workspace",
        "fixture-access",
        "fixture-new-access",
        "fixture-rotated",
        "fixture-refresh",
    ] {
        assert!(!status.contains(secret));
    }
    let signed_out: relay::SignOut = api
        .request(Method::DELETE, &account, Value::Null)
        .await
        .json()
        .await
        .unwrap();
    assert!(signed_out.revoked);
    api.stop().await;
    let restarted = Assistant::open(config(dir.path()), dir.path())
        .unwrap()
        .unwrap();
    assert!(restarted.status().await.accounts.is_empty());
    assert!(restarted.records.lock().await.revocations.is_empty());
    let again = restarted.insert(auth()).await.unwrap();
    assert_ne!(again.id, id);
    assert!(!again.approved && again.approved_lists.is_empty());
    mock.abort();
}

#[tokio::test]
async fn full_account_vault_revokes_the_unused_grant() {
    let dir = tempfile::tempdir().unwrap();
    let revoked = Arc::new(Notify::new());
    let router = Router::new().route(
        "/oauth/revoke",
        post({
            let revoked = revoked.clone();
            move |Json(body): Json<Value>| async move {
                assert_eq!(body["token"], "unused-fixture-grant");
                revoked.notify_one();
                StatusCode::NO_CONTENT
            }
        }),
    );
    let (openai, mock) = upstream(router).await;
    let service = service(openai, dir.path());
    for n in 0..MAX_ACCOUNTS {
        let mut auth = auth();
        auth.subject = format!("fixture-account-{n}");
        service.insert(auth).await.unwrap();
    }
    let mut auth = auth();
    auth.refresh = "unused-fixture-grant".into();
    assert!(matches!(
        service.accept(auth).await,
        Err(Error::AccountsFull)
    ));
    tokio::time::timeout(Duration::from_secs(3), revoked.notified())
        .await
        .unwrap();
    assert_eq!(service.status().await.accounts.len(), MAX_ACCOUNTS);
    mock.abort();
}

#[tokio::test]
async fn a_full_attempt_table_refuses_upstream_device_start_until_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new().route("/api/accounts/deviceauth/usercode", post({
        let calls = calls.clone();
        move || async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Json(json!({"device_auth_id":"fixture-request","usercode":"TEST-CODE","interval":"5"}))
        }
    }));
    let (openai, mock) = upstream(router).await;
    let mut service = service(openai, dir.path());
    Arc::get_mut(&mut service).unwrap().device_enabled = true;
    let api = Api::start(service.clone(), dir.path()).await;
    // Seed attempts directly instead of racing the slower rate limiter.
    let owner = Principal::Owner {
        device: todo_core::DeviceId::new("fixture").unwrap(),
    };
    for _ in 0..MAX_ATTEMPTS {
        service.attempts.lock().await.insert(
            Uuid::new_v4(),
            Attempt {
                owner: owner.clone(),
                valid_until: Instant::now() + LOOPBACK_EXPIRY,
                state: Arc::new(Mutex::new(AttemptState::Failed)),
            },
        );
    }
    let url = format!("{CHATGPT}/start");
    assert_eq!(
        api.request(Method::POST, &url, json!({"flow":"device"}))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    service
        .attempts
        .lock()
        .await
        .values_mut()
        .next()
        .unwrap()
        .valid_until = Instant::now();
    assert_eq!(
        api.request(Method::POST, &url, json!({"flow":"device"}))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    api.stop().await;
    mock.abort();
}

#[tokio::test]
async fn sign_out_waits_for_detached_refresh_and_revokes_the_rotated_grant() {
    let dir = tempfile::tempdir().unwrap();
    let entered = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let router = Router::new()
        .route("/oauth/token", post({
            let (entered, finish) = (entered.clone(), finish.clone());
            move || async move {
                entered.notify_one();
                finish.notified().await;
                Json(json!({"access_token":"fixture-new-access","refresh_token":"fixture-rotated","expires_in":3600}))
            }
        }))
        .route("/oauth/revoke", post(|Json(body): Json<Value>| async move {
            assert_eq!(body["token"], "fixture-rotated");
            StatusCode::NO_CONTENT
        }));
    let (openai, mock) = upstream(router).await;
    let service = service(openai, dir.path());
    let mut auth = auth();
    auth.expires_at = 0;
    let id = service.insert(auth).await.unwrap().id;
    let caller = tokio::spawn({
        let service = service.clone();
        async move { service.credential(id, None).await }
    });
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    caller.abort();
    let sign_out = async {
        let job = service.remove(id).await.unwrap();
        assert!(service.retry_revocation(job).await.unwrap());
    };
    tokio::pin!(sign_out);
    // Poll sign-out while the refresh still owns this account's gate.
    assert!(futures_util::poll!(&mut sign_out).is_pending());
    finish.notify_one();
    tokio::time::timeout(Duration::from_secs(3), sign_out)
        .await
        .unwrap();
    assert!(matches!(
        service.credential(id, None).await,
        Err(Error::AccountMissing)
    ));
    let (_, restarted) = Vault::open(dir.path(), &dir.path().join("assistant.secret")).unwrap();
    assert!(restarted.accounts.is_empty() && restarted.revocations.is_empty());
    mock.abort();
}

#[tokio::test]
async fn reconnect_queues_only_the_replaced_refresh_token_in_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let service = Assistant::open(config(dir.path()), dir.path())
        .unwrap()
        .unwrap();
    let id = service.insert(auth()).await.unwrap().id;
    assert_eq!(service.insert(auth()).await.unwrap().id, id);
    assert!(service.records.lock().await.revocations.is_empty());
    let mut fresh = auth();
    fresh.refresh = "fixture-reconnected".into();
    assert_eq!(service.insert(fresh).await.unwrap().id, id);
    let (_, restarted) = Vault::open(dir.path(), &dir.path().join("assistant.secret")).unwrap();
    assert_eq!(restarted.accounts[&id].refresh, "fixture-reconnected");
    assert_eq!(restarted.revocations.len(), 1);
    assert_eq!(
        restarted.revocations.values().next().unwrap().grant.refresh,
        "fixture-refresh"
    );
}

#[tokio::test]
async fn terminal_or_exhausted_revocations_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/oauth/revoke",
            post({
                let calls = calls.clone();
                move || async move {
                    match calls.fetch_add(1, Ordering::SeqCst) {
                        0 => StatusCode::BAD_REQUEST,
                        1 => StatusCode::TOO_MANY_REQUESTS,
                        _ => panic!("revocation should no longer retry"),
                    }
                }
            }),
        )
        .route(
            "/.well-known/openid-configuration",
            get(|| async { Json(json!({"issuer":"fixture"})) }),
        );
    let (openai, mock) = upstream(router).await;
    let service = service(openai, dir.path());
    for (flow, attempts, expired) in [
        (Flow::Device, 0, false),
        (Flow::Loopback, 0, false),
        (Flow::Device, vault::MAX_REVOCATION_ATTEMPTS - 1, false),
        (Flow::Device, 0, true),
    ] {
        let mut auth = auth();
        auth.flow = flow;
        let id = service.insert(auth).await.unwrap().id;
        let job = service.remove(id).await.unwrap();
        {
            let guard = service.records.clone().lock_owned().await;
            let mut records = guard.clone();
            let pending = records.revocations.get_mut(&job).unwrap();
            pending.attempts = attempts;
            if expired {
                pending.queued_at = openai::now() - 8 * 24 * 3600;
            }
            service.commit(guard, records).await.unwrap();
        }
        assert!(!service.retry_revocation(job).await.unwrap());
        assert!(service.records.lock().await.revocations.is_empty());
        assert!(!service.retry_revocation(job).await.unwrap());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    mock.abort();
}

#[cfg(not(target_os = "linux"))]
#[test]
fn an_enabled_relay_needs_an_explicit_key_path_outside_linux() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Config {
        chatgpt: true,
        ..Default::default()
    };
    assert!(matches!(
        Assistant::open(settings, dir.path()),
        Err(InitError::MissingKeyPath)
    ));
    assert!(!dir.path().join("assistant.vault").exists());
}

#[tokio::test]
async fn failed_revocation_is_backed_off_persisted_and_retried_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new().route(
        "/oauth/revoke",
        post({
            let calls = calls.clone();
            move |Json(body): Json<Value>| async move {
                assert_eq!(body["token"], "fixture-refresh");
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    StatusCode::BAD_GATEWAY
                } else {
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (openai, mock) = upstream(router).await;
    let issuer = openai.issuer.clone();
    let initial = service(openai, dir.path());
    let account = initial.insert(auth()).await.unwrap().id;
    let job = initial.remove(account).await.unwrap();
    assert!(!initial.retry_revocation(job).await.unwrap());
    assert!(!initial.retry_revocation(job).await.unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(initial);
    let after = service(OpenAi::at_fixture(issuer, None), dir.path());
    let guard = after.records.clone().lock_owned().await;
    assert!(guard.accounts.is_empty());
    let mut records = guard.clone();
    let pending = records.revocations.get_mut(&job).unwrap();
    assert_eq!(pending.attempts, 1);
    assert!(pending.retry_at > openai::now());
    pending.retry_at = 0;
    after.commit(guard, records).await.unwrap();
    let (stop, signal) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn({
        let after = after.clone();
        async move { after.run_revocations(signal).await }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !after.records.lock().await.revocations.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    worker.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    mock.abort();
}

#[tokio::test]
async fn handoff_is_expiring_owner_bound_and_one_shot() {
    // Public fixture key and signature over the invented claims below. No signing key is kept.
    const NONCE: &str = "fixture-nonce";
    const CLAIMS: &str = r#"{"iss":"https://auth.openai.com","sub":"fixture-subject","aud":"fixture-client","exp":4102444800,"email":"you@example.com","nonce":"fixture-nonce"}"#;
    const N: &str = "tyox82SFoiKekWg9Hmp68_DrIbILBH3vOz-R5JVfD4uda9Rp-9vdYJLHyP4N6wGhwsLI0UHLYE4Hj8q0QuZh9mQg632VOZz-LRlbFKz6qJ4ArZn0q2T6ZclE18zPXNriF6TsDusAnKMxwoXNHIJ0u6ezOqqHJV55QBZ0QylxMHabvVHQE9MMqfRlQ9sNtsIK4tTKPp2mmv_BGJDojGKiDbB9l4YJr7JwrDc8XfNb5zydPiAjXzQzzhiWATh8824J2SzMx1ZH1_5xl8bVZf_LogdIYPtQHc1JtTaeZVUl3axxe3-iqWq50nfXJXxer7rrGYwy_zvpjL0GFpweXRhVPw";
    const SIG: &str = "oNBIO4BZxCeLV3YE3qzioNKOUlBAU-EnwyMNOaVvKxNrbZdMTcdBK9RJ_Jon8-iKQ1b-IsQq7VXzzGWh1-1LuAdUw9_zdQGmbbebvnsbu7b4wJn1TRjUqOwG31s1_2u1iaZqzUA7rjFfIBxMcWbc4Hg05FXJjquUt3M6ChsOsqze7Unz0NxQUc_0nWZCJuBLkY__beUOMT1YfWlxc_Ej5JzwEJzmJtC9Z6_5pYuvI9PJO1lAzc97KATphx6pLdZDtvwayUm-XgkeMd8i9dNujD3p_GoyUi0MjJ0h7oW2A7TLXrsADx14nU5Ka3S7mevJTlThLZb-I28lAWtn4fYzgw";
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/api/accounts/oauth/token", post({
            let calls = calls.clone();
            move |body: String| async move {
                assert_eq!(calls.fetch_add(1, Ordering::SeqCst), 0);
                let fields = url::form_urlencoded::parse(body.as_bytes()).collect::<HashMap<_, _>>();
                assert_eq!(fields["client_id"], CLIENT);
                assert_eq!(fields["code"], "fixture-code");
                assert!(!fields["code_verifier"].is_empty());
                let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","kid":"fixture-handoff-key"}"#);
                let claims = URL_SAFE_NO_PAD.encode(CLAIMS);
                Json(json!({"access_token":"fixture-access","refresh_token":"fixture-refresh","token_type":"Bearer","scope":"resource.invoke chatgpt.tokens.use.direct offline_access","id_token":format!("{header}.{claims}.{SIG}")}))
            }
        }))
        .route("/.well-known/jwks.json", get(|| async { Json(json!({"keys":[{"kid":"fixture-handoff-key","kty":"RSA","n":N,"e":"AQAB"}]})) }));
    let dir = tempfile::tempdir().unwrap();
    let (openai, mock) = upstream(router).await;
    let service = service(openai, dir.path());
    let api = Api::start(service.clone(), dir.path()).await;
    for expired in [true, false] {
        let started = api
            .request(
                Method::POST,
                &format!("{CHATGPT}/start"),
                json!({"flow":"loopback","port":12480}),
            )
            .await
            .json::<relay::SignInStarted>()
            .await
            .unwrap();
        let relay::SignInStarted::Loopback {
            attempt,
            authorize_url,
        } = started
        else {
            panic!("expected loopback grant")
        };
        let state = url::Url::parse(&authorize_url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let slot = {
            let mut attempts = service.attempts.lock().await;
            let slot = attempts.get_mut(&attempt).unwrap();
            if expired {
                slot.valid_until = Instant::now();
            }
            slot.state.clone()
        };
        if let AttemptState::Loopback(grant) = &mut *slot.lock().await {
            grant.set_fixture_nonce(NONCE);
        }
        let url = format!("{CHATGPT}/handoff/{attempt}");
        let body = json!({"state":state,"code":"fixture-code","client_id":CLIENT});
        assert_eq!(
            api.as_owner(&api.other, Method::POST, &url, body.clone())
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let response = api.request(Method::POST, &url, body.clone()).await;
        if expired {
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            let account: Account = response.json().await.unwrap();
            assert_eq!(account.label, "you@example.com");
            assert_eq!(
                api.request(Method::POST, &url, body).await.status(),
                StatusCode::BAD_REQUEST
            );
            let poll = api
                .request(
                    Method::POST,
                    &format!("{CHATGPT}/poll/{attempt}"),
                    Value::Null,
                )
                .await
                .json::<Poll>()
                .await
                .unwrap();
            assert!(
                matches!(poll, Poll::SignedIn { account: replayed } if replayed.id == account.id)
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
    api.stop().await;
    mock.abort();
}

#[test]
fn cloud_request_forces_stream_and_disables_provider_storage_or_builtin_tools() {
    let mut request = relay::Chat {
        destination: DataClass::Cloud,
        lists: BTreeSet::new(),
        request: json!({"model":"example", "input":[{"role":"user","content":"hello"}],"store":true,"stream":false}),
    };
    validate_request(&mut request, DataClass::Cloud).unwrap();
    assert_eq!(request.request["stream"], true);
    assert_eq!(request.request["store"], false);
    request.request["tools"] = json!([{"type":"web_search"}]);
    assert!(validate_request(&mut request, DataClass::Cloud).is_err());
}
