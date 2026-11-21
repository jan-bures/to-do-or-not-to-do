//! Email to task: `POST /api/v1/ingest/email`, called by the Email Worker (`workers/email-ingest`).
//! The contract is `docs/ingest-email.md`; the shared signing test vector is
//! `docs/ingest-email-signature.json`.
//!
//! The route takes no bearer token. It is authenticated by an HMAC-SHA256 signature with a secret
//! shared only with the Worker, and it answers 404 unless that secret is configured
//! (`ingest.email_hmac_secret_file`). A request goes through these checks, in this order:
//!
//! 1. **Admission.** The per-IP ingest rate limit, and the same "too many failed authentications"
//!    budget the bearer-token routes use (both on the client IP, which honours
//!    `trust_cloudflare_headers`): 429.
//! 2. **Body size.** A `Content-Length` over [`MAX_BODY_BYTES`] is refused before the body is read,
//!    and reading stops at that size: 413.
//! 3. **Headers.** `X-Todo-Timestamp`, `X-Todo-Nonce` and `X-Todo-Signature` must each be present
//!    exactly once and well formed: 401.
//! 4. **Timestamp** within [`WINDOW_SECS`] of the server clock: 401.
//! 5. **Signature** over `timestamp \n nonce \n body`, compared in constant time, before the body is
//!    parsed and before anything touches the database: 401.
//! 6. **Nonce**, reserved with one `INSERT` under a primary key (a conflict is a replay): 401.
//! 7. **Schema**: 422.
//! 8. **Sender**: `forwarded_by` must be on the owner's allowlist (`/admin/ingest`): 403.
//! 9. **Dedupe and create**, in one transaction ([`Engine::execute_unique`]): the dedupe key (the
//!    SHA-256 of the `Message-ID` as the email link stores it, [`crate::db::message_key`]) is inserted
//!    under a unique key first, which takes the database's write lock; the sender is checked against
//!    the allowlist again, so a sender removed while the request waited for the writer is refused
//!    (403); then the task and its email link are stored as one undoable group with actor
//!    `email-ingest`. 201 `created`, or 200 `duplicate` with the task stored the first time.
//!
//! The Message-ID ledger is derived from the event log (the email links created by `email-ingest`),
//! so it is checked and, if needed, rebuilt with the projections.
//!
//! Every failure that may go away (rate limits, a busy or closed engine, no Inbox list yet) is 429 or
//! 5xx, so the Worker queues and retries; the other 4xx are permanent. Error bodies never echo the
//! request.
//!
//! **Lists.** Routing is explicit and never depends on list names, so renaming, deleting or adding a
//! list can't silently redirect mail. `list` (from `todo+<list>@`) must be a key the owner set on the
//! admin page, which names one list by ID. Without `list`, with the reserved key `inbox`, with a key
//! nobody set or with one whose list was deleted since, the task goes to the Inbox: the list the owner
//! picked on the same page. The last two add a line to the notes. If no Inbox is set, or it was
//! deleted, the request fails with 503 (the Worker retries) and the admin page says why.
//!
//! Agents see an email task like any other: one that can read its list sees the subject, the sender
//! and any kept body. The admin page says so next to the Inbox picker. Share pages never show notes
//! or links.
//!
//! **Privacy.** Subjects, bodies, addresses and the secret are never logged above `debug`: log lines
//! carry the outcome, a few booleans and `mid`, the first 12 hex digits of the SHA-256 of the
//! `Message-ID` (the same value the Worker logs).
//!
//! Attachments are out of scope: the Worker never sends them. Storing those of `#keep` mails (in R2,
//! through a separate upload endpoint) is a later step.
//!
//! [`Engine::execute_unique`]: crate::engine::Engine::execute_unique

pub mod store;

use crate::api::error::{ApiError, ErrorBody};
use crate::api::{RequestCancel, Work, blocking};
use crate::app::AppState;
use crate::auth::limits::KeyedLimiter;
use crate::auth::session::ClientIp;
use crate::config::{Config, ConfigError, IngestConfig};
use crate::db::StoreError;
use crate::engine::{EngineError, UniqueOutcome, UniquePlan};
use axum::Json;
use axum::extract::{Request, State};
use axum::http::header::CONTENT_LENGTH;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, Mac};
use jiff::tz::TimeZone;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::path::Path;
use store::IngestStore;
use todo_core::command::{Command, NewTask};
use todo_core::decide::{MAX_NOTES, MAX_URL};
use todo_core::ids::{LinkId, ListId, TaskId};
use todo_core::model::{EmailLink, LinkKind};
use todo_core::time::Timestamp;
use todo_core::{Actor, State as CoreState};

