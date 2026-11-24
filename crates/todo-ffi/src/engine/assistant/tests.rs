#![allow(clippy::unwrap_used)]

use super::*;
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use todo_client::assistant::{
    ConfirmedSnapshot, Revision,
    fake::{CAPABILITIES, FakeProvider, FakeRound, MODEL},
    provider::{StreamEvent, ToolCall},
};

fn now() -> LocalNow {
    LocalNow {
        epoch_ms: 0,
        utc_offset_secs: 0,
        zone_id: "UTC".into(),
        local_day: None,
        date_format: None,
    }
}

fn server() -> AssistantServer {
    AssistantServer {
        address: "https://todo.example.com".into(),
        model: MODEL.into(),
        private_server: true,
    }
}

#[derive(Default)]
struct FixturePlatform {
    keys: Mutex<HashMap<Vec<u8>, Zeroizing<Vec<u8>>>>,
    addresses: Vec<String>,
}

impl AssistantPlatform for FixturePlatform {
    // A handle into fixture memory stands in for ciphertext; no fixture key goes on disk.
    fn seal_key(&self, plain: Vec<u8>) -> Option<Vec<u8>> {
        let handle = Uuid::new_v4().into_bytes().to_vec();
        locked(&self.keys).insert(handle.clone(), Zeroizing::new(plain));
        Some(handle)
    }

    fn open_key(&self, sealed: Vec<u8>) -> Option<Vec<u8>> {
        locked(&self.keys).get(&sealed).map(|key| key.to_vec())
    }

    fn destroy_key(&self) -> bool {
        locked(&self.keys).clear();
        true
    }

    fn vpn_addresses(&self) -> Vec<String> {
        self.addresses.clone()
    }
}

fn host() -> (Arc<AndroidAssistant>, tempfile::TempDir) {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ai = AndroidAssistant::open(
        engine,
        dir.path().join("assistant.json").to_string_lossy().into(),
        Arc::new(FixturePlatform::default()),
    );
    ai.resume();
    (ai, dir)
}

fn snapshot(state: Arc<State>, cursor: u64) -> ConfirmedSnapshot {
    ConfirmedSnapshot {
        revision: Revision {
            epoch: Uuid::nil(),
            cursor,
        },
        state,
        stale: false,
    }
}

#[test]
fn proposal_dates_follow_the_phone_preference_and_local_year() {
    use todo_client::assistant::{ListRef, TaskRef};
    use todo_core::{
        event::Change,
        ids::{ListId, TaskId},
        model::Priority,
        time::Timestamp,
    };

    let before = Some("2026-11-19".parse().unwrap());
    let after = Some("2026-11-23".parse().unwrap());
    let next_year = Some("2027-01-04".parse().unwrap());
    let task = TaskRef {
        id: TaskId::from_uuid(Uuid::nil()),
        title: "Review the budget".into(),
        list: ListRef {
            id: ListId::from_uuid(Uuid::nil()),
            name: "Reading".into(),
        },
        due: after,
    };
    let edited = Effect::Edited {
        task: task.clone(),
        changes: vec![
            Edit::Due(Change {
                old: before,
                new: after,
            }),
            Edit::StartDate(Change {
                old: None,
                new: next_year,
            }),
        ],
    };
    let created = Effect::Created {
        task,
        priority: Priority::None,
        status: WorkflowStatus::Todo,
        placement: Placement::Last,
    };
    for (format, old, new, next) in [
        (None, "Thu 19.11.", "Mon 23.11.", "Mon 04.01.2027"),
        (
            Some("day_month_name"),
            "Thu 19 Nov",
            "Mon 23 Nov",
            "Mon 4 Jan 2027",
        ),
        (
            Some("month_name_day"),
            "Thu Nov 19",
            "Mon Nov 23",
            "Mon Jan 4, 2027",
        ),
        (
            Some("iso"),
            "Thu 2026-11-19",
            "Mon 2026-11-23",
            "Mon 2027-01-04",
        ),
        (
            Some("numeric_us"),
            "Thu 11/19",
            "Mon 11/23",
            "Mon 01/04/2027",
        ),
    ] {
        // Still 2026 in UTC, already 2027 in Prague. First exercise both years in 2026.
        let mut phone = LocalNow {
            epoch_ms: "2026-11-21T09:35:00Z"
                .parse::<Timestamp>()
                .unwrap()
                .as_millisecond(),
            zone_id: "Europe/Prague".into(),
            date_format: format.map(str::to_owned),
            ..now()
        };
        let dates = Now::new(phone.clone()).dates();
        let edit = effect(&edited, dates, None);
        assert_eq!(edit.fields[0].before.as_deref(), Some(old));
        assert_eq!(edit.fields[0].after, new);
        assert_eq!(edit.fields[1].before.as_deref(), Some("No date"));
        assert_eq!(edit.fields[1].after, next);
        assert_eq!(effect(&created, dates, None).fields[0].after, new);

        phone.epoch_ms = "2026-12-31T23:30:00Z"
            .parse::<Timestamp>()
            .unwrap()
            .as_millisecond();
        let dates = Now::new(phone).dates();
        assert_eq!(dates.today.year(), 2027);
        let edit = effect(&edited, dates, None);
        if format.is_none() {
            assert_eq!(edit.fields[0].before.as_deref(), Some("Thu 19.11.2026"));
            assert_eq!(edit.fields[0].after, "Mon 23.11.2026");
            assert_eq!(edit.fields[1].after, "Mon 04.01.");
        }
    }
}

