//! Email ingest helpers: a server with ingest on, signed requests, payloads, and the owner's
//! settings (the Inbox and list keys) set straight in the store.

use super::TestServer;
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::Sha256;
use std::io::Write as _;
use todo_core::time::Timestamp;
use todo_server::Config;
use todo_server::ingest::store::By;

pub const SECRET: &str = "test-only-ingest-secret-0123456789abcdef";
pub const OWNER: &str = "you@example.com";
pub const PATH: &str = "/api/v1/ingest/email";

/// Writes `secret` to an owner-only file in `dir` and points the config at it.
pub fn secret_file(cfg: &mut Config, secret: &str) {
    let path = cfg.data_dir.join("ingest-hmac.secret");
    let mut f = todo_fs::create_private_file(&path).unwrap();
    // A trailing newline, as `openssl rand -hex 32 > file` writes.
    writeln!(f, "{secret}").unwrap();
    cfg.ingest.email_hmac_secret_file = Some(path);
}

pub fn enabled(cfg: &mut Config) {
    secret_file(cfg, SECRET);
    cfg.ingest.allowed_senders = vec![OWNER.into(), "Second@Example.NET".into()];
}

pub async fn server() -> TestServer {
    TestServer::start_with(enabled).await
}

pub fn sign(secret: &str, ts: &str, nonce: &str, body: &[u8]) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    m.update(format!("{ts}\n{nonce}\n").as_bytes());
    m.update(body);
    hex::encode(m.finalize().into_bytes())
}

pub fn nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn now_secs() -> i64 {
    Timestamp::now().as_second()
}

pub fn payload(mid: &str) -> Value {
    json!({
        "version": 1,
        "message_id": mid,
        "list": null,
        "title": "Quarterly report",
        "subject": "Fwd: Quarterly report",
        "keep": false,
        "original_sender": { "address": "alice@example.org", "name": "Alice Example", "source": "forwarded_header" },
        "forwarded_by": OWNER,
        "received_at": "2025-11-21T08:00:00.000Z",
        "body": null,
        "raw_truncated": false,
        "parse_failed": false
    })
}

/// A raw request: exactly these headers and body.
pub async fn post_raw(
    t: &TestServer,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let mut req = t
        .client
        .post(format!("{}{PATH}", t.url))
        .header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = req.body(body).send().await.unwrap();
    let status = res.status();
    let text = res.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// A correctly signed request at `ts` with `nonce`.
pub async fn post_signed(t: &TestServer, ts: i64, nonce: &str, body: &[u8]) -> (StatusCode, Value) {
    let ts = ts.to_string();
    let sig = sign(SECRET, &ts, nonce, body);
    post_raw(
        t,
        &[
            ("x-todo-timestamp", &ts),
            ("x-todo-nonce", nonce),
            ("x-todo-signature", &sig),
        ],
        body.to_vec(),
    )
    .await
}

/// A correctly signed request, now, with a fresh nonce.
pub async fn send(t: &TestServer, p: &Value) -> (StatusCode, Value) {
    post_signed(t, now_secs(), &nonce(), &serde_json::to_vec(p).unwrap()).await
}

pub fn live_tasks(t: &TestServer) -> Vec<todo_core::model::Task> {
    let s = t.server.engine.state();
    s.tasks
        .values()
        .filter(|x| s.is_live(x.id.into()))
        .cloned()
        .collect()
}

pub fn task_id(v: &Value) -> String {
    v["task_id"].as_str().unwrap().to_owned()
}

pub const BY: By = By {
    at: 0,
    ip: None,
    actor: None,
};

/// Picks `list` as the Inbox, as the owner does on the admin page (tested in `tests/auth.rs`).
pub fn set_inbox(t: &TestServer, list: &str) {
    t.server
        .ingest
        .store
        .set_inbox(Some(list.parse().unwrap()), BY)
        .unwrap();
}

/// Makes `todo+<key>@` name `list`, as the owner does on the admin page.
pub fn add_key(t: &TestServer, key: &str, list: &str) {
    t.server
        .ingest
        .store
        .add_list_key(key, list.parse().unwrap(), BY)
        .unwrap()
        .unwrap();
}

/// A list picked as the Inbox.
pub async fn make_inbox(t: &TestServer) -> String {
    let l = t.create_list("Inbox").await;
    set_inbox(t, &l);
    l
}

/// The rows of the Message-ID ledger.
pub fn ledger(db: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT mid_hash, task_id FROM ingest_messages ORDER BY mid_hash")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}