/// Largest accepted request body (the Worker never sends more than 100,000 bytes).
pub const MAX_BODY_BYTES: usize = 128 * 1024;
/// How far a request's timestamp may be from the server clock, either way.
pub const WINDOW_SECS: i64 = 300;
/// Shortest accepted HMAC secret, in bytes.
pub const MIN_SECRET_LEN: usize = 32;
/// Largest accepted secret file.
const MAX_SECRET_FILE: u64 = 4096;

pub use store::INBOX_KEY;

/// Longest `Message-ID`, original subject (characters), title (characters), display name
/// (characters), address (characters) and kept body (bytes) the contract allows.
const MAX_MESSAGE_ID: usize = 998;
const MAX_SUBJECT: usize = 998;
const MAX_TITLE: usize = todo_core::decide::MAX_TITLE;
const MAX_NAME: usize = 200;
const MAX_ADDRESS: usize = 320;
const MAX_KEPT_BODY: usize = 64 * 1024;

type HmacSha256 = Hmac<Sha256>;

/// The HMAC key. Never printed.
pub struct HmacKey(Vec<u8>);

impl std::fmt::Debug for HmacKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HmacKey(<redacted>)")
    }
}

impl HmacKey {
    /// A key from the secret's UTF-8 bytes, used as is (as the Worker does).
    #[must_use]
    pub fn new(secret: &str) -> Self {
        Self(secret.as_bytes().to_vec())
    }
}

/// Reads the HMAC secret from `ingest.email_hmac_secret_file`, if configured. The file must be a
/// regular file (not a link), owned by and readable only by the account running the server (see
/// [`todo_fs::read_private_file`]); surrounding whitespace (a trailing newline) is ignored, and the
/// rest must be at least [`MIN_SECRET_LEN`] bytes. Errors never contain the secret.
pub fn load_secret(cfg: &IngestConfig) -> Result<Option<HmacKey>, ConfigError> {
    let Some(path) = &cfg.email_hmac_secret_file else {
        return Ok(None);
    };
    let bad = |why: String| {
        ConfigError::Invalid(format!(
            "ingest.email_hmac_secret_file {}: {why}",
            path.display()
        ))
    };
    // One open: the checks (a regular file, owned by this account, owner-only) are made on the
    // handle the secret is then read from.
    let bytes =
        todo_fs::read_private_file(path, MAX_SECRET_FILE).map_err(|e| bad(e.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|_| bad("the secret is not UTF-8".into()))?;
    let secret = text.trim();
    if secret.len() < MIN_SECRET_LEN {
        return Err(bad(format!(
            "the secret must be at least {MIN_SECRET_LEN} characters"
        )));
    }
    Ok(Some(HmacKey::new(secret)))
}

/// Lowercases and checks an email address (one `@`, non-empty local part and domain, no spaces or
/// control characters, at most 320 characters).
#[must_use]
pub fn normalize_address(s: &str) -> Option<String> {
    let a = s.trim().to_lowercase();
    let (local, domain) = a.split_once('@')?;
    let ok = !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && a.chars().count() <= MAX_ADDRESS
        && !a.chars().any(|c| c.is_whitespace() || c.is_control());
    ok.then_some(a)
}

/// The signature the Worker computes: lowercase hex HMAC-SHA256 over `timestamp \n nonce \n body`.
#[must_use]
pub fn sign(key: &HmacKey, timestamp: &str, nonce: &str, body: &[u8]) -> String {
    hex::encode(mac(key, timestamp, nonce, body).finalize().into_bytes())
}

fn mac(key: &HmacKey, timestamp: &str, nonce: &str, body: &[u8]) -> HmacSha256 {
    // HMAC takes keys of any length.
    let mut m = <HmacSha256 as Mac>::new_from_slice(&key.0).unwrap_or_else(|_| unreachable!());
    m.update(timestamp.as_bytes());
    m.update(b"\n");
    m.update(nonce.as_bytes());
    m.update(b"\n");
    m.update(body);
    m
}

/// Checks a signature in constant time.
#[must_use]
pub fn verify(key: &HmacKey, timestamp: &str, nonce: &str, body: &[u8], signature: &[u8]) -> bool {
    mac(key, timestamp, nonce, body)
        .verify_slice(signature)
        .is_ok()
}

/// The `mid` of the logs: the first 12 hex digits of the SHA-256 of the `Message-ID`.
#[must_use]
pub fn mid_hash(message_id: &str) -> String {
    let mut h = hex::encode(Sha256::digest(message_id.as_bytes()));
    h.truncate(12);
    h
}

/// Email ingest: the secret, the store and the per-IP limiter.
pub struct Ingest {
    key: Option<HmacKey>,
    /// Nonces, the sender allowlist and list keys.
    pub store: IngestStore,
    limiter: KeyedLimiter<IpAddr>,
}

impl std::fmt::Debug for Ingest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ingest")
            .field("enabled", &self.enabled())
            .finish_non_exhaustive()
    }
}

