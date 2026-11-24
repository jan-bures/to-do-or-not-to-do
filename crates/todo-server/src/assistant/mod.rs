//! Owner-only inference relay. It does not submit commands or grant a model access to task APIs.

mod openai;
#[cfg(test)]
mod tests;
mod upstream;
mod vault;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{FromRequestParts, Path as Route, State},
    http::{Method, StatusCode, request::Parts},
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::Value;
use todo_tools::assistant::{
    access::DataClass,
    relay::{self, Account, Poll, Status},
};
use tokio::{
    sync::{Mutex, OwnedMutexGuard, Semaphore, watch},
    time::Instant,
};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    api::error::ApiError,
    app::AppState,
    auth::{Authenticated, Principal, limits::KeyedLimiter},
    config::ConfigError,
    engine::Engine,
};
use openai::{DeviceGrant, LoopbackGrant, OpenAi, RevokeError};
use upstream::Private;
use vault::{Credential, Flow, Records, Revocation, Vault};

pub(crate) use upstream::PRIVATE_CHAT_HEAD_TIMEOUT;
pub use upstream::PrivateConfig;

const MAX_ATTEMPTS: usize = 8;
const MAX_ACCOUNTS: usize = 12;
const MAX_RULES: usize = 4096;
const DEVICE_EXPIRY: Duration = Duration::from_secs(15 * 60);
const LOOPBACK_EXPIRY: Duration = Duration::from_secs(600);
const POLL_WAIT: Duration = Duration::from_secs(5);
const REFRESH_MARGIN: u64 = 60;

/// Optional server-assistant settings. Omitting the section disables the relay.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Enable ChatGPT sign-in, including the installed client's browser handoff.
    pub chatgpt: bool,
    /// OAuth client ID authorized for OpenAI device-code login and ChatGPT inference.
    pub chatgpt_device_client_id: Option<String>,
    /// An owner-only encryption key file, created if absent on a fresh installation.
    /// Defaults to `/etc/todo/assistant.secret` on Linux and is required elsewhere.
    pub key_file: Option<PathBuf>,
    /// Optional private model gateway.
    pub private: Option<PrivateConfig>,
}