#[test]
fn unverified_https_and_public_http_are_refused_and_access_is_inherited() {
    let (ai, _temp) = host();
    assert!(ai.panel(now()).server.is_none());
    let mut bad = server();
    for address in [
        "http://todo.example.com",
        "http://sample.example.ts.net.evil.example",
        "http://100.63.255.255",
        "http://100.128.0.1",
        "http://192.168.0.1",
        "https://user@todo.example.com",
        "https://todo.example.com/?q=1",
    ] {
        bad.address = address.into();
        assert!(ai.save_server(Some(bad.clone()), None).is_err());
    }
    bad = server();
    bad.private_server = false;
    assert!(ai.save_server(Some(bad), None).is_err());
    ai.save_server(Some(server()), None).unwrap();
    let before = ai.panel(now());
    assert!(
        before
            .access
            .iter()
            .all(|row| row.access == Some(AssistantAccess::Hidden))
    );
    let collection = before.access.iter().find(|row| row.collection).unwrap();
    ai.set_access(collection.id.clone(), true, AssistantAccess::PrivateOnly)
        .unwrap();
    let after = ai.panel(now());
    let member = after
        .access
        .iter()
        .find(|row| row.nested && row.access == Some(AssistantAccess::PrivateOnly))
        .unwrap();
    assert!(member.inherited);
    ai.set_access(member.id.clone(), false, AssistantAccess::AllModels)
        .unwrap();
    let reloaded = AndroidAssistant::open(
        ai.engine.clone(),
        ai.path.to_string_lossy().into(),
        ai.platform.clone(),
    );
    let loaded = reloaded.panel(now());
    assert_eq!(loaded.server, Some(server()));
    assert_eq!(
        loaded
            .access
            .iter()
            .find(|row| row.id == member.id)
            .unwrap()
            .access,
        Some(AssistantAccess::AllModels)
    );
    reloaded.shutdown();
    assert!(reloaded.panel(now()).access.is_empty());
    assert!(reloaded.panel(now()).server.is_none());
}