impl Ingest {
    /// Loads the secret and opens the store next to the engine's database. Blocking.
    pub fn new(config: &Config, db: &Path) -> Result<Self, IngestInitError> {
        let key = load_secret(&config.ingest)?;
        let store = IngestStore::open(db)?;
        let i = &config.ingest;
        Ok(Self {
            key,
            store,
            limiter: KeyedLimiter::new(i.requests_per_minute, i.request_burst),
        })
    }

    /// Whether the route is enabled (a secret is configured).
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.key.is_some()
    }

    /// Seeds the allowlist from the config on the first start (see [`IngestStore::seed_senders`]).
    /// Returns how many addresses were added, or `None` if it was seeded before. Blocking.
    pub fn seed(&self, config: &IngestConfig, now_ms: i64) -> Result<Option<usize>, StoreError> {
        let addresses: Vec<String> = config
            .allowed_senders
            .iter()
            .filter_map(|a| normalize_address(a))
            .collect();
        self.store.seed_senders(&addresses, now_ms)
    }
}

/// Why ingest could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum IngestInitError {
    /// Bad configuration (the secret file).
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Storage failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

// ---- the request ----------------------------------------------------------------------------------

/// The signed headers of a request.
struct Signed {
    timestamp: String,
    secs: i64,
    nonce: String,
    signature: [u8; 32],
}

/// The value of a header that must appear exactly once.
fn one<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut all = h.get_all(name).iter();
    let v = all.next()?;
    if all.next().is_some() {
        return None;
    }
    v.to_str().ok()
}

fn lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn signed_headers(h: &HeaderMap) -> Option<Signed> {
    let timestamp = one(h, "x-todo-timestamp")?;
    let nonce = one(h, "x-todo-nonce")?;
    let signature = one(h, "x-todo-signature")?;
    let digits = !timestamp.is_empty()
        && timestamp.len() <= 15
        && timestamp.bytes().all(|b| b.is_ascii_digit())
        && (timestamp == "0" || !timestamp.starts_with('0'));
    if !digits || !lower_hex(nonce, 32) || !lower_hex(signature, 64) {
        return None;
    }
    let mut sig = [0u8; 32];
    hex::decode_to_slice(signature, &mut sig).ok()?;
    Some(Signed {
        timestamp: timestamp.to_owned(),
        secs: timestamp.parse().ok()?,
        nonce: nonce.to_owned(),
        signature: sig,
    })
}

