//! The admin API's handlers (JSON in, JSON out). Every change is audited with the actor
//! [`ADMIN_API_ACTOR`]; reads change nothing. No handler returns task content or a secret.

use super::{AdminCaller, AdminState};
use crate::api::error::{ApiError, ErrorBody};
use crate::api::{ApiJson, ApiPath, ApiQuery};
use crate::auth::service::{ADMIN_API_ACTOR, Who};
use crate::auth::session::blocking;
use crate::auth::store::{AgentRow, AuditRow, DeviceRow, ShareRow, ShareTarget};
use crate::ingest::normalize_address;
use crate::ingest::store::{By, KeyError, SenderRow};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use todo_core::ids::{CollectionId, ListId};
use todo_core::{AccessLevel, Grants, State as CoreState};
use utoipa::{IntoParams, ToSchema};

fn invalid(msg: &str) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", msg)
}

fn by(st: &AdminState, caller: AdminCaller) -> By {
    By {
        at: st.app.authsvc.now(),
        ip: caller.ip,
        actor: Some(ADMIN_API_ACTOR),
    }
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// `Collection / List` for a live list, or `None` if it is gone.
fn list_name(state: &CoreState, list: ListId) -> Option<String> {
    let l = state
        .lists
        .get(&list)
        .filter(|_| state.is_live(list.into()))?;
    Some(match l.collection.and_then(|c| state.collections.get(&c)) {
        Some(c) => format!("{} / {}", c.name, l.name),
        None => l.name.clone(),
    })
}

/// A live list from a request, or 422.
fn live_list(state: &CoreState, raw: &str) -> Result<ListId, ApiError> {
    raw.trim()
        .parse::<ListId>()
        .ok()
        .filter(|l| list_name(state, *l).is_some())
        .ok_or_else(|| invalid("unknown list"))
}

// ---- lists and collections ------------------------------------------------------------------------

/// A collection: ID and name only.
#[derive(Debug, Serialize, ToSchema)]
pub struct CollectionName {
    /// ID.
    #[schema(format = Uuid)]
    pub id: String,
    /// Name.
    pub name: String,
}

/// A list: ID, name and collection only.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListName {
    /// ID.
    #[schema(format = Uuid)]
    pub id: String,
    /// Name.
    pub name: String,
    /// Its collection, if any.
    #[schema(format = Uuid)]
    pub collection: Option<String>,
}

/// The live lists and collections, in display order: names only, never tasks.
#[derive(Debug, Serialize, ToSchema)]
pub struct Structure {
    /// Collections.
    pub collections: Vec<CollectionName>,
    /// Lists.
    pub lists: Vec<ListName>,
}

/// Lists and collections, to pick an Inbox, a key's list or a grant's target.
#[utoipa::path(
    get, path = "/api/v1/admin/lists", tag = "admin",
    responses((status = 200, body = Structure), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn lists(State(st): State<AdminState>, _c: AdminCaller) -> Json<Structure> {
    let state = st.app.engine.state();
    let (cols, lists) = crate::web::pages::sorted_structure(&state);
    Json(Structure {
        collections: cols
            .into_iter()
            .map(|(id, name)| CollectionName {
                id: id.to_string(),
                name,
            })
            .collect(),
        lists: lists
            .into_iter()
            .map(|(id, c, name)| ListName {
                id: id.to_string(),
                name,
                collection: c.map(|c| c.to_string()),
            })
            .collect(),
    })
}

// ---- email ingest ---------------------------------------------------------------------------------

/// A list a setting points at. `name` is `null` once the list was deleted.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListRef {
    /// The list ID.
    #[schema(format = Uuid)]
    pub id: String,
    /// `Collection / List`, or `null` if the list was deleted.
    pub name: Option<String>,
}

fn list_ref(state: &CoreState, list: ListId) -> ListRef {
    ListRef {
        id: list.to_string(),
        name: list_name(state, list),
    }
}

