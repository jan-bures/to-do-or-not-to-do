//! Owner-only inference relay. It does not submit commands or grant a model access to task APIs.

mod openai;
#[cfg(test)]
mod tests;
mod upstream;
mod vault;

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
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
use todo_core::ids::ListId;
use todo_tools::assistant::{
    access::{AccessRules, DataClass},
    relay::{self, Account, Poll, Status},
};
use tokio::{
    sync::{Mutex, OwnedMutexGuard, Semaphore},
    time::Instant,
};
use uuid::Uuid;

use crate::{
    api::error::ApiError,
    app::AppState,
    auth::{Authenticated, Principal, limits::KeyedLimiter},
    config::ConfigError,
};
use openai::{DeviceGrant, LoopbackGrant, OpenAi};
use upstream::Private;
use vault::{Credential, Flow, Records, Vault};

pub use upstream::PrivateConfig;

const MAX_ATTEMPTS: usize = 8;
const MAX_ACCOUNTS: usize = 12;
const MAX_RULES: usize = 4096;
const DEVICE_EXPIRY: Duration = Duration::from_secs(15 * 60);
const LOOPBACK_EXPIRY: Duration = Duration::from_secs(180);
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
    /// Defaults to `data_dir/assistant.key`.
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
    #[error("invalid assistant private model base URL")]
    PrivateAddress,
    #[error("could not read the assistant private model key file: {0}")]
    PrivateKey(std::io::Error),
    #[error("invalid assistant private model key")]
    BadPrivateKey,
    #[error("could not initialize the assistant HTTP client")]
    Http,
}

#[derive(Debug, thiserror::Error)]
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
    #[error("the request includes a list not allowed for this provider")]
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
            Error::Identity | Error::Upstream => (StatusCode::BAD_GATEWAY, "assistant_provider"),
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

