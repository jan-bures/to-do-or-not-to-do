//! Optional FCM wake-ups. The event log, not FCM, is the source of truth.

use crate::auth::service::AuthService;
use crate::config::PushConfig;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, StatusCode, Url, header};
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use rustls::pki_types::{PrivatePkcs8KeyDer, pem::PemObject};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use todo_core::event::{Event, EventPayload, FieldChange};
use todo_core::ids::EntityId;
use todo_core::{DeviceId, State};
use tokio::sync::{Notify, watch};
use tokio::time::{Instant, sleep_until};

#[cfg(test)]
#[path = "../tests/support/push_worker.rs"]
mod tests;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const METADATA_URL: &str =
    "http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/token";
const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const DEBOUNCE: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RETRY: Duration = Duration::from_secs(15 * 60);
const MAX_ATTEMPTS: u8 = 4;

pub(crate) fn valid_registration(token: &str) -> bool {
    (16..=4096).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
}

pub(crate) fn affects_alarms(before: &State, after: &State, events: &[Event]) -> bool {
    let mut tasks = BTreeSet::new();
    for event in events {
        let payload = &event.payload;
        match (payload.target(), payload) {
            (EntityId::Reminder(_), EventPayload::Updated { changes, .. }) => {
                if changes
                    .iter()
                    .any(|c| matches!(c, FieldChange::ReminderAt(_) | FieldChange::ParentTask(_)))
                {
                    return true;
                }
            }
            (EntityId::Reminder(_), _)
            | (
                EntityId::List(_) | EntityId::Collection(_),
                EventPayload::Deleted { .. }
                | EventPayload::Restored { .. }
                | EventPayload::Withdrawn { .. },
            ) => return true,
            (EntityId::List(_), EventPayload::Updated { changes, .. }) => {
                if changes
                    .iter()
                    .any(|c| matches!(c, FieldChange::ListCollection(_)))
                {
                    return true;
                }
            }
            (EntityId::Task(id), EventPayload::Updated { changes, .. }) => {
                if changes
                    .iter()
                    .any(|c| matches!(c, FieldChange::CompletedAt(_) | FieldChange::TaskList(_)))
                {
                    tasks.insert(id);
                }
            }
            (EntityId::Task(id), _) => {
                tasks.insert(id);
            }
            _ => {}
        }
    }
    !tasks.is_empty()
        && before
            .reminders
            .values()
            .chain(after.reminders.values())
            .any(|r| r.deleted.is_none() && tasks.contains(&r.task))
}

#[derive(Clone, Debug)]
struct Wake {
    pub(crate) origin: Option<DeviceId>,
    pub(crate) seq: u64,
}

/// Keep only the highest two hints with distinct origins. For any target, at least one is not
/// from that target if there was a remote change in the batch.
#[derive(Default)]
struct Batch {
    best: Option<Wake>,
    other: Option<Wake>,
}

impl Batch {
    fn add(&mut self, wake: Wake) {
        if let Some(best) = &mut self.best {
            if best.origin == wake.origin {
                best.seq = best.seq.max(wake.seq);
            } else if wake.seq >= best.seq {
                self.other = self.best.replace(wake);
            } else if self.other.as_ref().is_none_or(|w| w.seq <= wake.seq) {
                self.other = Some(wake);
            }
        } else {
            self.best = Some(wake);
        }
    }

    fn seq_for(&self, device: &str) -> Option<u64> {
        self.best
            .iter()
            .chain(&self.other)
            .find(|w| w.origin.as_ref().is_none_or(|d| d.as_str() != device))
            .map(|w| w.seq)
    }
}

/// Bounded even while the sender is stalled. The writer only merges two sequence hints here;
/// neither HTTP nor database work ever holds this lock.
#[derive(Default)]
pub(crate) struct Inbox {
    batch: Mutex<Batch>,
    ready: Notify,
}

impl Inbox {
    pub(crate) fn push(&self, origin: Option<DeviceId>, seq: u64) {
        self.batch
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add(Wake { origin, seq });
        self.ready.notify_one();
    }

    fn take(&self) -> Batch {
        std::mem::take(&mut *self.batch.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error("cannot build the FCM HTTP client")]
    Http,
    #[error("cannot read push.service_account_file")]
    File,
    #[error(
        "push.service_account_file must be a Google service-account JSON key with a PKCS#8 RSA key"
    )]
    Key,
}