#[tokio::test]
async fn destination_classification_matches_desktop_and_tailnet_http_fails_closed_without_a_vpn() {
    let (ai, _temp) = host();
    for (address, label, tailnet_http) in [
        ("http://127.0.0.1:8000", "This device", false),
        ("http://sample.example.ts.net:8000/v1", "Tailscale", true),
        ("http://100.64.0.1:8000/v1", "Tailscale", true),
        ("https://sample.example.ts.net/v1", "Tailscale", false),
    ] {
        let info = assistant_endpoint(address.into(), false).unwrap();
        assert_eq!(info.label, label);
        assert_eq!(info.tailnet_http, tailnet_http);
        assert!(!info.needs_attestation);
        ai.save_server(
            Some(AssistantServer {
                address: address.into(),
                private_server: false,
                ..server()
            }),
            None,
        )
        .unwrap();
        assert_eq!(ai.panel(now()).destination.as_deref(), Some(label));
        let provider = locked(&ai.session).provider.clone().unwrap();
        assert!(ListAccess::PrivateOnly.allows(provider.capabilities(MODEL).data_class));
        if tailnet_http {
            assert_eq!(
                provider.list_models().await.unwrap_err(),
                ProviderError::TailnetDisconnected
            );
            assert_eq!(
                provider.probe(MODEL).await.unwrap_err(),
                ProviderError::TailnetDisconnected
            );
        }
    }
    for (address, accepted) in [
        ("192.168.0.8", false),
        ("100.128.0.1", false),
        ("100.64.0.1", true),
        ("fd7a:115c:a1e0::1", true),
    ] {
        assert_eq!(
            AndroidVpn(Arc::new(FixturePlatform {
                addresses: vec![address.into()],
                ..Default::default()
            }))
            .connected(),
            accepted
        );
    }
}

const FIXTURE_BEARER: &str = "fixture-model-key";

#[tokio::test]
async fn bearer_is_sealed_bound_to_the_endpoint_and_never_sent_after_a_redirect() {
    let (ai, _temp) = host();
    let leaked = Arc::new(AtomicUsize::new(0));
    let target = {
        let leaked = leaked.clone();
        Router::new().fallback(move || async move {
            leaked.fetch_add(1, Ordering::Relaxed);
            StatusCode::OK
        })
    };
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect = format!("http://{}/elsewhere", tcp.local_addr().unwrap());
    let target = tokio::spawn(async { axum::serve(tcp, target).await.unwrap() });
    let authenticated = Arc::new(AtomicUsize::new(0));
    let handler = {
        let authenticated = authenticated.clone();
        move |headers: HeaderMap| {
            let redirect = redirect.clone();
            let authenticated = authenticated.clone();
            async move {
                if headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    == Some(&format!("Bearer {FIXTURE_BEARER}"))
                {
                    authenticated.fetch_add(1, Ordering::Relaxed);
                    (StatusCode::TEMPORARY_REDIRECT, [("location", redirect)])
                } else {
                    (StatusCode::UNAUTHORIZED, [("location", String::new())])
                }
            }
        }
    };
    let router = Router::new()
        .route("/v1/models", get(handler.clone()))
        .route("/v1/chat/completions", post(handler));
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = AssistantServer {
        address: format!("http://{}", tcp.local_addr().unwrap()),
        private_server: false,
        ..server()
    };
    let origin = tokio::spawn(async { axum::serve(tcp, router).await.unwrap() });
    ai.save_server(Some(local.clone()), None).unwrap();
    let provider = locked(&ai.session).provider.clone().unwrap();
    assert_eq!(
        provider.list_models().await.unwrap_err(),
        ProviderError::Unauthorized
    );
    ai.save_server(Some(local.clone()), Some(FIXTURE_BEARER.into()))
        .unwrap();
    let settings_file = fs::read_to_string(&ai.path).unwrap();
    assert!(!settings_file.contains(FIXTURE_BEARER));
    let reloaded = AndroidAssistant::open(
        ai.engine.clone(),
        ai.path.to_string_lossy().into(),
        ai.platform.clone(),
    );
    reloaded.resume();
    assert!(reloaded.panel(now()).has_api_key);
    let provider = locked(&reloaded.session).provider.clone().unwrap();
    assert_eq!(
        provider.list_models().await.unwrap_err(),
        ProviderError::InvalidResponse
    );
    assert!(provider.probe(MODEL).await.is_err());
    assert!(authenticated.load(Ordering::Relaxed) >= 2);
    assert_eq!(
        leaked.load(Ordering::Relaxed),
        0,
        "not even a keyless redirected request is permitted"
    );

    let fail_closed = AndroidAssistant::open(
        ai.engine.clone(),
        ai.path.to_string_lossy().into(),
        Arc::new(FixturePlatform::default()),
    );
    assert!(fail_closed.panel(now()).destination.is_none());
    let saved = read_settings(&ai.path).unwrap();
    assert!(
        open_key(
            &*ai.platform,
            "https://todo.example.com",
            saved.sealed_key.as_deref()
        )
        .is_err()
    );
    let first_seal = saved.sealed_key.unwrap();
    reloaded
        .save_server(Some(local.clone()), Some("fixture-key-replacement".into()))
        .unwrap();
    assert!(
        ai.platform.open_key(first_seal).is_none(),
        "the old seal must be unusable"
    );
    let replacement_seal = read_settings(&ai.path).unwrap().sealed_key.unwrap();
    // Replacing the destination never carries the key forward, even if the model is unchanged.
    reloaded.save_server(Some(server()), None).unwrap();
    assert!(ai.platform.open_key(replacement_seal).is_none());
    assert!(!reloaded.panel(now()).has_api_key);
    reloaded
        .save_server(Some(local.clone()), Some(FIXTURE_BEARER.into()))
        .unwrap();
    let remove_seal = read_settings(&ai.path).unwrap().sealed_key.unwrap();
    reloaded
        .save_server(Some(local.clone()), Some(String::new()))
        .unwrap();
    assert!(!reloaded.panel(now()).has_api_key);
    assert!(ai.platform.open_key(remove_seal).is_none());
    reloaded
        .save_server(Some(local), Some(FIXTURE_BEARER.into()))
        .unwrap();
    let disconnect_seal = read_settings(&ai.path).unwrap().sealed_key.unwrap();
    reloaded.save_server(None, None).unwrap();
    assert!(ai.platform.open_key(disconnect_seal).is_none());
    assert!(read_settings(&ai.path).unwrap().sealed_key.is_none());
    assert!(!reloaded.panel(now()).has_api_key);
    origin.abort();
    target.abort();
}