impl Config {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        for (invalid, reason) in [
            (
                self.chatgpt_device_client_id
                    .as_deref()
                    .is_some_and(|id| !self.chatgpt || !openai::valid_client_id(id)),
                "assistant.chatgpt_device_client_id requires chatgpt = true and a valid OAuth client ID",
            ),
            (
                self.key_file
                    .as_ref()
                    .is_some_and(|p| p.as_os_str().is_empty()),
                "assistant.key_file must not be empty",
            ),
            (
                self.private.as_ref().is_some_and(|cfg| !cfg.valid()),
                "assistant.private needs an HTTP(S) URL on loopback or Tailscale and a key_file",
            ),
        ] {
            if invalid {
                return Err(ConfigError::Invalid(reason.into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error(transparent)]
    Vault(#[from] vault::Error),
    #[error("assistant.key_file must be set explicitly outside Linux")]
    MissingKeyPath,
    #[error("invalid assistant private model base URL")]
    PrivateAddress,
    #[error("could not read the assistant private model key file: {0}")]
    PrivateKey(std::io::Error),
    #[error("invalid assistant private model key")]
    BadPrivateKey,
    #[error("could not initialize the assistant HTTP client")]
    Http,
}

impl InitError {
    pub(crate) fn owner_reason(&self) -> &'static str {
        match self {
            Self::Vault(_) => "Could not open the assistant vault or its encryption key.",
            Self::MissingKeyPath => "Set assistant.key_file to enable the relay on this server.",
            Self::PrivateKey(_) | Self::BadPrivateKey => "Could not load the private model key.",
            Self::PrivateAddress => "The private model address is invalid.",
            Self::Http => "Could not initialize the assistant HTTP client.",
        }
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
pub(super) enum Error {
    #[error("assistant provider is disabled")]
    Disabled,
    #[error("assistant provider request failed")]
    Upstream,
    #[error("assistant provider request timed out")]
    Timeout,
    #[error("assistant provider rejected sign-in")]
    UpstreamAuthorization,
    #[error("assistant provider rate limit reached")]
    UpstreamLimit,
    #[error("this model is not available")]
    ModelMissing,
    #[error("the assistant provider does not support this request")]
    Unsupported,
    #[error("invalid assistant request")]
    BadRequest,
    #[error("assistant provider identity could not be verified")]
    Identity,
    #[error("that ChatGPT account is not connected")]
    AccountMissing,
    #[error("there are too many connected ChatGPT accounts")]
    AccountsFull,
    #[error("settings have changed; refresh settings to continue")]
    AccessChanged,
    #[error("the request does not match the server's sharing settings")]
    Scope,
    #[error("approve these lists before sharing them with ChatGPT")]
    Consent,
    #[error("assistant storage could not be updated")]
    Storage,
    #[error("too many assistant requests")]
    Rate(u32),
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        let (status, code) = match err {
            Error::Disabled => (StatusCode::NOT_FOUND, "assistant_disabled"),
            Error::Timeout => (StatusCode::GATEWAY_TIMEOUT, "assistant_timeout"),
            Error::UpstreamAuthorization => {
                (StatusCode::FAILED_DEPENDENCY, "assistant_upstream_auth")
            }
            Error::UpstreamLimit => (StatusCode::TOO_MANY_REQUESTS, "assistant_upstream_limit"),
            Error::ModelMissing => (StatusCode::NOT_FOUND, "assistant_model_missing"),
            Error::Unsupported | Error::BadRequest => {
                (StatusCode::BAD_REQUEST, "assistant_request")
            }
            Error::Identity => {
                tracing::warn!("assistant provider identity check failed");
                (StatusCode::BAD_GATEWAY, "assistant_provider")
            }
            Error::Upstream => (StatusCode::BAD_GATEWAY, "assistant_provider"),
            Error::AccountMissing => (StatusCode::NOT_FOUND, "assistant_account_missing"),
            Error::AccountsFull => (StatusCode::CONFLICT, "assistant_accounts_full"),
            Error::AccessChanged => (StatusCode::CONFLICT, "assistant_access_changed"),
            Error::Scope => (StatusCode::FORBIDDEN, "assistant_scope"),
            Error::Consent => (StatusCode::CONFLICT, "assistant_consent"),
            Error::Storage => (StatusCode::INTERNAL_SERVER_ERROR, "assistant_storage"),
            Error::Rate(wait) => return Self::too_many_requests(wait),
        };
        Self::new(status, code, err.to_string())
    }
}

struct Attempt {
    owner: Principal,
    valid_until: Instant,
    state: Arc<Mutex<AttemptState>>,
}

enum AttemptState {
    Device { grant: DeviceGrant, next: Instant },
    Loopback(LoopbackGrant),
    Finished(Uuid),
    Failed,
}

type RefreshResult = watch::Receiver<Option<Result<Credential, Error>>>;

pub(crate) struct Assistant {
    openai: Option<OpenAi>,
    private: Option<Private>,
    device_enabled: bool,
    vault: Arc<Vault>,
    records: Arc<Mutex<Records>>,
    attempts: Mutex<HashMap<Uuid, Attempt>>,
    start_gate: Mutex<()>,
    account_gates: Mutex<HashMap<Uuid, Weak<Mutex<()>>>>,
    refreshing: Mutex<HashMap<Uuid, RefreshResult>>,
    revocation_gate: Mutex<()>,
    starts: KeyedLimiter<()>,
    inference: KeyedLimiter<()>,
    slots: Arc<Semaphore>,
}

impl std::fmt::Debug for Assistant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assistant").finish_non_exhaustive()
    }
}

impl Assistant {
    pub(crate) fn open(config: Config, dir: &Path) -> Result<Option<Arc<Self>>, InitError> {
        if !config.chatgpt && config.private.is_none() {
            return Ok(None);
        }
        let device_enabled = config.chatgpt_device_client_id.is_some();
        let openai = config
            .chatgpt
            .then(|| OpenAi::new(config.chatgpt_device_client_id))
            .transpose()?;
        let private = config.private.as_ref().map(Private::open).transpose()?;
        let key = config
            .key_file
            .or_else(|| {
                cfg!(target_os = "linux").then(|| PathBuf::from("/etc/todo/assistant.secret"))
            })
            .ok_or(InitError::MissingKeyPath)?;
        let (vault, records) = Vault::open(dir, &key)?;
        Ok(Some(Arc::new(Self {
            openai,
            private,
            device_enabled,
            vault: Arc::new(vault),
            records: Arc::new(Mutex::new(records)),
            attempts: Mutex::new(HashMap::new()),
            start_gate: Mutex::new(()),
            account_gates: Mutex::new(HashMap::new()),
            refreshing: Mutex::new(HashMap::new()),
            revocation_gate: Mutex::new(()),
            starts: KeyedLimiter::new(4, 3),
            inference: KeyedLimiter::new(30, 12),
            slots: Arc::new(Semaphore::new(4)),
        })))
    }

    async fn status(&self) -> Status {
        let records = self.records.lock().await;
        Status {
            chatgpt: self.openai.is_some(),
            chatgpt_device_code: self.device_enabled,
            private_models: self.private.is_some(),
            accounts: if self.openai.is_some() {
                records
                    .accounts
                    .iter()
                    .map(|(id, a)| a.public(*id))
                    .collect()
            } else {
                Vec::new()
            },
            access: records.access.clone(),
            access_version: records.version,
            disabled_reason: None,
        }
    }

    async fn commit(
        &self,
        mut before: OwnedMutexGuard<Records>,
        after: Records,
    ) -> Result<(), Error> {
        let vault = self.vault.clone();
        // The commit outlives cancellation after the rename. The guard moves with it, so no later
        // request can observe stale in-memory records while the blocking commit is in progress.
        tokio::task::spawn_blocking(move || {
            vault.save(&after).map_err(|_| Error::Storage)?;
            *before = after;
            Ok(())
        })
        .await
        .map_err(|_| Error::Storage)?
    }

    async fn account_gate(&self, id: Uuid) -> OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self.account_gates.lock().await;
            gates.retain(|_, gate| gate.strong_count() != 0);
            if let Some(gate) = gates.get(&id).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                gates.insert(id, Arc::downgrade(&gate));
                gate
            }
        };
        gate.lock_owned().await
    }

    async fn insert(&self, mut auth: Credential) -> Result<Account, Error> {
        let mut account_gate = None;
        loop {
            let guard = self.records.clone().lock_owned().await;
            let previous = guard.accounts.iter().find(|(_, a)| {
                a.flow == auth.flow
                    && a.subject == auth.subject
                    && a.workspace == auth.workspace
                    && a.client_id == auth.client_id
            });
            let id = match previous {
                Some((&id, old)) if account_gate.as_ref().is_some_and(|(held, _)| *held == id) => {
                    auth.approved = old.approved;
                    auth.approved_lists.clone_from(&old.approved_lists);
                    id
                }
                Some((&id, _)) => {
                    drop(guard);
                    drop(account_gate.take());
                    account_gate = Some((id, self.account_gate(id).await));
                    continue;
                }
                None if guard.accounts.len() < MAX_ACCOUNTS => Uuid::new_v4(),
                None => return Err(Error::AccountsFull),
            };
            let mut records = guard.clone();
            let result = auth.public(id);
            if let Some(old) = guard.accounts.get(&id)
                && old.refresh != auth.refresh
            {
                records.queue_revocation(Revocation::from(old), openai::now());
            }
            records.accounts.insert(id, auth);
            self.commit(guard, records).await?;
            return Ok(result);
        }
    }

    async fn accept(&self, auth: Credential) -> Result<Account, Error> {
        let grant = Revocation::from(&auth);
        let result = self.insert(auth).await;
        if result.is_err() {
            self.discard(&grant).await;
        }
        result
    }

    async fn discard(&self, grant: &Revocation) {
        if let Some(openai) = &self.openai {
            let _ = openai.revoke(grant).await;
        }
    }

    async fn credential(
        self: &Arc<Self>,
        id: Uuid,
        rejected: Option<&str>,
    ) -> Result<Credential, Error> {
        fn needs_refresh(auth: &Credential, rejected: Option<&str>) -> bool {
            auth.expires_at <= openai::now().saturating_add(REFRESH_MARGIN)
                || rejected.is_some_and(|rejected| rejected == auth.access)
        }
        let mut result = {
            let mut refreshing = self.refreshing.lock().await;
            if let Some(result) = refreshing.get(&id).filter(|r| r.has_changed().is_ok()) {
                result.clone()
            } else {
                let records = self.records.lock().await;
                let auth = records.accounts.get(&id).ok_or(Error::AccountMissing)?;
                if !needs_refresh(auth, rejected) {
                    return Ok(auth.clone());
                }
                let (done, result) = watch::channel(None);
                refreshing.insert(id, result.clone());
                let service = self.clone();
                let rejected = rejected.map(|value| Zeroizing::new(value.to_owned()));
                // The upstream can rotate the token before it answers. The shared result can be
                // dropped without cancelling the detached refresh or the vault commit.
                tokio::spawn(async move {
                    let result = async {
                        let _gate = service.account_gate(id).await;
                        let auth = service
                            .records
                            .lock()
                            .await
                            .accounts
                            .get(&id)
                            .cloned()
                            .ok_or(Error::AccountMissing)?;
                        if needs_refresh(&auth, rejected.as_ref().map(|v| v.as_str())) {
                            service.refresh(id, auth).await
                        } else {
                            Ok(auth)
                        }
                    }
                    .await;
                    let mut refreshing = service.refreshing.lock().await;
                    refreshing.remove(&id);
                    done.send_replace(Some(result));
                });
                result
            }
        };
        result
            .wait_for(Option::is_some)
            .await
            .map_err(|_| Error::Storage)?
            .clone()
            .ok_or(Error::Storage)?
    }

    async fn refresh(&self, id: Uuid, mut auth: Credential) -> Result<Credential, Error> {
        let openai = self.openai.as_ref().ok_or(Error::Disabled)?;
        openai.refresh(&mut auth).await?;
        let guard = self.records.clone().lock_owned().await;
        // Consent can change while HTTP is in flight. Commit only the new credential fields.
        let current = guard.accounts.get(&id).ok_or(Error::AccountMissing)?;
        auth.approved = current.approved;
        auth.approved_lists.clone_from(&current.approved_lists);
        let mut records = guard.clone();
        records.accounts.insert(id, auth.clone());
        self.commit(guard, records).await?;
        Ok(auth)
    }

    async fn approved_credential(&self, engine: &Engine, id: Uuid) -> Result<Credential, Error> {
        let records = self.records.lock().await;
        let auth = records.accounts.get(&id).ok_or(Error::AccountMissing)?;
        let lists = records.access.allowed(&engine.state(), DataClass::Cloud);
        if !auth.approved || !lists.is_subset(&auth.approved_lists) {
            return Err(Error::Consent);
        }
        Ok(auth.clone())
    }

    async fn remove(&self, id: Uuid) -> Result<Uuid, Error> {
        let _gate = self.account_gate(id).await;
        let guard = self.records.clone().lock_owned().await;
        let mut records = guard.clone();
        let auth = records.accounts.remove(&id).ok_or(Error::AccountMissing)?;
        let job = records.queue_revocation(Revocation::from(&auth), openai::now());
        // Sign-out drops the credential and consent immediately, even if revocation must retry.
        self.commit(guard, records).await?;
        Ok(job)
    }

    async fn retry_revocation(&self, job: Uuid) -> Result<bool, Error> {
        let openai = self.openai.as_ref().ok_or(Error::Disabled)?;
        let _gate = self.revocation_gate.lock().await;
        let Some(pending) = self.records.lock().await.revocations.get(&job).cloned() else {
            return Ok(false);
        };
        let now = openai::now();
        let result = if pending.expired(now) {
            Err(RevokeError::Terminal)
        } else if pending.retry_at > now {
            return Ok(false);
        } else {
            openai.revoke(&pending.grant).await
        };
        let guard = self.records.clone().lock_owned().await;
        let mut records = guard.clone();
        // It may have been evicted while the upstream request was running. Don't reinsert it.
        if let Some(pending) = records.revocations.get_mut(&job) {
            let now = openai::now();
            let delay = (60 << pending.attempts.min(6)).min(3600);
            pending.attempts = pending.attempts.saturating_add(1);
            if matches!(result, Err(RevokeError::Retry)) && !pending.expired(now) {
                pending.retry_at = now.saturating_add(delay);
            } else {
                records.revocations.remove(&job);
            }
            self.commit(guard, records).await?;
        }
        Ok(result.is_ok())
    }

    pub(crate) async fn run_revocations(&self, mut shutdown: watch::Receiver<bool>) {
        if self.openai.is_none() {
            return;
        }
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        while !*shutdown.borrow() {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tick.tick() => {
                    let now = openai::now();
                    let jobs: Vec<_> = self.records.lock().await.revocations.iter()
                        .filter(|(_, pending)| pending.retry_at <= now || pending.expired(now))
                        .take(MAX_ACCOUNTS)
                        .map(|(&job, _)| job)
                        .collect();
                    for job in jobs {
                        if *shutdown.borrow() { return; }
                        if self.retry_revocation(job).await.is_err() {
                            tracing::warn!("could not update pending assistant revocation");
                        }
                    }
                }
            }
        }
    }

    async fn attempt(
        &self,
        id: Uuid,
        caller: &Principal,
    ) -> Option<(Instant, Arc<Mutex<AttemptState>>)> {
        let map = self.attempts.lock().await;
        map.get(&id)
            .filter(|a| &a.owner == caller && a.valid_until > Instant::now())
            .map(|a| (a.valid_until, a.state.clone()))
    }
}

