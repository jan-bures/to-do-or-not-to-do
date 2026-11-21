//! A network-free stand-in for the server: `todo-core` plus an event log with sequence numbers and
//! idempotency by command ID, behind a mock [`Transport`] that speaks the real HTTP API.
#![allow(dead_code, clippy::unwrap_used)]

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use todo_client::notify::Notify;
use todo_client::platform::{
    Clock, Entropy, EventStream, Request, Response, SseFrame, StreamOpen, Transport, TransportError,
};
use todo_client::protocol::{CommandResult, EventRef, LogEvent};
use todo_core::command::{Command, NewTask};
use todo_core::error::DomainError;
use todo_core::ids::{ListId, TaskId};
use todo_core::time::Timestamp;
use todo_core::{Actor, State, apply, decide};
use uuid::Uuid;

pub fn owner() -> Actor {
    Actor::owner("pc").unwrap()
}

pub fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(1_800_000_000_000 + ms).unwrap()
}

pub fn list_id(n: u64) -> ListId {
    ListId::new_v7(1_700_000_000_000 + n, [n as u8; 10])
}

pub fn task_id(n: u64) -> TaskId {
    TaskId::new_v7(1_700_000_000_000 + n, [n as u8 ^ 0x55; 10])
}

pub fn create_list(id: ListId, name: &str) -> Command {
    Command::CreateList {
        id,
        collection: None,
        name: name.into(),
        color: None,
        placement: Default::default(),
    }
}

pub fn create_task(id: TaskId, list: ListId, title: &str) -> Command {
    Command::CreateTask(Box::new(NewTask::new(id, list, title)))
}

pub fn add_note(task: TaskId, text: &str) -> Command {
    Command::AddNote {
        task,
        text: text.into(),
    }
}

/// The log epoch of a [`FakeServer`] (until [`FakeServer::restore`]).
pub const EPOCH: Uuid = Uuid::from_u128(0xe0);

/// The authoritative side: state, log, idempotency records and a clock of its own.
pub struct FakeServer {
    pub state: State,
    pub log: Vec<LogEvent>,
    pub epoch: Uuid,
    results: HashMap<Uuid, (String, CommandResult)>,
    clock_ms: i64,
}

impl Default for FakeServer {
    fn default() -> Self {
        Self {
            state: State::new(),
            log: Vec::new(),
            epoch: EPOCH,
            results: HashMap::new(),
            clock_ms: 0,
        }
    }
}

impl FakeServer {
    /// Decides and stores a command (or replays the stored result for a known command ID).
    pub fn execute(
        &mut self,
        actor: &Actor,
        command_id: Uuid,
        command: &Command,
    ) -> Result<CommandResult, (u16, &'static str)> {
        let hash = format!("{actor:?}{command:?}");
        if let Some((h, r)) = self.results.get(&command_id) {
            return if *h == hash {
                Ok(r.clone())
            } else {
                Err((409, "command_id_reused"))
            };
        }
        // Server time runs on its own, far from the clients' submission times.
        self.clock_ms += 1000;
        let now = ts(10_000_000 + self.clock_ms);
        let events = decide(&self.state, actor, command, now).map_err(|e| match e {
            DomainError::NotFound(_) => (404, "not_found"),
            DomainError::Invalid { .. } => (422, "invalid"),
            _ => (409, "conflict"),
        })?;
        let mut refs = Vec::new();
        for (i, mut e) in events.into_iter().enumerate() {
            let seq = self.log.len() as u64 + 1;
            e.seq = Some(seq);
            apply(&mut self.state, &e).unwrap();
            refs.push(EventRef { seq, id: e.id });
            self.log.push(LogEvent {
                event: e,
                command_id,
                command_idx: i as u32,
            });
        }
        let result = CommandResult {
            command_id,
            events: refs,
            summary: String::new(),
            epoch: Some(self.epoch),
        };
        self.results.insert(command_id, (hash, result.clone()));
        Ok(result)
    }

    pub fn head(&self) -> u64 {
        self.log.len() as u64
    }

