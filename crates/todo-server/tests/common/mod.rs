//! Shared helpers: an in-process server on an ephemeral port with a temporary data directory, a
//! JSON client, and a minimal SSE reader.
#![allow(dead_code, clippy::unwrap_used)]

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use todo_core::DeviceId;
use todo_server::auth::hash_token;
use todo_server::config::OwnerToken;
use todo_server::{Config, Options, RunningServer, Server};
use uuid::Uuid;

pub const TOKEN_A: &str = "test-token-alpha-0123456789";
pub const TOKEN_B: &str = "test-token-beta-9876543210";

pub fn id() -> String {
    Uuid::new_v4().to_string()
}

pub struct TestServer {
    pub server: RunningServer,
    pub dir: TempDir,
    pub config: Config,
    pub client: reqwest::Client,
    pub url: String,
}

fn config_for(dir: &TempDir) -> Config {
    let mut cfg = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().to_owned(),
        ..Config::default()
    };
    cfg.auth.owner_tokens = vec![
        OwnerToken {
            device: DeviceId::new("alpha").unwrap(),
            sha256: hash_token(TOKEN_A),
        },
        OwnerToken {
            device: DeviceId::new("beta").unwrap(),
            sha256: hash_token(TOKEN_B),
        },
    ];
    cfg
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    pub async fn start_with(f: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config_for(&dir);
        f(&mut config);
        Self::boot(dir, config).await
    }

    /// Starts a server on an existing data directory (see [`stop`](Self::stop)).
    pub async fn boot(dir: TempDir, config: Config) -> Self {
        let server = Server::bind(&config, Options::default())
            .await
            .unwrap()
            .spawn()
            .unwrap();
        let url = server.url();
        Self {
            server,
            dir,
            config,
            client: reqwest::Client::new(),
            url,
        }
    }

    /// Stops the server and starts a new one on the same data directory.
    pub async fn restart(self) -> Self {
        let Self {
            server,
            dir,
            config,
            ..
        } = self;
        server.shutdown().await.unwrap();
        Self::boot(dir, config).await
    }

    pub async fn stop(self) -> (TempDir, Config) {
        self.server.shutdown().await.unwrap();
        (self.dir, self.config)
    }

    pub fn db_path(&self) -> PathBuf {
        self.config.database_path()
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut req = self.client.request(method, format!("{}{path}", self.url));
        if let Some(t) = token {
            req = req.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let text = res.text().await.unwrap();
        let value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        (status, headers, value)
    }

    pub async fn get(&self, path: &str) -> (StatusCode, Value) {
        let (s, _, v) = self.request(Method::GET, path, Some(TOKEN_A), None).await;
        (s, v)
    }

    pub async fn get_ok(&self, path: &str) -> Value {
        let (s, v) = self.get(path).await;
        assert_eq!(s, StatusCode::OK, "GET {path}: {v}");
        v
    }

    /// Posts a command with an explicit command ID.
    pub async fn command_with_id(
        &self,
        token: &str,
        command_id: &str,
        command: Value,
    ) -> (StatusCode, HeaderMap, Value) {
        self.request(
            Method::POST,
            "/api/v1/commands",
            Some(token),
            Some(json!({ "command_id": command_id, "command": command })),
        )
        .await
    }

    /// Posts a command with a fresh ID.
    pub async fn command(&self, command: Value) -> (StatusCode, Value) {
        let (s, _, v) = self.command_with_id(TOKEN_A, &id(), command).await;
        (s, v)
    }

    /// Posts a command that must succeed; returns the result.
    pub async fn ok(&self, command: Value) -> Value {
        let (s, v) = self.command(command.clone()).await;
        assert_eq!(s, StatusCode::OK, "{command}: {v}");
        v
    }

    /// Undoes `seq`, adding a fresh `command_id` to `body` unless it has one.
    pub async fn undo(&self, seq: u64, body: Option<Value>) -> (StatusCode, Value) {
        let mut body = body.unwrap_or_else(|| json!({}));
        if let Value::Object(m) = &mut body {
            m.entry("command_id").or_insert_with(|| json!(id()));
            if !m.contains_key("expected_event_id") {
                m.insert("expected_event_id".into(), json!(self.event_id_at(seq)));
            }
        }
        self.undo_raw(seq, Some(body)).await
    }

    /// The ID of the event at `seq`, or a random one if there is none.
    pub fn event_id_at(&self, seq: u64) -> Value {
        match self.server.engine.events_since(seq.saturating_sub(1), 1) {
            Ok(events) if events.first().and_then(|e| e.seq) == Some(seq) => json!(events[0].id),
            _ => json!(id()),
        }
    }

    /// Undoes `seq` with exactly `body`.
    pub async fn undo_raw(&self, seq: u64, body: Option<Value>) -> (StatusCode, Value) {
        let (s, _, v) = self
            .request(
                Method::POST,
                &format!("/api/v1/events/{seq}/undo"),
                Some(TOKEN_A),
                body,
            )
            .await;
        (s, v)
    }

    pub async fn create_list(&self, name: &str) -> String {
        let id = id();
        self.ok(json!({ "cmd": "create_list", "id": id, "name": name }))
            .await;
        id
    }

    pub async fn create_task(&self, list: &str, title: &str, extra: Value) -> String {
        let id = id();
        let mut cmd = json!({ "cmd": "create_task", "id": id, "list": list, "title": title });
        if let (Some(m), Value::Object(e)) = (cmd.as_object_mut(), extra) {
            m.extend(e);
        }
        self.ok(cmd).await;
        id
    }

    /// `GET /api/v1/tasks?query=...`.
    pub async fn query_tasks(&self, query: &str) -> (StatusCode, Value) {
        let res = self
            .client
            .get(format!("{}/api/v1/tasks", self.url))
            .header(AUTHORIZATION, format!("Bearer {TOKEN_A}"))
            .query(&[("query", query)])
            .send()
            .await
            .unwrap();
        (res.status(), res.json::<Value>().await.unwrap())
    }

    pub async fn sse(&self, token: &str, since: Option<u64>) -> SseReader {
        let mut req = self
            .client
            .get(format!("{}/api/v1/events", self.url))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(ACCEPT, "text/event-stream");
        if let Some(s) = since {
            req = req.query(&[("since", s)]);
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        SseReader::new(res)
    }
}

/// The seq of the first event in a command result.
pub fn first_seq(result: &Value) -> u64 {
    result["events"][0]["seq"].as_u64().unwrap()
}

// ---- SSE --------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frame {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
    pub comments: Vec<String>,
}