/// The ordinary REST gate already checks credentials and applies per-client limits. Unlike most
/// REST routes, the relay refuses dev/static owners as well as agents. Never trust an owner actor
/// alone: pairing must match the specific bearer sent with this request.
struct Owner(Principal);
impl FromRequestParts<AppState> for Owner {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let caller = Authenticated::from_request_parts(parts, app).await?;
        match &caller.principal {
            Principal::Session { .. } => Ok(Self(caller.principal)),
            Principal::Owner { device } if app.authsvc.paired_device(device, &parts.headers) => {
                Ok(Self(caller.principal))
            }
            Principal::Agent { .. } => Err(crate::api::no_such_endpoint()),
            _ => Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "assistant_owner_required",
                "a paired owner device is required",
            )),
        }
    }
}

// Keep the owner-only route inventory and the actual registrations in one table. The router is
// merged under the authenticated REST API before its session/bearer middleware is applied.
macro_rules! assistant_routes {
    ($build:ident) => {
        $build! {
            GET, "/assistant/status", get(status);
            PUT, "/assistant/access", axum::routing::put(update_access);
            POST, "/assistant/chatgpt/start", post(start);
            POST, "/assistant/chatgpt/poll/{attempt}", post(poll);
            POST, "/assistant/chatgpt/handoff/{attempt}", post(handoff);
            DELETE, "/assistant/chatgpt/accounts/{id}", axum::routing::delete(sign_out);
            PUT, "/assistant/chatgpt/accounts/{id}/consent", axum::routing::put(consent);
            GET, "/assistant/chatgpt/accounts/{id}/models", get(chatgpt_models);
            POST, "/assistant/chatgpt/accounts/{id}/chat", post(chatgpt_chat);
            GET, "/assistant/private/models", get(private_models);
            POST, "/assistant/private/chat", post(private_chat);
        }
    };
}