/// Who wrote the forwarded email.
#[derive(Debug, Clone, Deserialize)]
struct OriginalSender {
    address: String,
    #[serde(default)]
    name: Option<String>,
    #[allow(dead_code)]
    source: SenderSource,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SenderSource {
    ForwardedHeader,
    From,
}

#[derive(Debug, Clone, Deserialize)]
struct KeptBody {
    text: String,
    #[allow(dead_code)]
    source: BodySource,
    #[serde(default)]
    truncated: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BodySource {
    Text,
    Html,
}

/// The request body (schema version 1). Unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
struct Payload {
    version: u64,
    message_id: String,
    #[serde(default)]
    list: Option<String>,
    title: String,
    subject: String,
    keep: bool,
    original_sender: OriginalSender,
    forwarded_by: String,
    received_at: String,
    #[serde(default)]
    body: Option<KeptBody>,
    #[serde(default)]
    raw_truncated: bool,
    #[serde(default)]
    parse_failed: bool,
}

/// A checked payload.
#[derive(Debug, Clone)]
struct Email {
    p: Payload,
    forwarded_by: String,
    received_at: Timestamp,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn check(p: Payload) -> Option<Email> {
    let ok = p.version == 1
        && !p.message_id.trim().is_empty()
        && chars(&p.message_id) <= MAX_MESSAGE_ID
        && p.list.as_deref().is_none_or(store::valid_key)
        && !p.title.trim().is_empty()
        && chars(&p.title) <= MAX_TITLE
        && chars(&p.subject) <= MAX_SUBJECT
        && !p.original_sender.address.trim().is_empty()
        && chars(&p.original_sender.address) <= MAX_ADDRESS
        && p.original_sender
            .name
            .as_deref()
            .is_none_or(|n| chars(n) <= MAX_NAME)
        && p.body
            .as_ref()
            .is_none_or(|b| b.text.len() <= MAX_KEPT_BODY);
    if !ok {
        return None;
    }
    let forwarded_by = normalize_address(&p.forwarded_by)?;
    let received_at: Timestamp = p.received_at.parse().ok()?;
    Some(Email {
        p,
        forwarded_by,
        received_at,
    })
}

/// The `Message-ID` as the email link stores it, and as its dedupe key is computed from.
fn stored_message_id(raw: &str) -> &str {
    clip_bytes(raw.trim(), MAX_URL)
}

/// `s` cut to at most `max` bytes, at a character boundary.
fn clip_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Why an email went to the Inbox instead of the list it named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Miss {
    /// No list has the key.
    Unknown,
    /// The key's list was deleted.
    Deleted,
}

/// Where an email goes: the list, and why it isn't the one `key` asked for. `None` if it would go to
/// the Inbox and none is set (or it was deleted). Only explicit settings are used, never names.
fn route(
    state: &CoreState,
    conn: &Connection,
    key: Option<&str>,
) -> Result<Option<(ListId, Option<Miss>)>, StoreError> {
    let live = |l: &ListId| state.is_live((*l).into());
    let named = match key {
        None | Some(INBOX_KEY) => None,
        Some(k) => Some(match store::list_for_key(conn, k)? {
            Some(l) if live(&l) => return Ok(Some((l, None))),
            Some(_) => Miss::Deleted,
            None => Miss::Unknown,
        }),
    };
    Ok(store::inbox_list(conn)?
        .filter(live)
        .map(|inbox| (inbox, named)))
}

fn mailbox(address: &str, name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} <{address}>"),
        None => address.to_owned(),
    }
}

/// The task's notes: who sent and forwarded it and when, the subject, why it is in the Inbox, and the
/// kept body.
fn notes(e: &Email, zone: &TimeZone, missed: Option<(&str, Miss)>) -> String {
    let p = &e.p;
    let when = jiff::ZonedDateTime::new(e.received_at, zone.clone());
    let zone_name = zone.iana_name().unwrap_or("UTC");
    let mut n = format!(
        "From: {}\nForwarded by {} on {} ({zone_name})\nSubject: {}\n",
        mailbox(
            &p.original_sender.address,
            p.original_sender.name.as_deref()
        ),
        e.forwarded_by,
        when.strftime("%Y-%m-%d %H:%M"),
        p.subject,
    );
    match missed {
        Some((k, Miss::Unknown)) => n.push_str(&format!(
            "No list has the key \"{k}\", so this task went to the Inbox.\n"
        )),
        Some((k, Miss::Deleted)) => n.push_str(&format!(
            "The list with the key \"{k}\" was deleted, so this task went to the Inbox.\n"
        )),
        None => {}
    }
    if p.parse_failed {
        n.push_str("The email could not be parsed; the task was made from its headers only.\n");
    }
    if p.keep
        && let Some(b) = &p.body
    {
        n.push('\n');
        n.push_str(&b.text);
        if b.truncated || p.raw_truncated {
            n.push_str("\n\n[The email was too long; the text above is cut short.]");
        }
    }
    clip_bytes(&n, MAX_NOTES).to_owned()
}

/// What the plan found out (read back after [`crate::engine::Engine::execute_unique`]).
#[derive(Debug, Default)]
struct Planned {
    no_inbox: bool,
    fell_back: bool,
}

fn random_ids(now: Timestamp) -> Result<(TaskId, LinkId), ApiError> {
    let ms = u64::try_from(now.as_millisecond()).unwrap_or(0);
    let a = crate::auth::secret::random_bytes::<10>()?;
    let b = crate::auth::secret::random_bytes::<10>()?;
    Ok((TaskId::new_v7(ms, a), LinkId::new_v7(ms, b)))
}

fn plan(
    e: &Email,
    zone: &TimeZone,
    ids: (TaskId, LinkId),
    state: &CoreState,
    conn: &Connection,
    planned: &mut Planned,
) -> Result<UniquePlan, EngineError> {
    let p = &e.p;
    let Some((list, miss)) = route(state, conn, p.list.as_deref())? else {
        planned.no_inbox = true;
        return Err(EngineError::Unavailable);
    };
    let missed = p.list.as_deref().zip(miss);
    planned.fell_back = missed.is_some();
    let (task, link) = ids;
    let mut new = NewTask::new(task, list, p.title.trim());
    new.notes = notes(e, zone, missed);
    let sender = p.original_sender.address.trim();
    let subject = if p.subject.trim().is_empty() {
        p.title.trim()
    } else {
        p.subject.as_str()
    };
    let kind = LinkKind::Email(EmailLink {
        subject: clip_bytes(subject, MAX_URL).to_owned(),
        sender: clip_bytes(sender, MAX_URL).to_owned(),
        received_at: e.received_at,
        message_id: stored_message_id(&p.message_id).to_owned(),
        web_link: None,
    });
    Ok(UniquePlan {
        task,
        commands: vec![
            Command::CreateTask(Box::new(new)),
            Command::AddLink {
                id: link,
                task,
                kind,
            },
        ],
    })
}

fn unauthorized() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "the request is not signed correctly",
    )
}

