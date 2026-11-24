#![allow(clippy::unwrap_used)]

use super::*;
use axum::{Json, Router, http::HeaderMap, routing::post};
use serde_json::{Value, json};
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

fn host() -> (Arc<AndroidAssistant>, tempfile::TempDir) {
    let engine = TodoEngine::fixture("world".into(), now()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ai = AndroidAssistant::open(
        engine,
        dir.path().join("assistant.json").to_string_lossy().into(),
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
fn only_explicit_https_settings_are_persisted_and_access_is_inherited() {
    let (ai, _temp) = host();
    assert!(ai.panel(now()).server.is_none());
    let mut bad = server();
    for address in [
        "http://127.0.0.1",
        "http://todo.example.com",
        "https://user@todo.example.com",
        "https://todo.example.com/?q=1",
    ] {
        bad.address = address.into();
        assert!(ai.save_server(Some(bad.clone())).is_err());
    }
    bad = server();
    bad.private_server = false;
    assert!(ai.save_server(Some(bad)).is_err());
    ai.save_server(Some(server())).unwrap();
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
    let reloaded = AndroidAssistant::open(ai.engine.clone(), ai.path.to_string_lossy().into());
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
    {
        let mut session = locked(&ai.session);
        // Test-only connection, impossible to install through save_server on Android.
        session.settings.server = Some(server());
        session.provider = Some(
            LocalProvider::new(&local, "")
                .unwrap()
                .with_support(MODEL.into(), Support::StreamingTools),
        );
        ai.install(&session);
    }
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