/// The relay's owner-only REST routes, relative to `/api/v1`.
pub fn routes() -> Vec<(Method, &'static str)> {
    macro_rules! list {
        ($($method:ident, $path:literal, $handler:expr;)*) => { vec![$((Method::$method, $path)),*] };
    }
    assistant_routes!(list)
}

pub(crate) fn router() -> Router<AppState> {
    macro_rules! build {
        ($($method:ident, $path:literal, $handler:expr;)*) => { Router::new()$(.route($path, $handler))* };
    }
    assistant_routes!(build)
}

fn configured(app: &AppState) -> Result<&Arc<Assistant>, Error> {
    app.assistant.as_ref().ok_or(Error::Disabled)
}

async fn status(_owner: Owner, State(app): State<AppState>) -> Json<Status> {
    Json(match app.assistant {
        Some(a) => a.status().await,
        None => Status {
            disabled_reason: app.assistant_disabled_reason.map(str::to_owned),
            ..Status::default()
        },
    })
}

async fn update_access(
    _owner: Owner,
    State(app): State<AppState>,
    Json(input): Json<relay::AccessUpdate>,
) -> Result<Json<Status>, ApiError> {
    let service = configured(&app)?;
    if input.access.lists.len() + input.access.collections.len() > MAX_RULES {
        return Err(Error::BadRequest.into());
    }
    let guard = service.records.clone().lock_owned().await;
    if input.access_version != guard.version {
        return Err(Error::AccessChanged.into());
    }
    if input.access != guard.access {
        let mut next = guard.clone();
        next.access = input.access;
        let allowed = next.access.allowed(&app.engine.state(), DataClass::Cloud);
        for auth in next.accounts.values_mut() {
            auth.approved_lists.retain(|id| allowed.contains(id));
        }
        next.version = Uuid::new_v4();
        service.commit(guard, next).await?;
    } else {
        drop(guard);
    }
    Ok(Json(service.status().await))
}

