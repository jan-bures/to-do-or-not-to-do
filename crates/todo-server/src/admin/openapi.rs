//! The admin API's own OpenAPI document, served at `/api/v1/admin/openapi.json` on the admin
//! listener only. The public document doesn't list these routes: the public listener doesn't serve
//! them.

use super::{AdminCaller, handlers as h};
use crate::api::error;
use axum::Json;
use std::sync::OnceLock;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}

/// The admin API description.
#[derive(Debug, OpenApi)]
#[openapi(
    info(
        title = "todo-server admin API",
        description = "Local-only admin operations (loopback listener, `tdx_` bearer token): email ingest settings, agents, devices, share links, the audit log. No task data; no new credentials; no passkeys.",
        license(name = "PolyForm Noncommercial License 1.0.0", identifier = "PolyForm-Noncommercial-1.0.0"),
    ),
    paths(
        document,
        h::lists,
        h::ingest,
        h::inbox_set,
        h::senders,
        h::sender_add,
        h::sender_remove,
        h::keys,
        h::key_add,
        h::key_remove,
        h::agents,
        h::agent,
        h::agent_access,
        h::agent_revoke,
        h::devices,
        h::device_revoke,
        h::shares,
        h::share_revoke,
        h::audit,
    ),
    components(schemas(
        h::CollectionName,
        h::ListName,
        h::Structure,
        h::ListRef,
        h::Sender,
        h::ListKey,
        h::IngestSettings,
        h::SenderPage,
        h::KeyPage,
        h::InboxRequest,
        h::InboxResponse,
        h::SenderRequest,
        h::SenderAdded,
        h::KeyRequest,
        h::GrantsBody,
        h::AgentInfo,
        h::AgentPage,
        h::AccessRequest,
        h::DeviceInfo,
        h::DevicePage,
        h::ShareTargetInfo,
        h::ShareInfo,
        h::SharePage,
        h::AuditEntry,
        h::AuditPage,
        error::ErrorBody,
        error::ErrorDetail,
    )),
    modifiers(&BearerAuth),
)]
pub struct AdminDoc;

/// The generated document (built once).
pub fn spec() -> &'static utoipa::openapi::OpenApi {
    static DOC: OnceLock<utoipa::openapi::OpenApi> = OnceLock::new();
    DOC.get_or_init(AdminDoc::openapi)
}

/// This document.
#[utoipa::path(
    get, path = "/api/v1/admin/openapi.json", tag = "admin",
    responses((status = 200, description = "The admin API's OpenAPI document"), (status = 401, body = error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn document(_c: AdminCaller) -> Json<utoipa::openapi::OpenApi> {
    Json(spec().clone())
}