    /// Restores a "backup" of the first `len` events under a new epoch (forgetting the idempotency
    /// records of the lost commands).
    pub fn restore(&mut self, len: usize, epoch: Uuid) {
        let kept: Vec<LogEvent> = self.log[..len].to_vec();
        let lost: Vec<Uuid> = self.log[len..].iter().map(|e| e.command_id).collect();
        self.results.retain(|id, _| !lost.contains(id));
        self.state = State::new();
        for e in &kept {
            apply(&mut self.state, &e.event).unwrap();
        }
        self.log = kept;
        self.epoch = epoch;
    }

    /// The plain events, for applying to a state.
    pub fn events(&self) -> Vec<todo_core::event::Event> {
        self.log.iter().map(|e| e.event.clone()).collect()
    }

    /// How many distinct command IDs were stored.
    pub fn stored_commands(&self) -> usize {
        self.results.len()
    }
}

fn json_response(status: u16, body: &Value) -> Response {
    Response {
        status,
        retry_after: None,
        body: serde_json::to_vec(body).unwrap(),
    }
}

fn error(status: u16, code: &str, details: Value) -> Response {
    json_response(
        status,
        &json!({ "error": { "code": code, "message": code, "details": details } }),
    )
}

fn query<T: std::str::FromStr>(path: &str, key: &str) -> Option<T> {
    let q = path.split_once('?')?.1;
    q.split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
}

fn epoch_mismatch(path: &str, server: &FakeServer) -> Option<Response> {
    let epoch: Uuid = query(path, "epoch")?;
    (epoch != server.epoch).then(|| error(409, "epoch_mismatch", json!({ "epoch": server.epoch })))
}

/// Scripted faults for the mock transport.
#[derive(Default)]
pub struct Script {
    /// Answer this many command requests with 503 `outcome_unknown` *after* executing them.
    pub outcome_unknown: usize,
    /// Fail this many requests (of any kind) with a network error before they reach the server.
    pub offline: usize,
}

/// A [`Transport`] over a shared [`FakeServer`].
#[derive(Clone)]
pub struct MockTransport {
    pub server: Arc<Mutex<FakeServer>>,
    pub actor: Actor,
    pub script: Arc<Mutex<Script>>,
    /// Bumped when the log grows, to wake open streams.
    pub grew: Notify,
    pub requests: Arc<AtomicUsize>,
}

impl MockTransport {
    pub fn new(server: Arc<Mutex<FakeServer>>, actor: Actor) -> Self {
        Self {
            server,
            actor,
            script: Arc::default(),
            grew: Notify::default(),
            requests: Arc::default(),
        }
    }

    fn handle(&self, req: &Request) -> Response {
        let mut server = self.server.lock().unwrap();
        if req.path == "/api/v1/commands" {
            let body: Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
            let id: Uuid = serde_json::from_value(body["command_id"].clone()).unwrap();
            let command: Command = serde_json::from_value(body["command"].clone()).unwrap();
            let result = server.execute(&self.actor, id, &command);
            drop(server);
            self.grew.notify();
            return match result {
                Ok(r) => {
                    let mut script = self.script.lock().unwrap();
                    if script.outcome_unknown > 0 {
                        script.outcome_unknown -= 1;
                        return error(503, "outcome_unknown", Value::Null);
                    }
                    json_response(200, &serde_json::to_value(r).unwrap())
                }
                Err((status, code)) => error(status, code, json!({ "entity": null })),
            };
        }
        if req.path.starts_with("/api/v1/events?") {
            if let Some(r) = epoch_mismatch(&req.path, &server) {
                return r;
            }
            let since: u64 = query(&req.path, "since").unwrap_or(0);
            let limit: usize = query(&req.path, "limit").unwrap_or(500);
            if since > server.head() {
                return error(
                    400,
                    "invalid_since",
                    json!({ "since": since, "head_seq": server.head() }),
                );
            }
            let events: Vec<&LogEvent> =
                server.log.iter().skip(since as usize).take(limit).collect();
            let next_since = events.last().and_then(|e| e.event.seq).unwrap_or(since);
            return json_response(
                200,
                &json!({
                    "events": events,
                    "next_since": next_since,
                    "head_seq": server.head(),
                    "has_more": next_since < server.head(),
                    "epoch": server.epoch,
                }),
            );
        }
        error(404, "not_found", Value::Null)
    }