async fn start(
    Owner(owner): Owner,
    State(app): State<AppState>,
    Json(input): Json<relay::SignIn>,
) -> Result<Json<relay::SignInStarted>, ApiError> {
    let relay = configured(&app)?;
    let openai = relay.openai.as_ref().ok_or(Error::Disabled)?;
    if !relay.device_enabled && matches!(input, relay::SignIn::Device) {
        return Err(Error::Disabled.into());
    }
    relay.starts.check(&()).map_err(Error::Rate)?;
    let _start = relay.start_gate.lock().await;
    {
        let mut attempts = relay.attempts.lock().await;
        let now = Instant::now();
        attempts.retain(|_, a| a.valid_until > now);
        if attempts.len() >= MAX_ATTEMPTS {
            return Err(Error::Rate(30).into());
        }
    }
    let attempt = Uuid::new_v4();
    let (state, valid_for, response) = match input {
        relay::SignIn::Device => {
            let grant = openai.start_device().await?;
            let response = relay::SignInStarted::Device {
                attempt,
                user_code: grant.user_code.clone(),
                verification_url: format!("{}/codex/device", openai.issuer),
            };
            (
                AttemptState::Device {
                    grant,
                    next: Instant::now(),
                },
                DEVICE_EXPIRY,
                response,
            )
        }
        relay::SignIn::Loopback { port } => {
            let host = relay.records.lock().await.host;
            let (grant, authorize_url) = openai.start_loopback(host, port)?;
            (
                AttemptState::Loopback(grant),
                LOOPBACK_EXPIRY,
                relay::SignInStarted::Loopback {
                    attempt,
                    authorize_url,
                },
            )
        }
    };
    let mut attempts = relay.attempts.lock().await;
    let now = Instant::now();
    attempts.insert(
        attempt,
        Attempt {
            owner,
            valid_until: now + valid_for,
            state: Arc::new(Mutex::new(state)),
        },
    );
    Ok(Json(response))
}

