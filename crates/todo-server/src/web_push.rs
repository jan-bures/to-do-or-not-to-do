//! Web Push transport and scheduler. Claim before sending; release the claim only for an
//! explicitly retryable failure. A timeout can be ambiguous, so browsers coalesce by occurrence.

use crate::auth::service::AuthService;
use crate::engine::Engine;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use reqwest::{Client, StatusCode, Url, header::RETRY_AFTER};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::BTreeSet;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};
use todo_client::intent::reminder::{Alarm, alarms};
use todo_core::{ids::TaskId, time::Timestamp};
use tokio::sync::{Notify, watch};
use tokio::task::{JoinError, JoinSet};
use zeroize::Zeroizing;

const TTL_MS: i64 = 15 * 60 * 1000;
const MIN_TTL_MS: i64 = 1000;
const INACTIVITY_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const RESCAN: Duration = Duration::from_secs(60);
const PARALLEL_SENDS: usize = 8;
const AUTH_WARN_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const MAX_SUBSCRIPTIONS: i64 = 4;

type Occurrence = (i64, TaskId);

pub(crate) fn inactivity_cutoff(now: i64) -> i64 {
    now.saturating_sub(INACTIVITY_MS)
}

fn secret_key() -> io::Result<SecretKey> {
    let mut bytes = Zeroizing::new([0; 32]);
    loop {
        getrandom::fill(&mut *bytes).map_err(|_| io::Error::other("randomness unavailable"))?;
        if let Ok(key) = SecretKey::from_slice(&*bytes) {
            return Ok(key);
        }
    }
}

/// Generates a VAPID private key without printing it or replacing an existing file.
pub fn write_vapid_key(path: &Path) -> io::Result<()> {
    let key = secret_key()?;
    let bytes = Zeroizing::new(<[u8; 32]>::from(key.to_bytes()));
    let mut file = todo_fs::create_private_file(path)?;
    file.write_all(&*bytes)?;
    file.sync_all()
}

pub(crate) fn endpoint(raw: &str) -> Option<Url> {
    if raw.len() > 4096 || raw.bytes().any(|b| !b.is_ascii_graphic() || b == b'\\') {
        return None;
    }
    let authority = raw
        .strip_prefix("https://")?
        .split(['/', '?', '#'])
        .next()?;
    if authority.is_empty() || authority.contains(['@', '%']) {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if url.as_str().len() > 4096
        || url.port().is_some()
        || url.fragment().is_some()
        || url.path() == "/"
    {
        return None;
    }
    let host = url.host_str()?;
    let provider_host = |suffix| {
        host.strip_suffix(suffix).is_some_and(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b: u8| b.is_ascii_alphanumeric() || b == b'-')
        })
    };
    (matches!(
        host,
        "fcm.googleapis.com" | "updates.push.services.mozilla.com"
    ) || provider_host(".push.apple.com")
        || provider_host(".notify.windows.com"))
    .then_some(url)
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Registration {
    endpoint: String,
    keys: Keys,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct Keys {
    p256dh: String,
    auth: String,
}

pub(crate) struct Subscription {
    pub(crate) endpoint: Url,
    pub(crate) p256dh: [u8; 65],
    pub(crate) auth: [u8; 16],
}

impl Registration {
    pub(crate) fn validate(self) -> Option<Subscription> {
        let p256dh: [u8; 65] = B64.decode(&self.keys.p256dh).ok()?.try_into().ok()?;
        let auth = B64.decode(&self.keys.auth).ok()?.try_into().ok()?;
        if p256dh[0] != 4 || PublicKey::from_sec1_bytes(&p256dh).is_err() {
            return None;
        }
        Some(Subscription {
            endpoint: endpoint(&self.endpoint)?,
            p256dh,
            auth,
        })
    }
}

pub(crate) struct Target {
    pub(crate) id: i64,
    pub(crate) endpoint: String,
    pub(crate) p256dh: Vec<u8>,
    pub(crate) auth: Vec<u8>,
}

pub(crate) struct Sender {
    key: SigningKey,
    public_key: String,
    subject: Option<String>,
    client: Client,
    last_auth_warning: Mutex<Option<Instant>>,
    #[cfg(test)]
    test_destination: Option<Url>,
}

impl std::fmt::Debug for Sender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebPushSender").finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error("cannot read the owner-only web_push.vapid_key_file")]
    File,
    #[error("web_push.vapid_key_file is not a VAPID key (use todo-server web-push-key)")]
    Key,
    #[error("cannot build the Web Push HTTP client")]
    Http,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Accepted,
    Gone,
    Failed,
    Unknown,
    Retry(Option<RetryAfter>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryAfter {
    Delay(i64),
    Date(i64),
}

impl RetryAfter {
    fn parse(value: &str) -> Option<Self> {
        if let Ok(seconds) = value.parse::<u64>() {
            return Some(Self::Delay(
                i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX),
            ));
        }
        let date = httpdate::parse_http_date(value).ok()?;
        let ms = date.duration_since(UNIX_EPOCH).ok()?.as_millis();
        Some(Self::Date(i64::try_from(ms).unwrap_or(i64::MAX)))
    }
}

fn next_retry(at: i64, now: i64, attempt: u32, retry_after: Option<RetryAfter>) -> Option<i64> {
    let backoff = 15_000 * (1_i64 << attempt.saturating_sub(1).min(3));
    let requested = match retry_after {
        Some(RetryAfter::Delay(ms)) => now.saturating_add(ms),
        Some(RetryAfter::Date(ms)) => ms,
        None => now,
    };
    let next = now.saturating_add(backoff).max(requested);
    (next <= at.saturating_add(TTL_MS - MIN_TTL_MS)).then_some(next)
}

#[derive(Serialize)]
struct Reminder<'a> {
    task: TaskId,
    at: i64,
    title: &'a str,
}