#[derive(Deserialize)]
struct KeyFile {
    #[serde(rename = "type")]
    kind: String,
    client_email: String,
    private_key: String,
    private_key_id: String,
}

enum Credentials {
    Metadata {
        client: Client,
        url: String,
    },
    Key {
        email: String,
        key_id: String,
        key: Box<RsaKeyPair>,
    },
}

struct OAuth {
    credentials: Credentials,
    cached: Option<(String, Instant)>,
}

#[derive(Deserialize)]
struct AccessToken {
    access_token: String,
    expires_in: u64,
}

fn client(no_proxy: bool) -> Result<Client, InitError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let builder = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none());
    let builder = if no_proxy {
        builder.no_proxy()
    } else {
        builder
    };
    builder.build().map_err(|_| InitError::Http)
}

impl OAuth {
    fn new(config: &PushConfig) -> Result<Self, InitError> {
        let credentials = match &config.service_account_file {
            Some(path) => {
                use std::io::Read as _;
                let mut bytes = Vec::new();
                std::fs::File::open(path)
                    .and_then(|file| file.take(32 * 1024).read_to_end(&mut bytes))
                    .map_err(|_| InitError::File)?;
                let file: KeyFile = serde_json::from_slice(&bytes).map_err(|_| InitError::Key)?;
                if file.kind != "service_account"
                    || file.client_email.is_empty()
                    || file.private_key_id.is_empty()
                {
                    return Err(InitError::Key);
                }
                let der = PrivatePkcs8KeyDer::from_pem_slice(file.private_key.as_bytes())
                    .map_err(|_| InitError::Key)?;
                let key =
                    RsaKeyPair::from_pkcs8(der.secret_pkcs8_der()).map_err(|_| InitError::Key)?;
                Credentials::Key {
                    email: file.client_email,
                    key_id: file.private_key_id,
                    key: Box::new(key),
                }
            }
            None => Credentials::Metadata {
                client: client(true)?,
                url: METADATA_URL.into(),
            },
        };
        Ok(Self {
            credentials,
            cached: None,
        })
    }

    async fn token(&mut self, client: &Client) -> Result<&str, Duration> {
        if self
            .cached
            .as_ref()
            .is_none_or(|(_, until)| *until <= Instant::now())
        {
            let request = match &self.credentials {
                Credentials::Metadata { client, url } => {
                    client.get(url).header("Metadata-Flavor", "Google")
                }
                Credentials::Key { email, key_id, key } => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |t| t.as_secs());
                    let head = URL_SAFE_NO_PAD
                        .encode(json!({"alg":"RS256", "typ":"JWT", "kid":key_id}).to_string());
                    let claims = URL_SAFE_NO_PAD.encode(json!({"iss":email, "scope":FCM_SCOPE, "aud":TOKEN_URL, "iat":now, "exp":now + 3600}).to_string());
                    let mut jwt = format!("{head}.{claims}");
                    let mut signature = vec![0; key.public().modulus_len()];
                    key.sign(
                        &RSA_PKCS1_SHA256,
                        &ring::rand::SystemRandom::new(),
                        jwt.as_bytes(),
                        &mut signature,
                    )
                    .map_err(|_| Duration::from_secs(10))?;
                    jwt.push('.');
                    jwt.push_str(&URL_SAFE_NO_PAD.encode(signature));
                    // The assertion is base64url plus dots; none of its characters require escaping.
                    client.post(TOKEN_URL).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(format!("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={jwt}"))
                }
            };
            let wait = Duration::from_secs(10);
            let response = request.send().await.map_err(|_| wait)?;
            if !response.status().is_success() {
                return Err(retry_delay(&response));
            }
            if matches!(self.credentials, Credentials::Metadata { .. })
                && response
                    .headers()
                    .get("Metadata-Flavor")
                    .is_none_or(|v| v != "Google")
            {
                return Err(wait);
            }
            let token: AccessToken = response.json().await.map_err(|_| wait)?;
            if token.access_token.is_empty() || token.expires_in == 0 {
                return Err(wait);
            }
            let lifetime = Duration::from_secs(token.expires_in.min(3600).saturating_sub(60));
            self.cached = Some((token.access_token, Instant::now() + lifetime));
        }
        self.cached
            .as_ref()
            .map(|(token, _)| token.as_str())
            .ok_or(Duration::from_secs(10))
    }
}