async fn poll(
    Owner(owner): Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
) -> Result<Json<Poll>, ApiError> {
    let service = configured(&app)?.clone();
    let Some((expiry, attempt)) = service.attempt(id, &owner).await else {
        return Ok(Json(Poll::Expired));
    };
    let Ok(state) = attempt.try_lock_owned() else {
        return Ok(Json(Poll::Pending));
    };
    // Once the token exchange starts, finish saving or revoking even if the client disconnects.
    tokio::spawn(poll_attempt(service, app, owner, expiry, state))
        .await
        .map_err(|_| Error::Storage)?
        .map(Json)
}

async fn poll_attempt(
    service: Arc<Assistant>,
    app: AppState,
    owner: Principal,
    expiry: Instant,
    mut state: OwnedMutexGuard<AttemptState>,
) -> Result<Poll, ApiError> {
    let openai = service.openai.as_ref().ok_or(Error::Disabled)?;
    let result = match &mut *state {
        AttemptState::Device { grant, next } => {
            let until = (*next).min(expiry);
            let cap = Instant::now() + POLL_WAIT;
            tokio::time::sleep_until(until.min(cap)).await;
            if Instant::now() >= expiry {
                return Ok(Poll::Expired);
            }
            if Instant::now() < *next {
                return Ok(Poll::Pending);
            }
            *next = Instant::now() + Duration::from_secs(grant.interval.clamp(1, 900));
            match openai.poll_device(grant).await {
                Ok(None) => Poll::Pending,
                Ok(Some(auth)) if app.authsvc.is_active(&owner) && Instant::now() < expiry => {
                    *state = AttemptState::Failed;
                    let account = service.accept(auth).await?;
                    *state = AttemptState::Finished(account.id);
                    Poll::SignedIn { account }
                }
                Ok(Some(auth)) => {
                    *state = AttemptState::Failed;
                    service.discard(&Revocation::from(&auth)).await;
                    Poll::Expired
                }
                Err(Error::Timeout | Error::UpstreamLimit) => return Ok(Poll::Pending),
                Err(err) => {
                    *state = AttemptState::Failed;
                    return Err(err.into());
                }
            }
        }
        AttemptState::Finished(id) => {
            service
                .records
                .lock()
                .await
                .accounts
                .get(id)
                .map_or(Poll::Expired, |a| Poll::SignedIn {
                    account: a.public(*id),
                })
        }
        AttemptState::Failed => Poll::Failed,
        AttemptState::Loopback(_) => return Err(Error::BadRequest.into()),
    };
    Ok(result)
}