#[test]
fn failed_replacement_cannot_use_a_retired_key() {
    let (ai, _temp) = host();
    ai.save_server(Some(server()), Some(FIXTURE_BEARER.into()))
        .unwrap();
    let seal = read_settings(&ai.path).unwrap().sealed_key.unwrap();
    assert!(
        ai.save_server(Some(server()), Some("invalid\nkey".into()))
            .is_err()
    );
    assert!(
        ai.platform.open_key(seal.clone()).is_some(),
        "validate before destroying the alias"
    );
    fs::create_dir(ai.path.with_extension("json.new")).unwrap();
    assert!(
        ai.save_server(Some(server()), Some("fixture-key-replacement".into()))
            .is_err()
    );
    assert!(ai.platform.open_key(seal).is_none());
    assert!(
        ai.panel(now()).destination.is_none(),
        "do not keep the old authenticated transport"
    );
}

#[tokio::test]
async fn ffi_rules_keep_private_only_and_hidden_lists_out_of_cloud_requests() {
    let (ai, _temp) = host();
    let panel = ai.panel(now());
    let mut lists = panel.access.iter().filter(|row| !row.collection);
    let visible = lists.next().unwrap();
    let private = lists.next().unwrap();
    let hidden = lists.next().unwrap();
    ai.set_access(visible.id.clone(), false, AssistantAccess::AllModels)
        .unwrap();
    ai.set_access(private.id.clone(), false, AssistantAccess::PrivateOnly)
        .unwrap();
    let provider = Arc::new(FakeProvider::new(
        todo_client::assistant::provider::Capabilities {
            data_class: DataClass::Cloud,
            ..CAPABILITIES
        },
        [
            Ok(FakeRound::new([
                Ok(StreamEvent::Tool(
                    ToolCall::parse("call".into(), "list_lists".into(), "{}").unwrap(),
                )),
                Ok(StreamEvent::Finished),
            ])),
            Ok(FakeRound::new([Ok(StreamEvent::Finished)])),
        ],
    ));
    // The production host only permits local providers. Exercise its selection construction with
    // synthetic cloud capabilities so any future provider still uses the exact same FFI rules.
    let chat = Controller::new(TokioClock, OsEntropy);
    chat.configure(Some(selection(
        provider.clone(),
        &server(),
        &read_settings(&ai.path).unwrap().access,
    )));
    chat.update_snapshot(Some(snapshot(ai.engine.state(), 1)));
    chat.start(SendIntent {
        generation: chat.view().generation,
        sequence: 1,
        prompt: "Which lists can you see?".into(),
        context: Now::new(now()).eval(),
    })
    .unwrap()
    .await
    .unwrap();
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].list_scope,
        [list_id(&visible.id).unwrap()].into_iter().collect()
    );
    let history = serde_json::to_string(&requests[1].history).unwrap();
    assert!(history.contains(&visible.id));
    for entry in [private, hidden] {
        assert!(!history.contains(&entry.id));
        assert!(!history.contains(&entry.name));
    }
}