pub(crate) struct Sender {
    project: String,
    endpoint: Url,
    client: Client,
    oauth: OAuth,
}

#[derive(Debug, PartialEq, Eq)]
enum Delivery {
    Done,
    Invalid,
    Retry(Duration),
    RefreshAuth,
}

fn retry_delay(response: &reqwest::Response) -> Duration {
    let min = if response.status() == StatusCode::TOO_MANY_REQUESTS {
        60
    } else {
        10
    };
    let requested = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
                httpdate::parse_http_date(s)
                    .ok()?
                    .duration_since(SystemTime::now())
                    .ok()
            })
        });
    requested.unwrap_or_default().max(Duration::from_secs(min))
}

async fn send(
    client: &Client,
    endpoint: &Url,
    bearer: &str,
    registration: &str,
    seq: u64,
) -> Delivery {
    let response = client
        .post(endpoint.clone())
        .bearer_auth(bearer)
        .json(&json!({"message": {
            "token": registration,
            "data": {"t": "sync", "seq": seq.to_string()},
            "android": {"priority": "HIGH", "collapse_key": "reminder-sync", "ttl": "3600s"}
        }}))
        .send()
        .await;
    let Ok(response) = response else {
        return Delivery::Retry(Duration::from_secs(10));
    };
    let status = response.status();
    if status.is_success() {
        return Delivery::Done;
    }
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        return Delivery::Retry(retry_delay(&response));
    }
    if status == StatusCode::UNAUTHORIZED {
        return Delivery::RefreshAuth;
    }
    if matches!(status, StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST) {
        let body = response.json::<Value>().await.unwrap_or_default();
        if invalid_registration(&body["error"]) {
            return Delivery::Invalid;
        }
    }
    // Google may echo a token in an error body. Log only the HTTP status.
    tracing::warn!(%status, "FCM rejected a reminder wake-up");
    Delivery::Done
}

fn invalid_registration(error: &Value) -> bool {
    if error["status"] == "UNREGISTERED" {
        return true;
    }
    // An invalid payload can also return INVALID_ARGUMENT. Require evidence about this token.
    let Some(details) = error["details"].as_array() else {
        return false;
    };
    details.iter().any(|d| match d["@type"].as_str() {
        Some("type.googleapis.com/google.firebase.fcm.v1.FcmError") => {
            d["errorCode"] == "UNREGISTERED"
                || (d["errorCode"] == "INVALID_ARGUMENT"
                    && error["message"]
                        == "The registration token is not a valid FCM registration token")
        }
        Some("type.googleapis.com/google.rpc.BadRequest") => {
            d["fieldViolations"].as_array().is_some_and(|fields| {
                !fields.is_empty() && fields.iter().all(|v| v["field"] == "message.token")
            })
        }
        _ => false,
    })
}

struct Pending {
    // None during cooldown after the retry budget was exhausted. New hints honor that cooldown.
    seq: Option<u64>,
    at: Instant,
    attempts: u8,
}

impl Pending {
    fn retry(&mut self, delay: Duration) {
        self.attempts += 1;
        let delay = delay.max(Duration::from_secs(10 << (self.attempts - 1)));
        self.at = Instant::now() + delay.min(Duration::from_secs(24 * 60 * 60));
        if self.attempts >= MAX_ATTEMPTS || delay > MAX_RETRY {
            self.seq = None;
            self.attempts = 0;
        }
    }
}

impl Sender {
    pub(crate) fn new(config: &PushConfig) -> Result<Self, InitError> {
        let endpoint = Url::parse(&format!(
            "https://fcm.googleapis.com/v1/projects/{}/messages:send",
            config.project_id
        ))
        .map_err(|_| InitError::Http)?;
        Ok(Self {
            project: config.project_id.clone(),
            endpoint,
            client: client(false)?,
            oauth: OAuth::new(config)?,
        })
    }

    pub(crate) async fn run(
        mut self,
        auth: Arc<AuthService>,
        wakes: Arc<Inbox>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        tokio::select! {
            _ = shutdown.wait_for(|s| *s) => {},
            () = self.loop_wakes(auth, &wakes) => {},
        }
    }