fn reminder_body(alarm: Alarm, title: &str) -> Result<Vec<u8>, serde_json::Error> {
    let end = title
        .char_indices()
        .nth(200)
        .map_or(title.len(), |(end, _)| end);
    serde_json::to_vec(&Reminder {
        task: alarm.task,
        at: alarm.at.as_millisecond(),
        title: &title[..end],
    })
}

impl Sender {
    pub(crate) fn load(path: &Path, subject: Option<String>) -> Result<Self, InitError> {
        let bytes =
            Zeroizing::new(todo_fs::read_private_file(path, 32).map_err(|_| InitError::File)?);
        let key = SecretKey::from_slice(&bytes).map_err(|_| InitError::Key)?;
        if bytes.len() != 32 {
            return Err(InitError::Key);
        }
        Self::new(key, subject)
    }

    fn new(key: SecretKey, subject: Option<String>) -> Result<Self, InitError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder()
            .https_only(true)
            .no_proxy()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| InitError::Http)?;
        let public_key = B64.encode(key.public_key().to_encoded_point(false).as_bytes());
        Ok(Self {
            key: key.into(),
            public_key,
            subject,
            client,
            last_auth_warning: Mutex::new(None),
            #[cfg(test)]
            test_destination: None,
        })
    }

    pub(crate) fn public_key(&self) -> &str {
        &self.public_key
    }

    fn authorization(&self, url: &Url, now: i64) -> String {
        // RFC 8292: the audience is the push service's origin, not the subscription URL.
        let mut claims = serde_json::json!({
            "aud": url.origin().ascii_serialization(), "exp": now + 12 * 60 * 60,
        });
        if let Some(subject) = &self.subject {
            claims["sub"] = serde_json::Value::String(subject.clone());
        }
        let input = format!(
            "{}.{}",
            B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#),
            B64.encode(claims.to_string())
        );
        let signature: Signature = self.key.sign(input.as_bytes());
        format!(
            "vapid t={input}.{}, k={}",
            B64.encode(signature.to_bytes()),
            self.public_key
        )
    }

    pub(crate) async fn deliver(
        &self,
        auth: &Arc<AuthService>,
        target: Target,
        body: &[u8],
        ttl: u64,
    ) -> Delivery {
        // Revalidate stored endpoints too, including after migration/restore. No redirect or proxy
        // can turn an allowed destination into a request to a host outside the allowlist.
        let delivery = match endpoint(&target.endpoint) {
            Some(url) => {
                self.send(url, &target, body, ttl, auth.now().div_euclid(1000))
                    .await
            }
            None => Delivery::Gone,
        };
        if delivery == Delivery::Gone {
            let auth = auth.clone();
            if !matches!(
                tokio::task::spawn_blocking(move || auth.store().remove_web_push(target.id)).await,
                Ok(Ok(()))
            ) {
                tracing::warn!("could not remove an expired Web Push subscription");
            }
        }
        delivery
    }

    async fn send(&self, url: Url, target: &Target, body: &[u8], ttl: u64, now: i64) -> Delivery {
        let Ok(receiver) = PublicKey::from_sec1_bytes(&target.p256dh) else {
            return Delivery::Gone;
        };
        let Ok(secret) = secret_key() else {
            return Delivery::Failed;
        };
        let mut salt = [0; 16];
        if getrandom::fill(&mut salt).is_err() {
            return Delivery::Failed;
        }
        let Some(encrypted) = encrypt(body, &secret, &receiver, &target.auth, &salt) else {
            return Delivery::Failed;
        };
        let auth = self.authorization(&url, now);
        #[cfg(test)]
        let url = self.test_destination.clone().unwrap_or(url);
        match self
            .client
            .post(url)
            .header(reqwest::header::AUTHORIZATION, auth)
            .header(reqwest::header::CONTENT_ENCODING, "aes128gcm")
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header("TTL", ttl)
            .header("Urgency", "high")
            .body(encrypted)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => Delivery::Accepted,
            Ok(response)
                if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::GONE) =>
            {
                Delivery::Gone
            }
            Ok(response) => self.rejected(
                response.status(),
                response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok()),
            ),
            Err(error) if error.is_connect() && !error.is_timeout() => Delivery::Retry(None),
            // A timeout or other transport error can happen after acceptance. Keep the claim.
            // Do not log the error, which can include the secret endpoint.
            Err(_) => {
                tracing::debug!("Web Push delivery outcome unknown after a transport error");
                Delivery::Unknown
            }
        }
    }

    fn rejected(&self, status: StatusCode, retry_after: Option<&str>) -> Delivery {
        if matches!(
            status,
            StatusCode::TOO_MANY_REQUESTS
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::BAD_GATEWAY
                | StatusCode::GATEWAY_TIMEOUT
        ) {
            return Delivery::Retry(retry_after.and_then(RetryAfter::parse));
        }
        // Other 5xx responses may come from a service that already accepted the message.
        if status.is_server_error() {
            return Delivery::Unknown;
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            let mut last = self
                .last_auth_warning
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            if last.is_none_or(|last| now.duration_since(last) >= AUTH_WARN_INTERVAL) {
                *last = Some(now);
                tracing::warn!(
                    status = status.as_u16(),
                    "Web Push service rejected authentication"
                );
            }
        } else {
            tracing::debug!(
                status = status.as_u16(),
                "Web Push service declined a message"
            );
        }
        Delivery::Failed
    }

    pub(crate) async fn run(
        self: Arc<Self>,
        engine: Arc<Engine>,
        auth: Arc<AuthService>,
        wake: Arc<Notify>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        tokio::select! {
            _ = shutdown.wait_for(|s| *s) => {},
            () = self.schedule(&engine, &auth, &wake) => {},
        }
    }

    async fn schedule(
        self: &Arc<Self>,
        engine: &Arc<Engine>,
        auth: &Arc<AuthService>,
        wake: &Notify,
    ) {
        let mut finished = BTreeSet::new();
        loop {
            let next = self.rescan(engine, auth, &mut finished).await;
            let delay = next.map_or(RESCAN, |at| {
                let ms = at.saturating_sub(engine.now().as_millisecond());
                Duration::from_millis(ms.max(1).unsigned_abs()).min(RESCAN)
            });
            tokio::select! {
                () = wake.notified() => {},
                () = tokio::time::sleep(delay) => {},
            }
        }
    }

    async fn rescan(
        self: &Arc<Self>,
        engine: &Arc<Engine>,
        auth: &Arc<AuthService>,
        finished: &mut BTreeSet<Occurrence>,
    ) -> Option<i64> {
        let now = engine.now();
        let now_ms = now.as_millisecond();
        let cutoff_ms = now_ms.saturating_sub(TTL_MS);
        let cutoff = Timestamp::from_millisecond(cutoff_ms).unwrap_or(Timestamp::MIN);
        finished.retain(|(at, _)| *at > cutoff_ms);
        prune(auth, now_ms).await;

        let mut next = None;
        let mut seen = BTreeSet::new();
        let mut sends = JoinSet::new();
        for alarm in alarms(&engine.state(), cutoff) {
            let at = alarm.at.as_millisecond();
            if alarm.at > now {
                next = earliest(next, Some(at));
                break;
            }
            let occurrence = (at, alarm.task);
            if at < cutoff_ms.saturating_add(MIN_TTL_MS)
                || finished.contains(&occurrence)
                || !seen.insert(occurrence)
            {
                continue;
            }
            let (db, key, task) = (
                auth.clone(),
                self.public_key.clone(),
                alarm.task.to_string(),
            );
            let Ok(Ok(targets)) = tokio::task::spawn_blocking(move || {
                db.store().unclaimed_web_push(&key, &task, at, now_ms)
            })
            .await
            else {
                tracing::warn!("could not load Web Push targets");
                continue;
            };
            if targets.is_empty() {
                finished.insert(occurrence);
            }
            // Spawn immediately so sends progress during target lookups. The JoinSet bounds
            // concurrency across alarms and aborts outstanding sends when shutdown cancels rescan.
            for (id, retry_at) in targets {
                if let Some(retry) = retry_at.filter(|&retry| retry > now_ms) {
                    next = earliest(next, Some(retry));
                    continue;
                }
                if sends.len() >= PARALLEL_SENDS {
                    next = earliest(next, sends.join_next().await.and_then(retry_from_send));
                }
                let (sender, engine, auth) = (self.clone(), engine.clone(), auth.clone());
                sends.spawn(async move { sender.remind(&engine, &auth, alarm, id).await });
            }
        }
        while let Some(result) = sends.join_next().await {
            next = earliest(next, retry_from_send(result));
        }
        next
    }

    async fn remind(
        &self,
        engine: &Engine,
        auth: &Arc<AuthService>,
        alarm: Alarm,
        id: i64,
    ) -> Option<i64> {
        let at = alarm.at.as_millisecond();
        let expires = at.saturating_add(TTL_MS);
        let now = engine.now().as_millisecond();
        if expires.saturating_sub(now) < MIN_TTL_MS || !alarm.is_current(&engine.state()) {
            return None;
        }
        let task = alarm.task.to_string();
        let (db, key) = (auth.clone(), task.clone());
        let result =
            tokio::task::spawn_blocking(move || db.store().claim_web_push(id, &key, at, now)).await;
        let (target, attempt) = match result {
            Ok(Ok(Some(claim))) => claim,
            Ok(Ok(None)) => return None,
            _ => {
                tracing::warn!("could not claim a Web Push delivery");
                return None;
            }
        };
        let remaining = expires.saturating_sub(engine.now().as_millisecond());
        let body = {
            let state = engine.state();
            if remaining < MIN_TTL_MS || !alarm.is_current(&state) {
                return None;
            }
            let task_row = state.tasks.get(&alarm.task)?;
            reminder_body(alarm, &task_row.title).ok()?
        };
        let Delivery::Retry(requested) = self
            .deliver(auth, target, &body, remaining.unsigned_abs() / 1000)
            .await
        else {
            return None;
        };
        let next = next_retry(at, engine.now().as_millisecond(), attempt, requested)?;
        let db = auth.clone();
        if !matches!(
            tokio::task::spawn_blocking(move || db.store().retry_web_push(id, &task, at, next))
                .await,
            Ok(Ok(()))
        ) {
            tracing::warn!("could not release a Web Push delivery claim for retry");
            return None;
        }
        Some(next)
    }
}