/// An allowed sender.
#[derive(Debug, Serialize, ToSchema)]
pub struct Sender {
    /// The normalized address.
    pub address: String,
    /// When it was added (Unix milliseconds).
    pub added_at: i64,
    /// `config` (seeded from the config) or `admin`.
    pub source: String,
}

impl From<SenderRow> for Sender {
    fn from(r: SenderRow) -> Self {
        Self {
            address: r.address,
            added_at: r.added_at,
            source: r.source,
        }
    }
}

/// A list key (`todo+<key>@`).
#[derive(Debug, Serialize, ToSchema)]
pub struct ListKey {
    /// The key.
    pub key: String,
    /// The list it names.
    pub list: ListRef,
}

/// Email ingest settings.
#[derive(Debug, Serialize, ToSchema)]
pub struct IngestSettings {
    /// Whether the ingest route is on (an HMAC secret is configured).
    pub enabled: bool,
    /// The Inbox, if set. Without a live Inbox, email is refused.
    pub inbox: Option<ListRef>,
    /// The sender allowlist.
    pub senders: Vec<Sender>,
    /// The list keys.
    pub keys: Vec<ListKey>,
}

/// The allowlist.
#[derive(Debug, Serialize, ToSchema)]
pub struct SenderPage {
    /// Allowed senders, by address.
    pub senders: Vec<Sender>,
}

/// The list keys.
#[derive(Debug, Serialize, ToSchema)]
pub struct KeyPage {
    /// Keys, by key.
    pub keys: Vec<ListKey>,
}

/// Email ingest settings: on or off, the Inbox, the allowlist and the list keys.
#[utoipa::path(
    get, path = "/api/v1/admin/ingest", tag = "admin",
    responses((status = 200, body = IngestSettings), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn ingest(
    State(st): State<AdminState>,
    _c: AdminCaller,
) -> Result<Json<IngestSettings>, ApiError> {
    let ingest = st.app.ingest.clone();
    let (senders, keys, inbox) = blocking(&st.app.authsvc, move |_| {
        Ok::<_, crate::db::StoreError>((
            ingest.store.senders()?,
            ingest.store.list_keys()?,
            ingest.store.inbox()?,
        ))
    })
    .await??;
    let state = st.app.engine.state();
    Ok(Json(IngestSettings {
        enabled: st.app.ingest.enabled(),
        inbox: inbox.map(|l| list_ref(&state, l)),
        senders: senders.into_iter().map(Sender::from).collect(),
        keys: keys
            .into_iter()
            .map(|(key, l)| ListKey {
                key,
                list: list_ref(&state, l),
            })
            .collect(),
    }))
}

/// Sets or unsets the Inbox.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InboxRequest {
    /// A live list, or `null` to unset the Inbox (email is then refused).
    #[schema(format = Uuid)]
    pub list: Option<String>,
}

/// The Inbox after a change.
#[derive(Debug, Serialize, ToSchema)]
pub struct InboxResponse {
    /// The Inbox, if set.
    pub inbox: Option<ListRef>,
    /// False if it already was this.
    pub changed: bool,
}