    fn offline(&self) -> bool {
        let mut s = self.script.lock().unwrap();
        if s.offline > 0 {
            s.offline -= 1;
            true
        } else {
            false
        }
    }
}

/// Streams the fake log from a position, waiting for it to grow.
pub struct MockStream {
    server: Arc<Mutex<FakeServer>>,
    grew: Notify,
    next: u64,
    hello: Option<String>,
}

impl EventStream for MockStream {
    async fn next_frame(&mut self) -> Option<Result<SseFrame, TransportError>> {
        if let Some(data) = self.hello.take() {
            return Some(Ok(SseFrame::Event {
                event: "hello".into(),
                id: None,
                data,
            }));
        }
        loop {
            let seen = self.grew.generation();
            let event = self
                .server
                .lock()
                .unwrap()
                .log
                .get(self.next as usize)
                .cloned();
            if let Some(e) = event {
                self.next += 1;
                return Some(Ok(SseFrame::Event {
                    event: "event".into(),
                    id: Some(self.next.to_string()),
                    data: serde_json::to_string(&e).unwrap(),
                }));
            }
            self.grew.changed(seen).await;
        }
    }
}

impl Transport for MockTransport {
    type Stream = MockStream;

    async fn send(&self, request: Request) -> Result<Response, TransportError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        yield_now().await;
        if self.offline() {
            return Err(TransportError::Network("offline".into()));
        }
        Ok(self.handle(&request))
    }

    async fn open_stream(
        &self,
        request: Request,
    ) -> Result<StreamOpen<MockStream>, TransportError> {
        yield_now().await;
        if self.offline() {
            return Err(TransportError::Network("offline".into()));
        }
        let server = self.server.lock().unwrap();
        if let Some(r) = epoch_mismatch(&request.path, &server) {
            return Ok(StreamOpen::Refused(r));
        }
        let since: u64 = query(&request.path, "since").unwrap_or(0);
        let head = server.head();
        let hello = json!({ "epoch": server.epoch, "head_seq": head }).to_string();
        drop(server);
        if since > head {
            return Ok(StreamOpen::Refused(error(
                400,
                "invalid_since",
                json!({ "since": since, "head_seq": head }),
            )));
        }
        Ok(StreamOpen::Open(MockStream {
            server: self.server.clone(),
            grew: self.grew.clone(),
            next: since,
            hello: Some(hello),
        }))
    }
}

/// Yields to the executor once.
pub async fn yield_now() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await;
}

/// A clock that advances one millisecond per reading. Short sleeps (retry delays) only yield, so retry
/// loops run at full speed on a single-threaded executor; long ones (request and idle timeouts) never
/// end, because nothing in the fake ever hangs.
#[derive(Clone, Default)]
pub struct FastClock(Arc<AtomicI64>);

impl Clock for FastClock {
    fn now(&self) -> Timestamp {
        ts(self.0.fetch_add(1, Ordering::SeqCst))
    }

    async fn sleep(&self, duration: Duration) {
        if duration > Duration::from_secs(5) {
            std::future::pending::<()>().await;
        }
        yield_now().await;
    }
}

/// Deterministic "random" bytes; give each client its own seed.
#[derive(Default)]
pub struct CountingEntropy(AtomicI64);

impl CountingEntropy {
    pub fn seeded(seed: i64) -> Self {
        Self(AtomicI64::new(seed << 32))
    }
}

impl Entropy for CountingEntropy {
    fn fill(&self, buf: &mut [u8]) {
        let n = self.0.fetch_add(1, Ordering::SeqCst).to_le_bytes();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = n[i % 8] ^ (i as u8).wrapping_mul(31);
        }
    }
}
