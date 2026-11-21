//! The OpenAPI document, generated with `utoipa` and served at `/api/v1/openapi.json`.

use super::{attachments, commands, dto, error, events, pair, read};
use crate::auth::Authenticated;
use crate::engine::{CommandResult, EventRef};
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

/// The API description.
#[derive(Debug, OpenApi)]
#[openapi(
    info(
        title = "todo-server",
        description = "Personal to-do server: commands, undo, reads and the event stream.",
        license(name = "PolyForm Noncommercial License 1.0.0", identifier = "PolyForm-Noncommercial-1.0.0"),
    ),
    paths(
        commands::post_command,
        commands::post_undo,
        events::get_events,
        read::activity,
        read::collections,
        read::lists,
        read::list_tasks,
        read::query_tasks,
        read::task_detail,
        read::tags,
        read::smart_lists,
        read::smart_list_tasks,
        attachments::upload,
        attachments::download,
        attachments::head,
        attachments::fill_content,
        pair::start,
        pair::poll,
        crate::ingest::post_email,
        document,
    ),
    components(schemas(
        CommandResult,
        EventRef,
        dto::CommandRequest,
        dto::CommandResponse,
        dto::UndoRequest,
        attachments::UploadResponse,
        attachments::ContentResponse,
        dto::TaskDetail,
        dto::EventPage,
        dto::ActivityItem,
        dto::ActivityPage,
        dto::SmartList,
        dto::CollectionPage,
        dto::ListPage,
        dto::TaskPage,
        dto::TagPage,
        events::CatchUp,
        events::Hello,
        crate::db::LogEvent,
        pair::PairStart,
        pair::PairStarted,
        pair::PairPoll,
        pair::PairStatus,
        error::ErrorBody,
        error::ErrorDetail,
    )),
    modifiers(&BearerAuth),
)]
pub struct ApiDoc;

/// The generated document (built once).
pub fn spec() -> &'static utoipa::openapi::OpenApi {
    static DOC: OnceLock<utoipa::openapi::OpenApi> = OnceLock::new();
    DOC.get_or_init(ApiDoc::openapi)
}

/// This document.
#[utoipa::path(
    get, path = "/api/v1/openapi.json", tag = "meta",
    responses((status = 200, description = "The OpenAPI document"), (status = 401, body = error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn document(_auth: Authenticated) -> Json<utoipa::openapi::OpenApi> {
    Json(spec().clone())
}