impl Frame {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.data).unwrap()
    }

    pub fn seq(&self) -> u64 {
        self.id.as_deref().unwrap().parse().unwrap()
    }
}

pub struct SseReader {
    res: reqwest::Response,
    buf: String,
    /// The `log-epoch` response header.
    pub epoch_header: Option<String>,
    /// The `hello` frame, once read (every stream starts with one; the readers skip it).
    pub hello: Option<Frame>,
}

impl SseReader {
    pub fn new(res: reqwest::Response) -> Self {
        Self {
            epoch_header: res
                .headers()
                .get("log-epoch")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            res,
            buf: String::new(),
            hello: None,
        }
    }

    fn parse(block: &str) -> Frame {
        let mut f = Frame::default();
        let mut data = Vec::new();
        for line in block.lines() {
            if let Some(c) = line.strip_prefix(':') {
                f.comments.push(c.trim().to_owned());
            } else if let Some((k, v)) = line.split_once(':') {
                let v = v.strip_prefix(' ').unwrap_or(v);
                match k {
                    "id" => f.id = Some(v.to_owned()),
                    "event" => f.event = Some(v.to_owned()),
                    "data" => data.push(v.to_owned()),
                    _ => {}
                }
            }
        }
        f.data = data.join("\n");
        f
    }

    /// Next frame (event or comment) after the `hello` frame, or `None` on timeout or end of stream.
    pub async fn next_frame(&mut self, within: Duration) -> Option<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let normalized = self.buf.replace("\r\n", "\n");
            if let Some(pos) = normalized.find("\n\n") {
                let block = normalized[..pos].to_owned();
                self.buf = normalized[pos + 2..].to_owned();
                let frame = Self::parse(&block);
                if self.hello.is_none() {
                    assert_eq!(frame.event.as_deref(), Some("hello"), "the first frame");
                    self.hello = Some(frame);
                    continue;
                }
                return Some(frame);
            }
            let chunk = tokio::time::timeout_at(deadline, self.res.chunk()).await;
            match chunk {
                Ok(Ok(Some(bytes))) => self.buf.push_str(&String::from_utf8_lossy(&bytes)),
                _ => return None,
            }
        }
    }

    /// Next frame that carries an event or a named notice (skipping heartbeats).
    pub async fn next_event_or_named(&mut self, within: Duration) -> Option<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let f = self.next_frame(left).await?;
            if f.id.is_some() || f.event.is_some() {
                return Some(f);
            }
        }
    }

    /// Next frame that carries an event (skipping heartbeats).
    pub async fn next_event(&mut self, within: Duration) -> Option<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let f = self.next_frame(left).await?;
            if f.id.is_some() {
                return Some(f);
            }
        }
    }
}