pub(crate) async fn prune(auth: &Arc<AuthService>, now: i64) {
    let db = auth.clone();
    let cutoff = now.saturating_sub(TTL_MS);
    if !matches!(
        tokio::task::spawn_blocking(move || db.store().prune_web_push(now, cutoff)).await,
        Ok(Ok(()))
    ) {
        tracing::warn!("could not prune Web Push subscriptions and claims");
    }
}

fn retry_from_send(result: Result<Option<i64>, JoinError>) -> Option<i64> {
    match result {
        Ok(retry) => retry,
        Err(_) => {
            tracing::warn!("Web Push send task did not complete");
            None
        }
    }
}

fn earliest(current: Option<i64>, candidate: Option<i64>) -> Option<i64> {
    current.into_iter().chain(candidate).min()
}

/// RFC 8291 section 3 and RFC 8188: a single final record, no padding beyond the 0x02 delimiter.
fn encrypt(
    body: &[u8],
    sender: &SecretKey,
    receiver: &PublicKey,
    auth: &[u8],
    salt: &[u8; 16],
) -> Option<Vec<u8>> {
    const RECORD_SIZE: u32 = 4096;
    const HEADER_SIZE: usize = 16 + 4 + 1 + 65;
    if body.len() > RECORD_SIZE as usize - HEADER_SIZE - 16 - 1 || auth.len() != 16 {
        return None;
    }
    let receiver_point = receiver.to_encoded_point(false);
    let sender_point = sender.public_key().to_encoded_point(false);
    let shared = diffie_hellman(sender.to_nonzero_scalar(), receiver.as_affine());
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(receiver_point.as_bytes());
    info.extend_from_slice(sender_point.as_bytes());
    let mut ikm = Zeroizing::new([0; 32]);
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&info, &mut *ikm)
        .ok()?;
    let hkdf = Hkdf::<Sha256>::new(Some(salt), &*ikm);
    let mut key = Zeroizing::new([0; 16]);
    let mut nonce = [0; 12];
    hkdf.expand(b"Content-Encoding: aes128gcm\0", &mut *key)
        .ok()?;
    hkdf.expand(b"Content-Encoding: nonce\0", &mut nonce).ok()?;
    let mut plaintext = body.to_vec();
    plaintext.push(2);
    let record = Aes128Gcm::new((&*key).into())
        .encrypt(&Nonce::from(nonce), plaintext.as_slice())
        .ok()?;
    let mut result = Vec::with_capacity(HEADER_SIZE + record.len());
    result.extend_from_slice(salt);
    result.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    result.push(65);
    result.extend_from_slice(sender_point.as_bytes());
    result.extend(record);
    Some(result)
}

#[cfg(test)]
mod tests;
