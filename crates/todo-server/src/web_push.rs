//! Web Push transport and scheduler. Write the delivery claim before making the request: an
//! ambiguous network failure or server restart must not send the same occurrence again.

use crate::auth::service::AuthService;
use crate::engine::Engine;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use futures_util::{StreamExt, stream};
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use todo_client::intent::reminder::{Alarm, alarms};
use todo_core::{ids::TaskId, time::Timestamp};
use tokio::sync::{Notify, watch};
use zeroize::Zeroizing;

const TTL: u64 = 15 * 60;
const RESCAN: Duration = Duration::from_secs(60);
const PARALLEL_SENDS: usize = 8;
pub(crate) const MAX_SUBSCRIPTIONS: i64 = 4;

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
}

#[derive(Serialize)]
struct Reminder {
    task: TaskId,
    at: i64,
}

fn reminder_body(alarm: Alarm) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&Reminder {
        task: alarm.task,
        at: alarm.at.as_millisecond(),
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
            Ok(response) => {
                tracing::debug!(status = %response.status(), "Web Push service declined a message");
                Delivery::Failed
            }
            // A reqwest error can include the endpoint, and the provider may echo it in its body.
            Err(_) => {
                tracing::debug!("Web Push transport failed");
                Delivery::Failed
            }
        }
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

    async fn schedule(&self, engine: &Engine, auth: &Arc<AuthService>, wake: &Notify) {
        loop {
            let now = engine.now();
            let cutoff_ms = now.as_millisecond().saturating_sub(TTL as i64 * 1000);
            let cutoff = Timestamp::from_millisecond(cutoff_ms).unwrap_or(Timestamp::MIN);
            let (idle, absolute) = auth.session_cutoffs(now.as_millisecond());
            let db = auth.clone();
            if !matches!(
                tokio::task::spawn_blocking(move || db
                    .store()
                    .prune_web_push(idle, absolute, cutoff_ms))
                .await,
                Ok(Ok(()))
            ) {
                tracing::warn!("could not prune Web Push subscriptions and claims");
            }
            let mut next = None;
            let mut last = std::collections::BTreeSet::new();
            for alarm in alarms(&engine.state(), cutoff) {
                if alarm.at > now {
                    next = Some(alarm.at);
                    break;
                }
                if last.insert((alarm.task, alarm.at.as_millisecond())) {
                    self.remind(engine, auth, alarm).await;
                }
            }
            let delay = next.map_or(RESCAN, |at| {
                let ms = at
                    .as_millisecond()
                    .saturating_sub(engine.now().as_millisecond());
                Duration::from_millis(ms.max(1).unsigned_abs()).min(RESCAN)
            });
            tokio::select! {
                () = wake.notified() => {},
                () = tokio::time::sleep(delay) => {},
            }
        }
    }

    async fn remind(&self, engine: &Engine, auth: &Arc<AuthService>, alarm: Alarm) {
        let Ok(body) = reminder_body(alarm) else {
            return;
        };
        let at = alarm.at.as_millisecond();
        let task = alarm.task.to_string();
        let (db, key, task_id) = (auth.clone(), self.public_key.clone(), task.clone());
        let Ok(Ok(ids)) =
            tokio::task::spawn_blocking(move || db.store().unclaimed_web_push(&key, &task_id, at))
                .await
        else {
            tracing::warn!("could not load Web Push targets");
            return;
        };
        stream::iter(ids)
            .for_each_concurrent(PARALLEL_SENDS, |id| {
                let (task, body) = (task.clone(), &body);
                async move {
                    let now = engine.now().as_millisecond();
                    let elapsed = now.saturating_sub(at).max(0).unsigned_abs() / 1000;
                    let ttl = TTL.saturating_sub(elapsed);
                    if ttl == 0 || !alarm.is_current(&engine.state()) {
                        return;
                    }
                    let (idle, absolute) = auth.session_cutoffs(now);
                    let db = auth.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        db.store().claim_web_push(id, &task, at, idle, absolute)
                    })
                    .await;
                    match result {
                        Ok(Ok(Some(target))) => {
                            self.deliver(auth, target, body, ttl).await;
                        }
                        Ok(Ok(None)) => {}
                        _ => tracing::warn!("could not claim a Web Push delivery"),
                    }
                }
            })
            .await;
    }
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