pub(crate) struct Assistant {
    openai: Option<OpenAi>,
    private: Option<Private>,
    device_enabled: bool,
    vault: Arc<Vault>,
    records: Arc<Mutex<Records>>,
    attempts: Mutex<HashMap<Uuid, Attempt>>,
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
        let key = config.key_file.unwrap_or_else(|| dir.join("assistant.key"));
        let (vault, records) = Vault::open(dir, &key)?;
        Ok(Some(Arc::new(Self {
            openai,
            private,
            device_enabled,
            vault: Arc::new(vault),
            records: Arc::new(Mutex::new(records)),
            attempts: Mutex::new(HashMap::new()),
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

    async fn insert(&self, auth: Credential) -> Result<Account, Error> {
        let guard = self.records.clone().lock_owned().await;
        let mut records = guard.clone();
        let previous = records.accounts.iter().find(|(_, a)| {
            a.flow == auth.flow
                && a.subject == auth.subject
                && a.workspace == auth.workspace
                && a.client_id == auth.client_id
        });
        let (id, auth) = match previous {
            Some((&id, old)) => {
                let mut auth = auth;
                auth.approved = old.approved;
                auth.approved_lists.clone_from(&old.approved_lists);
                (id, auth)
            }
            None if records.accounts.len() < MAX_ACCOUNTS => (Uuid::new_v4(), auth),
            None => return Err(Error::AccountsFull),
        };
        let result = auth.public(id);
        records.accounts.insert(id, auth);
        self.commit(guard, records).await?;
        Ok(result)
    }

    async fn credential(
        &self,
        id: Uuid,
        lists: Option<&BTreeSet<ListId>>,
        rejected: Option<&str>,
    ) -> Result<Credential, Error> {
        let openai = self.openai.as_ref().ok_or(Error::Disabled)?;
        let guard = self.records.clone().lock_owned().await;
        let auth = guard.accounts.get(&id).ok_or(Error::AccountMissing)?;
        if lists.is_some_and(|lists| !auth.approved || !lists.is_subset(&auth.approved_lists)) {
            return Err(Error::Consent);
        }
        let needs_refresh = auth.expires_at <= openai::now().saturating_add(REFRESH_MARGIN)
            || rejected.is_some_and(|rejected| rejected == auth.access);
        if !needs_refresh {
            return Ok(auth.clone());
        }
        let mut auth = auth.clone();
        openai.refresh(&mut auth).await?;
        let mut records = guard.clone();
        records.accounts.insert(id, auth.clone());
        self.commit(guard, records).await?;
        Ok(auth)
    }

    async fn remove(&self, id: Uuid) -> Result<Credential, Error> {
        let guard = self.records.clone().lock_owned().await;
        let mut records = guard.clone();
        let auth = records.accounts.remove(&id).ok_or(Error::AccountMissing)?;
        // Removing the credential and consent durably is mandatory even if remote revocation fails.
        self.commit(guard, records).await?;
        Ok(auth)
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

fn configured(app: &AppState) -> Result<&Assistant, Error> {
    app.assistant.as_deref().ok_or(Error::Disabled)
}

async fn status(_owner: Owner, State(app): State<AppState>) -> Json<Status> {
    Json(match app.assistant {
        Some(a) => a.status().await,
        None => Status::default(),
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
    attempts.retain(|_, a| a.valid_until > now);
    if attempts.len() >= MAX_ATTEMPTS {
        return Err(Error::Rate(30).into());
    }
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
    let service = configured(&app)?;
    let openai = service.openai.as_ref().ok_or(Error::Disabled)?;
    let Some((expiry, attempt)) = service.attempt(id, &owner).await else {
        return Ok(Json(Poll::Expired));
    };
    let Ok(mut state) = attempt.try_lock() else {
        return Ok(Json(Poll::Pending));
    };
    let result = match &mut *state {
        AttemptState::Device { grant, next } => {
            let until = (*next).min(expiry);
            let cap = Instant::now() + POLL_WAIT;
            tokio::time::sleep_until(until.min(cap)).await;
            if Instant::now() >= expiry {
                return Ok(Json(Poll::Expired));
            }
            if Instant::now() < *next {
                return Ok(Json(Poll::Pending));
            }
            *next = Instant::now() + Duration::from_secs(grant.interval.clamp(1, 900));
            match openai.poll_device(grant).await {
                Ok(None) => Poll::Pending,
                Ok(Some(auth)) if app.authsvc.is_active(&owner) && Instant::now() < expiry => {
                    *state = AttemptState::Failed;
                    let account = service.insert(auth).await?;
                    *state = AttemptState::Finished(account.id);
                    Poll::SignedIn { account }
                }
                Ok(Some(_)) => {
                    *state = AttemptState::Failed;
                    Poll::Expired
                }
                Err(Error::Timeout | Error::UpstreamLimit) => return Ok(Json(Poll::Pending)),
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
    Ok(Json(result))
}

async fn handoff(
    Owner(owner): Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
    Json(input): Json<relay::Handoff>,
) -> Result<Json<Account>, ApiError> {
    let service = configured(&app)?;
    let openai = service.openai.as_ref().ok_or(Error::Disabled)?;
    let (expiry, attempt) = service.attempt(id, &owner).await.ok_or(Error::BadRequest)?;
    let mut state = attempt.try_lock().map_err(|_| Error::Rate(1))?;
    let AttemptState::Loopback(grant) = std::mem::replace(&mut *state, AttemptState::Failed) else {
        return Err(Error::BadRequest.into());
    };
    let auth = openai.handoff(&grant, &input).await?;
    if Instant::now() >= expiry || !app.authsvc.is_active(&owner) {
        return Err(Error::BadRequest.into());
    }
    let account = service.insert(auth).await?;
    *state = AttemptState::Finished(account.id);
    Ok(Json(account))
}

async fn sign_out(
    _owner: Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
) -> Result<Json<relay::SignOut>, ApiError> {
    let service = configured(&app)?;
    let openai = service.openai.as_ref().ok_or(Error::Disabled)?;
    let auth = service.remove(id).await?;
    Ok(Json(relay::SignOut {
        revoked: openai.revoke(&auth).await.is_ok(),
    }))
}

fn check_scope(
    state: &todo_core::State,
    access: &AccessRules,
    lists: &BTreeSet<ListId>,
    class: DataClass,
) -> Result<(), Error> {
    if lists.len() > MAX_RULES
        || lists
            .iter()
            .any(|&id| !access.list(state, id).allows(class))
    {
        return Err(Error::Scope);
    }
    Ok(())
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
    check_scope(
        &app.engine.state(),
        &guard.access,
        &input.lists,
        DataClass::Cloud,
    )?;
    let mut records = guard.clone();
    let credential = records.accounts.get_mut(&id).ok_or(Error::AccountMissing)?;
    credential.approved = true;
    credential.approved_lists = input.lists;
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
    check_scope(
        &app.engine.state(),
        &service.records.lock().await.access,
        &input.lists,
        DataClass::Local,
    )?;
    service.inference.check(&()).map_err(Error::Rate)?;
    let permit = service
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Rate(1))?;
    Ok(upstream::relay(
        upstream.chat(&input.request).await?,
        permit,
        true,
    )?)
}

async fn chatgpt_models(
    _owner: Owner,
    State(app): State<AppState>,
    Route(id): Route<Uuid>,
) -> Result<Json<Vec<relay::Model>>, ApiError> {
    let service = configured(&app)?;
    let upstream = service.openai.as_ref().ok_or(Error::Disabled)?;
    service.inference.check(&()).map_err(Error::Rate)?;
    let auth = service.credential(id, None, None).await?;
    let result = match upstream.models(&auth).await {
        Err(Error::UpstreamAuthorization) => {
            let auth = service.credential(id, None, Some(&auth.access)).await?;
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
    check_scope(
        &app.engine.state(),
        &service.records.lock().await.access,
        &input.lists,
        DataClass::Cloud,
    )?;
    service.inference.check(&()).map_err(Error::Rate)?;
    let permit = service
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Rate(1))?;
    let auth = service.credential(id, Some(&input.lists), None).await?;
    // Refresh may have taken time. Recheck list visibility before sending. No retry after sending a
    // billed request, even on 401; a later explicit request may reconnect or refresh.
    check_scope(
        &app.engine.state(),
        &service.records.lock().await.access,
        &input.lists,
        DataClass::Cloud,
    )?;
    let res = upstream
        .chat(&auth, &responses_for(auth.flow, input.request)?)
        .await?;
    if res.status() == StatusCode::UNAUTHORIZED {
        let _ = service.credential(id, None, Some(&auth.access)).await;
    }
    Ok(upstream::relay(res, permit, false)?)
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
