//! MCP helpers: real agent tokens with grants, an `rmcp` client, and tool calls.

use super::TestServer;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use todo_core::ids::ListId;
use todo_core::{AccessLevel, AgentId, Grants};

pub type Client = RunningService<RoleClient, ()>;

/// Every tool the server offers.
pub const ALL_TOOLS: [&str; 23] = [
    "add_link",
    "add_note",
    "add_reminder",
    "add_step",
    "archive_task",
    "complete_task",
    "create_tag",
    "create_task",
    "get_activity",
    "get_task",
    "list_collections",
    "list_lists",
    "list_tags",
    "merge_tasks",
    "move_task",
    "query_tasks",
    "set_my_day",
    "tag_task",
    "undo_event",
    "untag_task",
    "update_reminder",
    "update_step",
    "update_task",
];

/// A real agent token with `grants`; returns `(token row id, token)`.
pub fn agent(
    t: &TestServer,
    name: &str,
    grants: &Grants,
    writes_per_hour: Option<u32>,
) -> (i64, String) {
    let svc = &t.server.authsvc;
    let (row, token) = svc
        .create_agent(
            &AgentId::new(name).unwrap(),
            AccessLevel::None,
            writes_per_hour,
            None,
            None,
        )
        .unwrap();
    assert!(svc.set_grants(row, grants, None).unwrap());
    (row, token)
}

/// An MCP client for `token`.
pub async fn connect(t: &TestServer, token: &str) -> Client {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", t.url)).auth_header(token),
    );
    ().serve(transport).await.unwrap()
}

pub fn args(v: Value) -> rmcp::model::JsonObject {
    match v {
        Value::Object(m) => m,
        _ => panic!("arguments must be an object"),
    }
}

pub async fn call(c: &Client, tool: &str, a: Value) -> CallToolResult {
    c.call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args(a)))
        .await
        .unwrap()
}

/// The structured result of a call that must succeed.
pub async fn ok(c: &Client, tool: &str, a: Value) -> Value {
    let r = call(c, tool, a.clone()).await;
    let v = r.structured_content.clone().unwrap_or(Value::Null);
    assert_ne!(r.is_error, Some(true), "{tool} {a}: {v}");
    v
}

/// The error of a call that must fail as a tool error.
pub async fn fails(c: &Client, tool: &str, a: Value) -> Value {
    let r = call(c, tool, a.clone()).await;
    assert_eq!(r.is_error, Some(true), "{tool} {a} succeeded");
    let v = r.structured_content.unwrap();
    // The text content carries the same envelope, for clients that only show text.
    v["error"].clone()
}

/// An error with `id` replaced by a placeholder, to compare the answers for two different IDs.
pub fn same_shape(e: &Value, id: &str) -> String {
    e.to_string().replace(id, "<id>")
}

pub fn list_id(s: &str) -> ListId {
    serde_json::from_value(json!(s)).unwrap()
}

pub fn grants(default: AccessLevel, lists: &[(&str, AccessLevel)]) -> Grants {
    let mut g = Grants::with_default(default);
    for (l, level) in lists {
        g.lists.insert(list_id(l), *level);
    }
    g
}
