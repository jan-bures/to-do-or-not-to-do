#![cfg(all(feature = "assistant", feature = "native"))]
#![allow(clippy::unwrap_used)]

mod server;

use serde_json::{Value, json};
use server::{
    Client, TOKEN_A, TOKEN_B, TestServer, alpha, beta, client, converged, eventually, spawn, stop,
};
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use todo_client::activity::{ActivityQuery, actor_name};
use todo_client::assistant::{
    ApprovalOutcome, Controller, ListAccess, ReviewError, Selection, SendIntent,
    fake::{CAPABILITIES, FakeProvider, FakeRound, MODEL},
    provider::{StreamEvent, ToolCall},
};
use todo_client::mirror::{MirrorStorage, Persisted};
use todo_client::native::{HttpTransport, OsEntropy, TokioClock};
use todo_client::platform::{
    EventStream, Request, Response, SseFrame, StreamOpen, Transport, TransportError,
};
use todo_client::protocol::APPROVALS_PATH;
use todo_client::{MemoryStorage, Operation, Storage, TrackedUndo};
use todo_core::{
    Command,
    ids::{ListId, TaskId},
    query::EvalContext,
    time::Timestamp,
};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(10);

fn command(v: Value) -> Command {
    serde_json::from_value(v).unwrap()
}

async fn review<T: Transport>(
    c: &Client<T>,
    list: ListId,
) -> Controller<FakeProvider, TokioClock, OsEntropy> {
    let script = ["First approved task", "Second approved task"]
        .into_iter()
        .enumerate()
        .map(|(i, title)| {
            Ok(FakeRound::new([
                Ok(StreamEvent::Tool(
                    ToolCall::parse(
                        i.to_string(),
                        "create_task".into(),
                        &json!({"title":title,"list_id":list}).to_string(),
                    )
                    .unwrap(),
                )),
                Ok(StreamEvent::Finished),
            ]))
        })
        .chain([Ok(FakeRound::new([Ok(StreamEvent::Finished)]))]);
    let chat = Controller::new(TokioClock, OsEntropy);
    chat.configure(Some(Selection {
        provider: Arc::new(FakeProvider::new(CAPABILITIES, script)),
        model: MODEL.into(),
        allow_billed: false,
        billed_list_scope: None,
        access: BTreeMap::from([(list, ListAccess::AllModels)]).into(),
    }));
    let snapshot = c.assistant_snapshot().unwrap();
    assert!(Arc::ptr_eq(
        &snapshot.state,
        &c.assistant_snapshot().unwrap().state
    ));
    chat.update_snapshot(Some(snapshot));
    chat.start(SendIntent {
        generation: chat.view().generation,
        sequence: 1,
        prompt: "Plan two tasks".into(),
        context: EvalContext::utc(Timestamp::now()),
    })
    .unwrap()
    .await
    .unwrap();
    chat
}

#[derive(Clone)]
struct ApprovalTransport {
    http: HttpTransport,
    sends: Arc<Mutex<Vec<Vec<u8>>>>,
    lose: Arc<AtomicBool>,
}