fn not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "sender_not_allowed",
        "the forwarding address is not on the allowlist",
    )
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        "request body is too large",
    )
}

/// Turns a failed authentication into 401, counting it against the client's failure budget.
fn reject(app: &AppState, ip: Option<IpAddr>, reason: &'static str) -> ApiError {
    tracing::debug!(reason, "ingest request rejected");
    app.authsvc.bearer_failed(ip);
    unauthorized()
}

fn accepted(status: StatusCode, outcome: &str, task: TaskId) -> Response {
    (status, Json(json!({ "status": outcome, "task_id": task }))).into_response()
}

/// `POST /api/v1/ingest/email`. See the module docs.
#[utoipa::path(
    post, path = "/api/v1/ingest/email", tag = "ingest",
    description = "Email to task, for the Email Worker only. Takes no bearer token: the request is authenticated by an HMAC-SHA256 signature over `timestamp \n nonce \n body` with a secret shared with the Worker. The body is schema version 1 of `docs/ingest-email.md`, which is the full contract. Answers 404 unless the secret is configured.",
    params(
        ("X-Todo-Timestamp" = String, Header, description = "Unix seconds; within 300 s of the server clock"),
        ("X-Todo-Nonce" = String, Header, description = "32 lowercase hex digits, single use"),
        ("X-Todo-Signature" = String, Header, description = "Lowercase hex HMAC-SHA256"),
    ),
    request_body(content_type = "application/json", description = "Schema version 1 (see docs/ingest-email.md)"),
    responses(
        (status = 201, description = "Created: `{\"status\":\"created\",\"task_id\"}`"),
        (status = 200, description = "Duplicate Message-ID: `{\"status\":\"duplicate\",\"task_id\"}` with the task stored the first time"),
        (status = 401, description = "Missing, repeated or malformed signature headers, a stale timestamp, a bad signature or a replayed nonce", body = ErrorBody),
        (status = 403, description = "The forwarding address is not on the allowlist (`sender_not_allowed`)", body = ErrorBody),
        (status = 404, description = "Email ingest is not configured", body = ErrorBody),
        (status = 413, description = "Body over 128 KiB", body = ErrorBody),
        (status = 422, description = "The payload does not match schema version 1", body = ErrorBody),
        (status = 429, description = "Too many requests, or failed authentications, from this address; see `Retry-After`", body = ErrorBody),
        (status = 503, description = "No Inbox is set (`inbox_not_found`), or the server is busy; see `Retry-After`", body = ErrorBody),
    ),
    security(())
)]
pub async fn post_email(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    RequestCancel(cancel): RequestCancel,
    req: Request,
) -> Result<Response, ApiError> {
    let ingest = app.ingest.clone();
    if !ingest.enabled() {
        return Err(ApiError::not_found("no such endpoint"));
    }
    if let Some(ip) = ip {
        app.authsvc
            .bearer_admission(Some(ip))
            .and_then(|()| ingest.limiter.check(&ip))
            .map_err(ApiError::too_many_requests)?;
    }
    let declared = req
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_BODY_BYTES as u64) {
        return Err(too_large());
    }
    let Some(signed) = signed_headers(req.headers()) else {
        return Err(reject(&app, ip, "headers"));
    };
    let now = app.engine.now();
    if (now.as_second() - signed.secs).abs() > WINDOW_SECS {
        return Err(reject(&app, ip, "timestamp"));
    }
    let body = match axum::body::to_bytes(req.into_body(), MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => return Err(too_large()),
    };
    let Some(key) = ingest.key.as_ref() else {
        return Err(ApiError::not_found("no such endpoint"));
    };
    if !verify(
        key,
        &signed.timestamp,
        &signed.nonce,
        &body,
        &signed.signature,
    ) {
        return Err(reject(&app, ip, "signature"));
    }
    // Authenticated: from here on, the request comes from the Worker.
    let fresh = {
        let (ingest, nonce, at) = (ingest.clone(), signed.nonce, now.as_millisecond());
        blocking(&app, Work::Write, move || {
            ingest.store.reserve_nonce(&nonce, at)
        })
        .await??
    };
    if !fresh {
        return Err(reject(&app, ip, "replay"));
    }
    let Some(email) = serde_json::from_slice::<Payload>(&body)
        .ok()
        .and_then(check)
    else {
        tracing::warn!("ingest request with a valid signature but an invalid payload");
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_payload",
            "the payload does not match schema version 1",
        ));
    };
    let mid = mid_hash(&email.p.message_id);
    let allowed = {
        let (ingest, who) = (ingest.clone(), email.forwarded_by.clone());
        blocking(&app, Work::Read, move || ingest.store.is_allowed(&who)).await??
    };
    if !allowed {
        tracing::info!(mid = %mid, outcome = "sender_not_allowed", "email not ingested");
        return Err(not_allowed());
    }
    let key = crate::db::message_key(stored_message_id(&email.p.message_id));
    let ids = random_ids(now)?;
    let who = email.forwarded_by.clone();
    let (engine, zone) = (app.engine.clone(), app.zone.clone());
    let (has_list, keep) = (email.p.list.is_some(), email.p.keep);
    let (result, planned) = blocking(&app, Work::Write, move || {
        let mut planned = Planned::default();
        let r = engine.execute_unique(
            &Actor::EmailIngest,
            &key,
            &cancel,
            |s, c| plan(&email, &zone, ids, s, c, &mut planned),
            // Again inside the transaction: the sender may have been removed while this waited.
            |tx| store::sender_allowed(tx, &who),
        );
        (r, planned)
    })
    .await?;
    match result {
        Ok(UniqueOutcome::Created { task, .. }) => {
            tracing::info!(
                mid = %mid,
                outcome = "created",
                has_list,
                list_fallback = planned.fell_back,
                keep,
                "email ingested"
            );
            Ok(accepted(StatusCode::CREATED, "created", task))
        }
        Ok(UniqueOutcome::Duplicate { task }) => {
            tracing::info!(mid = %mid, outcome = "duplicate", "email ingested");
            Ok(accepted(StatusCode::OK, "duplicate", task))
        }
        Ok(UniqueOutcome::Refused) => {
            tracing::info!(mid = %mid, outcome = "sender_not_allowed", "email not ingested");
            Err(not_allowed())
        }
        Err(_) if planned.no_inbox => {
            tracing::warn!(
                mid = %mid,
                "cannot ingest email: no Inbox is set, or its list was deleted (pick one on /admin/ingest)"
            );
            Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "inbox_not_found",
                "no Inbox list is set for email",
            )
            .with_retry_after(300))
        }
        Err(e) => {
            tracing::warn!(mid = %mid, error = %e, "email not ingested");
            Err(match e {
                // Our own validation should have caught it; retrying cannot help.
                EngineError::Domain(_) => ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_payload",
                    "the payload cannot be stored as a task",
                ),
                other => other.into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Vector {
        secret: String,
        timestamp: String,
        nonce: String,
        body: String,
        body_sha256: String,
        signature: String,
    }

    fn vector() -> Vector {
        serde_json::from_str(include_str!("../../../../docs/ingest-email-signature.json")).unwrap()
    }

    #[test]
    fn the_shared_vector_verifies() {
        let v = vector();
        assert_eq!(
            hex::encode(Sha256::digest(v.body.as_bytes())),
            v.body_sha256
        );
        let key = HmacKey::new(&v.secret);
        assert_eq!(
            sign(&key, &v.timestamp, &v.nonce, v.body.as_bytes()),
            v.signature
        );
        let sig = hex::decode(&v.signature).unwrap();
        assert!(verify(
            &key,
            &v.timestamp,
            &v.nonce,
            v.body.as_bytes(),
            &sig
        ));
        // Any change to the key, the timestamp, the nonce or a body byte breaks it.
        assert!(!verify(
            &HmacKey::new("x"),
            &v.timestamp,
            &v.nonce,
            v.body.as_bytes(),
            &sig
        ));
        assert!(!verify(
            &key,
            "1763712001",
            &v.nonce,
            v.body.as_bytes(),
            &sig
        ));
        assert!(!verify(
            &key,
            &v.timestamp,
            "0".repeat(32).as_str(),
            v.body.as_bytes(),
            &sig
        ));
        let mut body = v.body.into_bytes();
        body[0] = b' ';
        assert!(!verify(&key, &v.timestamp, &v.nonce, &body, &sig));
        // The payload itself passes the schema check.
        let p: Payload = serde_json::from_str(&vector().body).unwrap();
        let e = check(p).unwrap();
        assert_eq!(e.forwarded_by, "you@example.com");
        assert!(e.p.keep);
    }

    #[test]
    fn headers_must_be_single_and_well_formed() {
        let v = vector();
        let base = || {
            let mut h = HeaderMap::new();
            h.insert("x-todo-timestamp", v.timestamp.parse().unwrap());
            h.insert("x-todo-nonce", v.nonce.parse().unwrap());
            h.insert("x-todo-signature", v.signature.parse().unwrap());
            h
        };
        assert!(signed_headers(&base()).is_some());
        for name in ["x-todo-timestamp", "x-todo-nonce", "x-todo-signature"] {
            let mut h = base();
            h.remove(name);
            assert!(signed_headers(&h).is_none(), "missing {name}");
            let mut h = base();
            let value = h.get(name).unwrap().clone();
            h.append(name, value);
            assert!(signed_headers(&h).is_none(), "repeated {name}");
        }
        for (name, bad) in [
            ("x-todo-timestamp", "01763712000"),
            ("x-todo-timestamp", "+1763712000"),
            ("x-todo-timestamp", "1763712000.5"),
            ("x-todo-timestamp", ""),
            ("x-todo-nonce", "00112233445566778899AABBCCDDEEFF"),
            ("x-todo-nonce", "0011"),
            ("x-todo-signature", &"A".repeat(64)),
            ("x-todo-signature", &"a".repeat(63)),
        ] {
            let mut h = base();
            h.insert(name, bad.parse().unwrap());
            assert!(signed_headers(&h).is_none(), "{name}: {bad}");
        }
    }

    #[test]
    fn addresses_and_message_ids_normalize() {
        assert_eq!(
            normalize_address("  You@Example.COM ").as_deref(),
            Some("you@example.com")
        );
        for bad in [
            "",
            "you",
            "@example.com",
            "you@",
            "a@b@c",
            "a b@example.com",
        ] {
            assert!(normalize_address(bad).is_none(), "{bad}");
        }
        assert_eq!(stored_message_id("  <a@b>\r\n"), "<a@b>");
        assert_eq!(clip_bytes("ab✓", 4), "ab");
        assert_eq!(mid_hash("<a@b>").len(), 12);
    }
}