async fn handoff(
    Owner(owner): Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
    Json(input): Json<relay::Handoff>,
) -> Result<Json<Account>, ApiError> {
    let service = configured(&app)?.clone();
    let (expiry, attempt) = service.attempt(id, &owner).await.ok_or(Error::BadRequest)?;
    let mut state = attempt.try_lock_owned().map_err(|_| Error::Rate(1))?;
    tokio::spawn(async move {
        let openai = service.openai.as_ref().ok_or(Error::Disabled)?;
        let AttemptState::Loopback(grant) = &*state else {
            return Err(Error::BadRequest.into());
        };
        let result = openai.handoff(grant, &input).await;
        *state = AttemptState::Failed;
        let auth = result?;
        if Instant::now() >= expiry || !app.authsvc.is_active(&owner) {
            service.discard(&Revocation::from(&auth)).await;
            return Err(Error::BadRequest.into());
        }
        let account = service.accept(auth).await?;
        *state = AttemptState::Finished(account.id);
        Ok(Json(account))
    })
    .await
    .map_err(|_| Error::Storage)?
}

async fn sign_out(
    _owner: Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
) -> Result<Json<relay::SignOut>, ApiError> {
    let service = configured(&app)?.clone();
    if service.openai.is_none() {
        return Err(Error::Disabled.into());
    }
    let job = service.remove(id).await?;
    let revoked = tokio::spawn(async move { service.retry_revocation(job).await })
        .await
        .is_ok_and(|result| result.is_ok_and(|revoked| revoked));
    Ok(Json(relay::SignOut { revoked }))
}

async fn consent(
    _owner: Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
    Json(input): Json<relay::Consent>,
) -> Result<Json<Account>, ApiError> {
    let service = configured(&app)?;
    if service.openai.is_none() {
        return Err(Error::Disabled.into());
    }
    let guard = service.records.clone().lock_owned().await;
    if input.access_version != guard.version {
        return Err(Error::AccessChanged.into());
    }
    let derived = guard.access.allowed(&app.engine.state(), DataClass::Cloud);
    if input.lists != derived {
        return Err(Error::Scope.into());
    }
    let mut records = guard.clone();
    let credential = records.accounts.get_mut(&id).ok_or(Error::AccountMissing)?;
    credential.approved = true;
    credential.approved_lists = derived;
    let account = credential.public(id);
    service.commit(guard, records).await?;
    Ok(Json(account))
}

async fn private_models(
    _owner: Owner,
    State(app): State<AppState>,
) -> Result<Json<Vec<relay::Model>>, ApiError> {
    let service = configured(&app)?;
    let upstream = service.private.as_ref().ok_or(Error::Disabled)?;
    service.inference.check(&()).map_err(Error::Rate)?;
    Ok(Json(upstream.models().await?))
}

async fn private_chat(
    _owner: Owner,
    State(app): State<AppState>,
    Json(mut input): Json<relay::Chat>,
) -> Result<Response, ApiError> {
    let service = configured(&app)?;
    let upstream = service.private.as_ref().ok_or(Error::Disabled)?;
    validate_request(&mut input, DataClass::Local)?;
    service.inference.check(&()).map_err(Error::Rate)?;
    let permit = service
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Rate(1))?;
    Ok(upstream::relay(upstream.chat(&input.request).await?, permit, true).await?)
}