/// Sets (or with `null` unsets) the Inbox. Audited as `ingest_inbox_set`.
#[utoipa::path(
    put, path = "/api/v1/admin/ingest/inbox", tag = "admin", request_body = InboxRequest,
    responses(
        (status = 200, body = InboxResponse),
        (status = 401, body = ErrorBody),
        (status = 422, description = "Not a live list (`invalid`)", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn inbox_set(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiJson(r): ApiJson<InboxRequest>,
) -> Result<Json<InboxResponse>, ApiError> {
    let state = st.app.engine.state();
    let list = r
        .list
        .as_deref()
        .map(|l| live_list(&state, l))
        .transpose()?;
    let (ingest, by) = (st.app.ingest.clone(), by(&st, c));
    let changed = blocking(&st.app.authsvc, move |_| ingest.store.set_inbox(list, by)).await??;
    Ok(Json(InboxResponse {
        inbox: list.map(|l| list_ref(&state, l)),
        changed,
    }))
}

/// The sender allowlist.
#[utoipa::path(
    get, path = "/api/v1/admin/ingest/senders", tag = "admin",
    responses((status = 200, body = SenderPage), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn senders(
    State(st): State<AdminState>,
    _c: AdminCaller,
) -> Result<Json<SenderPage>, ApiError> {
    let ingest = st.app.ingest.clone();
    let rows = blocking(&st.app.authsvc, move |_| ingest.store.senders()).await??;
    Ok(Json(SenderPage {
        senders: rows.into_iter().map(Sender::from).collect(),
    }))
}

/// A sender to allow.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SenderRequest {
    /// An email address (normalized to lowercase).
    pub address: String,
}

/// The result of adding a sender.
#[derive(Debug, Serialize, ToSchema)]
pub struct SenderAdded {
    /// The normalized address.
    pub address: String,
    /// False if it was allowed already.
    pub added: bool,
}

/// Allows a sender. 201 when added, 200 when it was there already. Audited as `ingest_sender_added`.
/// Remember to update the Email Worker's own `ALLOWED_SENDERS` too.
#[utoipa::path(
    post, path = "/api/v1/admin/ingest/senders", tag = "admin", request_body = SenderRequest,
    responses(
        (status = 201, body = SenderAdded),
        (status = 200, description = "Allowed already", body = SenderAdded),
        (status = 401, body = ErrorBody),
        (status = 422, description = "Not an email address (`invalid`)", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn sender_add(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiJson(r): ApiJson<SenderRequest>,
) -> Result<Response, ApiError> {
    let address = normalize_address(&r.address).ok_or_else(|| invalid("not an email address"))?;
    let (ingest, by, a) = (st.app.ingest.clone(), by(&st, c), address.clone());
    let added = blocking(&st.app.authsvc, move |_| ingest.store.add_sender(&a, by)).await??;
    let status = if added {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(SenderAdded { address, added })).into_response())
}

/// Removes a sender from the allowlist. Audited as `ingest_sender_removed`.
#[utoipa::path(
    delete, path = "/api/v1/admin/ingest/senders/{address}", tag = "admin",
    params(("address" = String, Path, description = "The address (percent-encoded)")),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, body = ErrorBody),
        (status = 404, description = "Not on the allowlist", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn sender_remove(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiPath(address): ApiPath<String>,
) -> Result<Response, ApiError> {
    let address =
        normalize_address(&address).ok_or_else(|| ApiError::not_found("no such sender"))?;
    let (ingest, by) = (st.app.ingest.clone(), by(&st, c));
    let removed = blocking(&st.app.authsvc, move |_| {
        ingest.store.remove_sender(&address, by)
    })
    .await??;
    if removed {
        Ok(no_content())
    } else {
        Err(ApiError::not_found("no such sender"))
    }
}

/// The list keys.
#[utoipa::path(
    get, path = "/api/v1/admin/ingest/keys", tag = "admin",
    responses((status = 200, body = KeyPage), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn keys(
    State(st): State<AdminState>,
    _c: AdminCaller,
) -> Result<Json<KeyPage>, ApiError> {
    let ingest = st.app.ingest.clone();
    let rows = blocking(&st.app.authsvc, move |_| ingest.store.list_keys()).await??;
    let state = st.app.engine.state();
    Ok(Json(KeyPage {
        keys: rows
            .into_iter()
            .map(|(key, l)| ListKey {
                key,
                list: list_ref(&state, l),
            })
            .collect(),
    }))
}

/// A list key to set.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyRequest {
    /// `[a-z0-9][a-z0-9_-]{0,63}`; `inbox` is reserved.
    pub key: String,
    /// A live list.
    #[schema(format = Uuid)]
    pub list: String,
}

/// Makes `todo+<key>@` deliver to a list. Audited as `list_key_added`.
#[utoipa::path(
    post, path = "/api/v1/admin/ingest/keys", tag = "admin", request_body = KeyRequest,
    responses(
        (status = 201, body = ListKey),
        (status = 401, body = ErrorBody),
        (status = 409, description = "The key is set already (`key_taken`); remove it first", body = ErrorBody),
        (status = 422, description = "A malformed or reserved key, or not a live list (`invalid`)", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn key_add(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiJson(r): ApiJson<KeyRequest>,
) -> Result<Response, ApiError> {
    let state = st.app.engine.state();
    let list = live_list(&state, &r.list)?;
    let key = r.key.trim().to_owned();
    let (ingest, by, k) = (st.app.ingest.clone(), by(&st, c), key.clone());
    let added = blocking(&st.app.authsvc, move |_| {
        ingest.store.add_list_key(&k, list, by)
    })
    .await??;
    match added {
        Ok(()) => Ok((
            StatusCode::CREATED,
            Json(ListKey {
                key,
                list: list_ref(&state, list),
            }),
        )
            .into_response()),
        Err(KeyError::Invalid) => Err(invalid(
            "a key must be 1 to 64 lowercase letters, digits, - or _, starting with a letter or digit",
        )),
        Err(KeyError::Reserved) => Err(invalid("the key inbox always means the Inbox")),
        Err(KeyError::Taken) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "key_taken",
            "that key is set already; remove it first to point it at another list",
        )),
    }
}

/// Removes a list key (its mail then goes to the Inbox). Audited as `list_key_removed`.
#[utoipa::path(
    delete, path = "/api/v1/admin/ingest/keys/{key}", tag = "admin",
    params(("key" = String, Path, description = "The key")),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, body = ErrorBody),
        (status = 404, description = "No such key", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn key_remove(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiPath(key): ApiPath<String>,
) -> Result<Response, ApiError> {
    let (ingest, by) = (st.app.ingest.clone(), by(&st, c));
    let removed = blocking(&st.app.authsvc, move |_| {
        ingest.store.remove_list_key(&key, by)
    })
    .await??;
    match removed {
        Some(_) => Ok(no_content()),
        None => Err(ApiError::not_found("no such key")),
    }
}

// ---- agents ---------------------------------------------------------------------------------------

/// An agent's access levels. Keys of `collections` and `lists` are IDs; anything not named inherits
/// (a list from its collection, a collection from `default`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantsBody {
    /// The level wherever no override applies.
    pub default: AccessLevel,
    /// Per-collection levels, by collection ID.
    #[serde(default)]
    pub collections: BTreeMap<String, AccessLevel>,
    /// Per-list levels, by list ID (they beat the collection's).
    #[serde(default)]
    pub lists: BTreeMap<String, AccessLevel>,
}

impl From<&Grants> for GrantsBody {
    fn from(g: &Grants) -> Self {
        Self {
            default: g.default,
            collections: g
                .collections
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
            lists: g.lists.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }
}

impl GrantsBody {
    /// The grants, checking that every collection and list exists.
    fn to_grants(&self, state: &CoreState) -> Result<Grants, ApiError> {
        let mut g = Grants::with_default(self.default);
        for (k, v) in &self.collections {
            let c: CollectionId = k
                .parse()
                .ok()
                .filter(|c| state.collections.contains_key(c))
                .ok_or_else(|| invalid("unknown collection"))?;
            g.collections.insert(c, *v);
        }
        for (k, v) in &self.lists {
            let l: ListId = k
                .parse()
                .ok()
                .filter(|l| state.lists.contains_key(l))
                .ok_or_else(|| invalid("unknown list"))?;
            g.lists.insert(l, *v);
        }
        Ok(g)
    }
}

/// An agent token, without its secret (the server keeps only a hash).
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentInfo {
    /// The token's ID (use it in the other agent routes).
    pub id: i64,
    /// The agent's name; its changes are recorded as `agent:<name>`.
    pub agent: String,
    /// `active`, `expired` or `revoked`.
    pub status: &'static str,
    /// Access levels.
    pub grants: GrantsBody,
    /// Write quota (commands and undos per rolling hour).
    pub writes_per_hour: u32,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds, if it expires.
    pub expires_at: Option<i64>,
    /// Unix milliseconds (recorded at most once a minute).
    pub last_used_at: Option<i64>,
    /// Unix milliseconds, once revoked.
    pub revoked_at: Option<i64>,
}

fn agent_info(a: &AgentRow, now: i64) -> AgentInfo {
    let status = if a.revoked_at.is_some() {
        "revoked"
    } else if a.expires_at.is_some_and(|x| x <= now) {
        "expired"
    } else {
        "active"
    };
    AgentInfo {
        id: a.id,
        agent: a.agent.as_str().to_owned(),
        status,
        grants: GrantsBody::from(&a.grants),
        writes_per_hour: a.writes_per_hour,
        created_at: a.created_at,
        expires_at: a.expires_at,
        last_used_at: a.last_used_at,
        revoked_at: a.revoked_at,
    }
}

/// Agent tokens.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentPage {
    /// Newest first, revoked ones included.
    pub agents: Vec<AgentInfo>,
}

/// Every agent token, newest first (no secrets).
#[utoipa::path(
    get, path = "/api/v1/admin/agents", tag = "admin",
    responses((status = 200, body = AgentPage), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn agents(
    State(st): State<AdminState>,
    _c: AdminCaller,
) -> Result<Json<AgentPage>, ApiError> {
    let rows = blocking(&st.app.authsvc, |svc| svc.agents()).await??;
    let now = st.app.authsvc.now();
    Ok(Json(AgentPage {
        agents: rows.iter().map(|a| agent_info(a, now)).collect(),
    }))
}

/// One agent token.
#[utoipa::path(
    get, path = "/api/v1/admin/agents/{id}", tag = "admin",
    params(("id" = i64, Path, description = "The token's ID")),
    responses(
        (status = 200, body = AgentInfo),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn agent(
    State(st): State<AdminState>,
    _c: AdminCaller,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<AgentInfo>, ApiError> {
    let row = blocking(&st.app.authsvc, move |svc| svc.store().agent(id)).await??;
    let row = row.ok_or_else(|| ApiError::not_found("no such agent token"))?;
    Ok(Json(agent_info(&row, st.app.authsvc.now())))
}

/// An agent's new access: grants and quota, both replaced.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessRequest {
    /// The new grants (replacing the old ones entirely).
    pub grants: GrantsBody,
    /// The new write quota per rolling hour (at least 1).
    pub writes_per_hour: u32,
}

/// Replaces an active agent's grants and write quota. Takes effect on its next request; its event
/// streams close and reconnect under the new grants. Audited as `grants_changed`.
#[utoipa::path(
    put, path = "/api/v1/admin/agents/{id}/access", tag = "admin",
    params(("id" = i64, Path, description = "The token's ID")),
    request_body = AccessRequest,
    responses(
        (status = 200, body = AgentInfo),
        (status = 401, body = ErrorBody),
        (status = 404, description = "No such active token", body = ErrorBody),
        (status = 422, description = "An unknown list or collection, or a zero quota (`invalid`)", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn agent_access(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiPath(id): ApiPath<i64>,
    ApiJson(r): ApiJson<AccessRequest>,
) -> Result<Json<AgentInfo>, ApiError> {
    if r.writes_per_hour == 0 {
        return Err(invalid("writes_per_hour must be at least 1"));
    }
    let grants = r.grants.to_grants(&st.app.engine.state())?;
    let wph = r.writes_per_hour;
    let who = Who::admin_api(c.ip);
    let row = blocking(&st.app.authsvc, move |svc| {
        if !svc.set_agent_access(id, &grants, wph, who)? {
            return Ok(None);
        }
        svc.store().agent(id).map_err(ApiError::from)
    })
    .await??;
    let row = row.ok_or_else(|| ApiError::not_found("no such active agent token"))?;
    Ok(Json(agent_info(&row, st.app.authsvc.now())))
}

/// Revokes an agent token at once. Audited as `agent_token_revoked`.
#[utoipa::path(
    post, path = "/api/v1/admin/agents/{id}/revoke", tag = "admin",
    params(("id" = i64, Path, description = "The token's ID")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, body = ErrorBody),
        (status = 404, description = "No such active token", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn agent_revoke(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiPath(id): ApiPath<i64>,
) -> Result<Response, ApiError> {
    let who = Who::admin_api(c.ip);
    if blocking(&st.app.authsvc, move |svc| svc.revoke_agent(id, who)).await?? {
        Ok(no_content())
    } else {
        Err(ApiError::not_found("no such active agent token"))
    }
}

// ---- devices --------------------------------------------------------------------------------------

/// A paired device, without its token.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeviceInfo {
    /// The device ID (the actor of its events).
    pub id: String,
    /// The name it gave at pairing.
    pub name: String,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds (recorded at most once a minute).
    pub last_used_at: Option<i64>,
    /// Unix milliseconds, once revoked.
    pub revoked_at: Option<i64>,
}

impl From<DeviceRow> for DeviceInfo {
    fn from(d: DeviceRow) -> Self {
        Self {
            id: d.id.as_str().to_owned(),
            name: d.name,
            created_at: d.created_at,
            last_used_at: d.last_used_at,
            revoked_at: d.revoked_at,
        }
    }
}

/// Paired devices.
#[derive(Debug, Serialize, ToSchema)]
pub struct DevicePage {
    /// Newest first, revoked ones included.
    pub devices: Vec<DeviceInfo>,
}

/// Every paired device, newest first.
#[utoipa::path(
    get, path = "/api/v1/admin/devices", tag = "admin",
    responses((status = 200, body = DevicePage), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn devices(
    State(st): State<AdminState>,
    _c: AdminCaller,
) -> Result<Json<DevicePage>, ApiError> {
    let rows = blocking(&st.app.authsvc, |svc| svc.store().devices()).await??;
    Ok(Json(DevicePage {
        devices: rows.into_iter().map(DeviceInfo::from).collect(),
    }))
}

/// Revokes a device: its token stops working and its event streams close. Audited as
/// `device_revoked`.
#[utoipa::path(
    post, path = "/api/v1/admin/devices/{id}/revoke", tag = "admin",
    params(("id" = String, Path, description = "The device ID")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, body = ErrorBody),
        (status = 404, description = "No such active device", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn device_revoke(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiPath(id): ApiPath<String>,
) -> Result<Response, ApiError> {
    let who = Who::admin_api(c.ip);
    if blocking(&st.app.authsvc, move |svc| svc.revoke_device(&id, who)).await?? {
        Ok(no_content())
    } else {
        Err(ApiError::not_found("no such active device"))
    }
}

// ---- share links ----------------------------------------------------------------------------------

/// What a share link shows.
#[derive(Debug, Serialize, ToSchema)]
pub struct ShareTargetInfo {
    /// `list` or `collection`.
    pub kind: &'static str,
    /// Its ID.
    #[schema(format = Uuid)]
    pub id: String,
    /// Its name, or `null` if it was deleted.
    pub name: Option<String>,
}

/// A share link, without its token.
#[derive(Debug, Serialize, ToSchema)]
pub struct ShareInfo {
    /// The link's ID.
    pub id: i64,
    /// What it shows.
    pub target: ShareTargetInfo,
    /// Unix milliseconds.
    pub created_at: i64,
    /// Unix milliseconds of the last rotation.
    pub rotated_at: Option<i64>,
    /// Unix milliseconds, once revoked.
    pub revoked_at: Option<i64>,
}

fn share_info(state: &CoreState, r: &ShareRow) -> ShareInfo {
    let target = match r.target {
        ShareTarget::List(l) => ShareTargetInfo {
            kind: "list",
            id: l.to_string(),
            name: state.lists.get(&l).map(|l| l.name.clone()),
        },
        ShareTarget::Collection(c) => ShareTargetInfo {
            kind: "collection",
            id: c.to_string(),
            name: state.collections.get(&c).map(|c| c.name.clone()),
        },
    };
    ShareInfo {
        id: r.id,
        target,
        created_at: r.created_at,
        rotated_at: r.rotated_at,
        revoked_at: r.revoked_at,
    }
}

/// Share links.
#[derive(Debug, Serialize, ToSchema)]
pub struct SharePage {
    /// Newest first, revoked ones included.
    pub shares: Vec<ShareInfo>,
}

/// Every share link, newest first (no tokens: the server keeps only hashes).
#[utoipa::path(
    get, path = "/api/v1/admin/shares", tag = "admin",
    responses((status = 200, body = SharePage), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn shares(
    State(st): State<AdminState>,
    _c: AdminCaller,
) -> Result<Json<SharePage>, ApiError> {
    let rows = blocking(&st.app.authsvc, |svc| svc.store().shares()).await??;
    let state = st.app.engine.state();
    Ok(Json(SharePage {
        shares: rows.iter().map(|r| share_info(&state, r)).collect(),
    }))
}

/// Revokes a share link. Audited as `share_revoked`.
#[utoipa::path(
    post, path = "/api/v1/admin/shares/{id}/revoke", tag = "admin",
    params(("id" = i64, Path, description = "The link's ID")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, body = ErrorBody),
        (status = 404, description = "No such active link", body = ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn share_revoke(
    State(st): State<AdminState>,
    c: AdminCaller,
    ApiPath(id): ApiPath<i64>,
) -> Result<Response, ApiError> {
    let who = Who::admin_api(c.ip);
    if blocking(&st.app.authsvc, move |svc| svc.revoke_share(id, who)).await?? {
        Ok(no_content())
    } else {
        Err(ApiError::not_found("no such active share link"))
    }
}

// ---- audit ----------------------------------------------------------------------------------------

/// Most audit entries per page.
pub const MAX_AUDIT_PAGE: usize = 500;
/// Audit entries per page by default.
pub const DEFAULT_AUDIT_PAGE: usize = 100;

/// Audit paging.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct AuditParams {
    /// Only entries older than this entry ID (the previous page's `next_before`).
    pub before: Option<i64>,
    /// Entries per page (default 100, at most 500).
    pub limit: Option<usize>,
}

/// One audit entry.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditEntry {
    /// Entry ID (increasing).
    pub id: i64,
    /// Unix milliseconds.
    pub at: i64,
    /// What happened, for example `sign_in` or `ingest_sender_added`.
    pub kind: String,
    /// The client IP, if known.
    pub ip: Option<String>,
    /// Details (never secrets). Changes made through this API have `"actor": "admin API"`.
    #[schema(value_type = Object)]
    pub detail: Value,
}

impl From<AuditRow> for AuditEntry {
    fn from(r: AuditRow) -> Self {
        Self {
            id: r.id,
            at: r.at,
            kind: r.kind,
            ip: r.ip,
            detail: serde_json::from_str(&r.detail).unwrap_or(Value::String(r.detail)),
        }
    }
}

/// A page of the audit log.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditPage {
    /// Newest first.
    pub entries: Vec<AuditEntry>,
    /// Pass as `before` for the next (older) page; `null` on the last page.
    pub next_before: Option<i64>,
}

/// The audit log, newest first.
#[utoipa::path(
    get, path = "/api/v1/admin/audit", tag = "admin", params(AuditParams),
    responses((status = 200, body = AuditPage), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody)),
    security(("bearer" = []))
)]
pub async fn audit(
    State(st): State<AdminState>,
    _c: AdminCaller,
    ApiQuery(q): ApiQuery<AuditParams>,
) -> Result<Json<AuditPage>, ApiError> {
    let limit = q
        .limit
        .unwrap_or(DEFAULT_AUDIT_PAGE)
        .clamp(1, MAX_AUDIT_PAGE);
    let before = q.before;
    // One more than asked, to know whether there is a next page.
    let mut rows = blocking(&st.app.authsvc, move |svc| {
        svc.store().audit_before(before, limit + 1)
    })
    .await??;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next_before = if more {
        rows.last().map(|r| r.id)
    } else {
        None
    };
    Ok(Json(AuditPage {
        entries: rows.into_iter().map(AuditEntry::from).collect(),
        next_before,
    }))
}