struct ApprovalStream<S> {
    inner: S,
    sends: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl<S: EventStream> EventStream for ApprovalStream<S> {
    async fn next_frame(&mut self) -> Option<Result<SseFrame, TransportError>> {
        let frame = self.inner.next_frame().await;
        if matches!(&frame, Some(Ok(SseFrame::Event { data, event, .. }))
            if event == "event" && data.contains(r#""agent_id":"assistant""#))
        {
            // Hold the originating replica's SSE echo long enough to exercise the POST replay.
            eventually("approval POST replayed", WAIT, || {
                self.sends.lock().unwrap().len() >= 2
            })
            .await;
        }
        frame
    }
}

impl Transport for ApprovalTransport {
    type Stream = ApprovalStream<<HttpTransport as Transport>::Stream>;

    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        if req.path != APPROVALS_PATH {
            return self.http.send(req).await;
        }
        self.sends.lock().unwrap().push(req.body.clone().unwrap());
        let result = self.http.send(req).await?;
        if self.lose.swap(false, Ordering::SeqCst) {
            assert_eq!(result.status, 200);
            return Err(TransportError::Network(
                "response lost after commit (test)".into(),
            ));
        }
        Ok(result)
    }

    async fn open_stream(&self, req: Request) -> Result<StreamOpen<Self::Stream>, TransportError> {
        Ok(match self.http.open_stream(req).await? {
            StreamOpen::Open(inner) => StreamOpen::Open(ApprovalStream {
                inner,
                sends: self.sends.clone(),
            }),
            StreamOpen::Refused(response) => StreamOpen::Refused(response),
        })
    }
}

#[tokio::test]
async fn approval_waits_for_durability_recovers_after_close_and_converges_over_sse() {
    let s = TestServer::start().await;
    let list = ListId::from_uuid(Uuid::new_v4());
    s.running
        .engine
        .execute(
            &alpha(),
            Uuid::new_v4(),
            &command(json!({"cmd":"create_list","id":list,"name":"Inbox"})),
        )
        .unwrap();
    let mut storage = MirrorStorage::new();
    let transport = ApprovalTransport {
        http: s.transport(TOKEN_A),
        sends: Arc::default(),
        lose: Arc::new(AtomicBool::new(true)),
    };
    let a = client(transport.clone(), storage.clone(), alpha());
    let b = client(s.transport(TOKEN_B), MemoryStorage::new(), beta());
    let run_a = spawn(&a);
    let run_b = spawn(&b);
    eventually("both owners caught up", WAIT, || {
        a.status().caught_up && b.status().caught_up
    })
    .await;
    let head = s.head();
    let chat = review(&a, list).await;
    let draft = chat.pending().unwrap();
    let preview = draft.preview();
    assert!(storage.load().unwrap().outbox.is_empty());
    assert!(transport.sends.lock().unwrap().is_empty());
    let mut wrong = preview.digest;
    wrong[0] ^= 1;
    assert!(matches!(
        chat.approve(&a, preview.id, wrong),
        Err(ReviewError::NotPending)
    ));
    let approval = chat.approve(&a, preview.id, preview.digest).unwrap();
    assert!(matches!(
        chat.approve(&a, preview.id, preview.digest),
        Err(ReviewError::NotPending)
    ));
    assert_eq!(a.approval_outcome(&approval), ApprovalOutcome::Saving);
    assert!(
        a.view().tasks.is_empty(),
        "the pending operation must not overlay tasks"
    );
    let saved = storage.load().unwrap().outbox[0].clone();
    assert_eq!(saved.command_id, preview.id);
    assert!(matches!(saved.op, Operation::AssistantApproval { .. }));
    assert!(transport.sends.lock().unwrap().is_empty());
    assert_eq!(s.head(), head);

    // Drive the same transaction journal an IndexedDB flusher writes, but close the submitting
    // replica first so it cannot send. Only the approved payload may be present in the backend.
    stop(&a, run_a).await;
    let mut disk = Persisted::default();
    for batch in storage.take_batches() {
        for change in &batch {
            disk.apply(change);
        }
    }
    let persisted = disk.outbox.get(&preview.id.to_string()).unwrap();
    for excluded in [
        "Plan two tasks",
        "previous_draft",
        "instructions",
        "preview",
    ] {
        assert!(!persisted.contains(excluded));
    }
    let (mut reopened, recovery) = MirrorStorage::recover(disk);
    assert_eq!(recovery, todo_client::mirror::Recovery::default());
    assert_eq!(reopened.load().unwrap().outbox, vec![saved]);

    let a = client(transport.clone(), reopened, alpha());
    let run_a = spawn(&a);
    // Only the original ID is sent, and both SSE clients settle the same two-event command.
    eventually("both replicas converge", WAIT, || {
        s.head() == head + 2 && converged(&a, &s) && converged(&b, &s)
    })
    .await;
    {
        let sends = transport.sends.lock().unwrap();
        assert!(sends.len() >= 2);
        for bytes in sends.iter() {
            let payload: Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(payload["command_id"], json!(preview.id));
            assert_eq!(payload["batch"]["digest"], json!(preview.digest));
            assert_eq!(bytes, &sends[0]);
        }
    }
    assert_eq!(
        s.stored_command_ids()
            .iter()
            .filter(|(_, kind)| kind == "assistant_approval")
            .count(),
        1
    );
    let tasks: Vec<TaskId> = s.state().tasks.keys().copied().collect();
    let events = s.running.engine.events_since(head, 20).unwrap();
    assert_eq!(actor_name(&events[0].actor), "In-app assistant");
    assert_eq!(events[0].group, events[1].group);
    let query = ActivityQuery {
        agent: Some(todo_core::AgentId::new("assistant").unwrap()),
        ..ActivityQuery::default()
    };
    assert_eq!(b.activity(&query).await.unwrap().items.len(), 2);
    let seq = events[1].seq.unwrap();
    b.undo(seq, events[1].id, false).unwrap();
    eventually("both replicas converge after group undo", WAIT, || {
        s.head() == head + 4 && converged(&a, &s) && converged(&b, &s)
    })
    .await;
    assert!(tasks.iter().all(|id| !s.state().tasks.contains_key(id)));
    stop(&a, run_a).await;
    stop(&b, run_b).await;
    s.stop().await;
}

#[tokio::test]
async fn live_receipt_supports_exact_group_undo_and_never_rebases_locally() {
    let s = TestServer::start().await;
    let list = ListId::from_uuid(Uuid::new_v4());
    s.running
        .engine
        .execute(
            &alpha(),
            Uuid::new_v4(),
            &command(json!({"cmd":"create_list","id":list,"name":"Inbox"})),
        )
        .unwrap();
    let a = client(s.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let run = spawn(&a);
    eventually("owner caught up", WAIT, || a.status().caught_up).await;
    let stale_chat = review(&a, list).await;
    let stale = stale_chat.pending().unwrap();
    a.submit(command(
        json!({"cmd":"create_list","id":Uuid::new_v4(),"name":"Other"}),
    ))
    .unwrap();
    eventually("new head", WAIT, || s.head() == 2 && converged(&a, &s)).await;
    let preview = stale.preview();
    assert!(matches!(
        stale_chat.approve(&a, preview.id, preview.digest),
        Err(ReviewError::Stale)
    ));
    assert!(a.pending().is_empty());
    let chat = review(&a, list).await;
    let draft = chat.pending().unwrap();
    let preview = draft.preview();
    a.set_sending(false);
    let receipt = chat.approve(&a, preview.id, preview.digest).unwrap();
    assert_eq!(a.approval_outcome(&receipt), ApprovalOutcome::Pending);
    assert!(a.view().tasks.is_empty());
    a.set_sending(true);
    eventually(
        "receipt confirmed by the whole batch",
        WAIT,
        || matches!(a.approval_outcome(&receipt), ApprovalOutcome::Confirmed { epoch, first_event } if epoch == s.epoch() && first_event.seq == 3),
    ).await;
    assert!(matches!(
        a.compensate_tracked(receipt.undo_target()).unwrap(),
        TrackedUndo::Queued(_)
    ));
    eventually("undo confirmed", WAIT, || {
        s.head() == 6 && converged(&a, &s)
    })
    .await;
    assert!(a.view().tasks.is_empty());
    stop(&a, run).await;
    s.stop().await;
}