async fn mock_model(
    ai: &AndroidAssistant,
    id: String,
    titles: &'static [&'static str],
) -> JoinHandle<()> {
    async fn completion(
        id: String,
        titles: &[&str],
        headers: HeaderMap,
        body: Value,
    ) -> impl axum::response::IntoResponse {
        assert!(!headers.contains_key("authorization"));
        let round = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "tool")
            .count();
        let (delta, reason) = if let Some(title) = titles.get(round) {
            let function = json!({"name":"create_task", "arguments": json!({"list_id":id,"title":title}).to_string()});
            let call = json!({"index":0,"id":format!("call_{round}"),"type":"function","function":function});
            (json!({"tool_calls":[call]}), "tool_calls")
        } else {
            (json!({"content":"Ready to review."}), "stop")
        };
        let chunk = |delta, reason| {
            format!(
                "data: {}\n\n",
                json!({"choices":[{"index":0,"delta":delta,"finish_reason":reason}]})
            )
        };
        (
            [("content-type", "text/event-stream")],
            format!(
                "{}{}data: [DONE]\n\n",
                chunk(delta, Value::Null),
                chunk(json!({}), json!(reason))
            ),
        )
    }
    let router = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            completion(id.clone(), titles, headers, body)
        }),
    );
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = format!("http://{}", tcp.local_addr().unwrap());
    let fixture = tokio::spawn(async { axum::serve(tcp, router).await.unwrap() });
    ai.save_server(
        Some(AssistantServer {
            address: local,
            private_server: false,
            ..server()
        }),
        None,
    )
    .unwrap();
    fixture
}