    async fn loop_wakes(&mut self, auth: Arc<AuthService>, wakes: &Inbox) {
        let mut flush_at = None;
        let mut pending = BTreeMap::<String, Pending>::new();
        loop {
            let earliest = flush_at
                .into_iter()
                .chain(pending.values().map(|p| p.at))
                .min();
            tokio::select! {
                biased;
                () = async { match earliest { Some(at) => sleep_until(at).await, None => std::future::pending().await } } => {},
                () = wakes.ready.notified(), if flush_at.is_none() => {
                    flush_at = Some(Instant::now() + DEBOUNCE);
                    continue;
                }
            }
            let now = Instant::now();
            let flush = flush_at.is_some_and(|at| at <= now);
            pending.retain(|_, p| p.seq.is_some() || p.at > now);
            if !flush && pending.values().all(|p| p.at > now) {
                continue;
            }
            let (db, project) = (auth.clone(), self.project.clone());
            let Ok(Ok(targets)) =
                tokio::task::spawn_blocking(move || db.store().push_targets(&project)).await
            else {
                tracing::warn!("could not read reminder wake-up registrations");
                if flush {
                    flush_at = Some(now + Duration::from_secs(10));
                }
                pending
                    .values_mut()
                    .filter(|p| p.at <= now)
                    .for_each(|p| p.at = now + Duration::from_secs(10));
                continue;
            };
            let targets = targets.into_iter().collect::<BTreeMap<_, _>>();
            pending.retain(|d, _| targets.contains_key(d));
            if flush {
                let batch = wakes.take();
                for device in targets.keys() {
                    if let Some(seq) = batch.seq_for(device) {
                        pending
                            .entry(device.clone())
                            .and_modify(|p| p.seq = p.seq.max(Some(seq)))
                            .or_insert(Pending {
                                seq: Some(seq),
                                at: now,
                                attempts: 0,
                            });
                    }
                }
                flush_at = None;
            }
            self.deliver(&auth, targets, &mut pending, now).await;
        }
    }

    async fn deliver(
        &mut self,
        auth: &Arc<AuthService>,
        targets: BTreeMap<String, String>,
        pending: &mut BTreeMap<String, Pending>,
        now: Instant,
    ) {
        use futures_util::{StreamExt as _, stream};
        let due = targets
            .into_iter()
            .filter_map(|(device, token)| {
                let p = pending.get(&device)?;
                (p.at <= now).then_some((device, token, p.seq?))
            })
            .collect::<Vec<_>>();
        if due.is_empty() {
            return;
        }
        let bearer = match self.oauth.token(&self.client).await {
            Ok(token) => token,
            Err(delay) => {
                tracing::warn!("could not obtain an FCM access token");
                for p in pending
                    .values_mut()
                    .filter(|p| p.at <= now && p.seq.is_some())
                {
                    p.retry(delay);
                }
                return;
            }
        };
        let (client, endpoint) = (&self.client, &self.endpoint);
        let mut results = stream::iter(due)
            .map(|(device, token, seq)| async move {
                let delivery = send(client, endpoint, bearer, &token, seq).await;
                (device, token, delivery)
            })
            .buffer_unordered(8);
        let mut refresh = false;
        while let Some((device, token, delivery)) = results.next().await {
            let delay = match delivery {
                Delivery::Retry(delay) => Some(delay),
                Delivery::RefreshAuth => {
                    refresh = true;
                    Some(Duration::from_secs(10))
                }
                Delivery::Invalid => {
                    let (db, project, device) =
                        (auth.clone(), self.project.clone(), device.clone());
                    if !matches!(
                        tokio::task::spawn_blocking(move || db
                            .store()
                            .remove_invalid_push(&device, &project, &token))
                        .await,
                        Ok(Ok(()))
                    ) {
                        tracing::warn!("could not clear an invalid FCM registration");
                    }
                    None
                }
                Delivery::Done => None,
            };
            if let Some(delay) = delay {
                if let Some(p) = pending.get_mut(&device) {
                    p.retry(delay);
                }
            } else {
                pending.remove(&device);
            }
        }
        drop(results);
        if refresh {
            self.oauth.cached = None;
        }
    }
}