async fn chatgpt_models(
    _owner: Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
) -> Result<Json<Vec<relay::Model>>, ApiError> {
    let service = configured(&app)?;
    let upstream = service.openai.as_ref().ok_or(Error::Disabled)?;
    service.inference.check(&()).map_err(Error::Rate)?;
    let auth = service.credential(id, None).await?;
    let result = match upstream.models(&auth).await {
        Err(Error::UpstreamAuthorization) => {
            let auth = service.credential(id, Some(&auth.access)).await?;
            upstream.models(&auth).await?
        }
        result => result?,
    };
    Ok(Json(result))
}

async fn chatgpt_chat(
    _owner: Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
    Json(mut input): Json<relay::Chat>,
) -> Result<Response, ApiError> {
    let service = configured(&app)?;
    let upstream = service.openai.as_ref().ok_or(Error::Disabled)?;
    validate_request(&mut input, DataClass::Cloud)?;
    service.approved_credential(&app.engine, id).await?;
    service.inference.check(&()).map_err(Error::Rate)?;
    let permit = service
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Rate(1))?;
    service.credential(id, None).await?;
    // Refresh may have taken time. Re-derive scope and check consent before sending. Never
    // replay a billed request, even on 401; rotate the credentials for the next request instead.
    let auth = service.approved_credential(&app.engine, id).await?;
    let res = upstream
        .chat(&auth, &responses_for(auth.flow, input.request)?)
        .await?;
    if res.status() == StatusCode::UNAUTHORIZED {
        let _ = service.credential(id, Some(&auth.access)).await;
    }
    Ok(upstream::relay(res, permit, false).await?)
}

fn validate_request(input: &mut relay::Chat, class: DataClass) -> Result<(), Error> {
    if input.destination != class {
        return Err(Error::Scope);
    }
    let request = input.request.as_object_mut().ok_or(Error::BadRequest)?;
    let keys: &[&str] = match class {
        DataClass::Local => &[
            "model",
            "stream",
            "messages",
            "tools",
            "parallel_tool_calls",
            "max_tokens",
        ],
        DataClass::Cloud => &[
            "model",
            "stream",
            "store",
            "instructions",
            "input",
            "tools",
            "parallel_tool_calls",
        ],
    };
    if request.keys().any(|key| !keys.contains(&key.as_str()))
        || !request
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(upstream::valid_model)
    {
        return Err(Error::BadRequest);
    }
    let valid_messages = |v: &Value| {
        v.is_array()
            && v.as_array().is_some_and(|items| {
                items.iter().all(|item| match class {
                    DataClass::Local => item
                        .get("content")
                        .is_none_or(|c| c.is_string() || c.is_null()),
                    DataClass::Cloud => {
                        (item.get("role").is_some()
                            && item.get("content").is_some_and(Value::is_string))
                            || matches!(
                                item["type"].as_str(),
                                Some("function_call" | "function_call_output")
                            )
                    }
                })
            })
    };
    let message_key = if class == DataClass::Local {
        "messages"
    } else {
        "input"
    };
    if !request.get(message_key).is_some_and(valid_messages)
        || request.get("tools").is_some_and(|tools| {
            !tools
                .as_array()
                .is_some_and(|tools| tools.iter().all(|t| t["type"] == "function"))
        })
    {
        return Err(Error::BadRequest);
    }
    if class == DataClass::Cloud {
        request.insert("store".into(), Value::Bool(false));
        request.insert("stream".into(), Value::Bool(true));
        request.insert("parallel_tool_calls".into(), Value::Bool(false));
    }
    Ok(())
}

fn responses_for(flow: Flow, mut body: Value) -> Result<Value, Error> {
    // Uses the common flat function schema on the Codex backend. SIWC only accepts functions
    // within a namespace, as in the desktop client's existing Responses adapter.
    if flow == Flow::Loopback {
        let body = body.as_object_mut().ok_or(Error::BadRequest)?;
        if let Some(tools) = body.get_mut("tools")
            && tools.as_array().is_some_and(|t| !t.is_empty())
        {
            *tools = serde_json::json!([{"type":"namespace", "name":"todo", "description":"Read tasks or prepare proposals for review.", "tools": tools}]);
        }
        if let Some(messages) = body.get_mut("input").and_then(Value::as_array_mut) {
            for message in messages {
                if message["type"] == "function_call"
                    && let Some(item) = message.as_object_mut()
                {
                    item.insert("namespace".into(), Value::String("todo".into()));
                }
            }
        }
    }
    Ok(body)
}