async fn draft(ai: &Arc<AndroidAssistant>, generation: u64) -> AssistantProposal {
    ai.clone()
        .send(generation, 1, "Make a task".into(), now())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let version = ai.chat.view_version();
            let panel = ai.panel(now());
            if !panel.streaming {
                break panel.turns.into_iter().last().unwrap().proposal.unwrap();
            }
            ai.chat.changed(version).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn streamed_effect_needs_the_displayed_digest_and_confirmed_revision() {
    let (ai, _temp) = host();
    let list = ai
        .panel(now())
        .access
        .into_iter()
        .find(|row| !row.collection)
        .unwrap();
    ai.set_access(list.id.clone(), false, AssistantAccess::PrivateOnly)
        .unwrap();
    let fixture = mock_model(&ai, list.id, &["Collect the books"]).await;
    ai.chat
        .update_snapshot(Some(snapshot(ai.engine.state(), 1)));
    let generation = ai.panel(now()).generation;
    let proposal = draft(&ai, generation).await;
    assert_eq!(proposal.status, AssistantProposalStatus::Pending);
    assert_eq!(proposal.items[0].effects[0].title, "Collect the books");
    let mut forged = proposal.digest.clone();
    forged[0] ^= 1;
    assert!(ai.approve(proposal.id.clone(), forged).is_err());
    assert!(
        ai.approve(proposal.id.clone(), proposal.digest.clone())
            .is_err(),
        "fixture sync is not caught up"
    );
    assert_eq!(ai.engine.status().pending, 0);
    ai.chat
        .update_snapshot(Some(snapshot(ai.engine.state(), 2)));
    assert_eq!(
        ai.panel(now()).turns[0].proposal.as_ref().unwrap().status,
        AssistantProposalStatus::Stale
    );
    ai.reject(proposal.id, proposal.digest).unwrap();
    assert_eq!(
        ai.panel(now()).turns[0].proposal.as_ref().unwrap().status,
        AssistantProposalStatus::Rejected
    );
    ai.pause();
    assert!(
        ai.clone()
            .send(generation, 2, "A second request".into(), now())
            .is_err()
    );
    ai.engine.client.revocation().observe_status(401);
    ai.resume();
    let signed_out = ai.panel(now());
    assert!(
        signed_out.server.is_none() && signed_out.turns.is_empty() && signed_out.access.is_empty()
    );
    assert!(
        ai.send(signed_out.generation, 3, "A third request".into(), now())
            .is_err()
    );
    fixture.abort();
}

async fn until(mut done: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approved_subset_syncs_once_and_can_be_undone() {
    use crate::{
        EngineConfig,
        view::{RoomKey, SmartList},
    };
    use todo_server::{Config, Options, Server};

    let data = tempfile::tempdir().unwrap();
    let paired = Server::bind(
        &Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: data.path().to_owned(),
            ..Config::default()
        },
        Options::default(),
    )
    .await
    .unwrap()
    .spawn()
    .unwrap();
    let (device, token) = paired.authsvc.create_device("Test phone", None).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let engine = TodoEngine::open(EngineConfig {
        server_url: paired.url(),
        token,
        device_id: device.to_string(),
        database_path: cache.path().join("cache.sqlite").to_string_lossy().into(),
        allow_loopback_http: true,
    })
    .unwrap();
    let RoomKey::List { id } = engine
        .create_list(
            "Reading".into(),
            RoomKey::Smart {
                list: SmartList::MyDay,
            },
        )
        .unwrap()
    else {
        panic!("expected list")
    };
    let ai = AndroidAssistant::open(
        engine.clone(),
        cache.path().join("assistant.json").to_string_lossy().into(),
        Arc::new(FixturePlatform::default()),
    );
    ai.resume();
    engine.start().unwrap();
    until(|| {
        engine.status().caught_up
            && engine.status().pending == 0
            && ai.panel(now()).access.len() == 1
    })
    .await;
    ai.set_access(id.clone(), false, AssistantAccess::PrivateOnly)
        .unwrap();
    let fixture = mock_model(&ai, id, &["Collect the books", "Return the maps"]).await;
    let generation = ai.panel(now()).generation;
    let original = draft(&ai, generation).await;
    assert_eq!(original.items.len(), 2);
    assert_eq!(engine.state().tasks.len(), 0);
    ai.select_items(original.id.clone(), original.digest.clone(), vec![1])
        .unwrap();
    let subset = ai.panel(now()).turns.pop().unwrap().proposal.unwrap();
    assert_ne!(original.digest, subset.digest);
    assert!(!subset.items[0].selected && subset.items[1].selected);
    assert!(ai.approve(original.id, original.digest).is_err());
    ai.approve(subset.id.clone(), subset.digest.clone())
        .unwrap();
    assert!(ai.approve(subset.id.clone(), subset.digest).is_err());
    until(|| {
        ai.panel(now())
            .turns
            .last()
            .and_then(|t| t.proposal.as_ref())
            .is_some_and(|p| p.status == AssistantProposalStatus::Applied)
    })
    .await;
    assert_eq!(engine.state().tasks.len(), 1);
    assert_eq!(
        engine.state().tasks.values().next().unwrap().title,
        "Return the maps"
    );
    assert!(
        matches!(&engine.state().tasks.values().next().unwrap().created.by,
        todo_core::Actor::Agent { agent_id } if agent_id.as_str() == "assistant")
    );
    ai.undo(subset.id).unwrap();
    until(|| {
        engine.status().pending == 0
            && engine
                .state()
                .tasks
                .values()
                .all(|task| task.deleted.is_some())
    })
    .await;
    engine.stop();
    fixture.abort();
}
