//! ChatGPT plan transport. Only the public Responses route sees prompts or scoped tool history.
use super::{Accounts, Error, RESOURCE};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::VecDeque, pin::Pin, time::Duration};
use todo_client::{
    assistant::provider::{
        BillingClass, Cancel, Capabilities, DataClass, Message, Model, Provider, ProviderError,
        RoundRequest, StreamEvent, ToolCall, Usage,
    },
    platform::SseFrame,
    sse::SseParser,
};

const NAMESPACE: &str = "todo";
pub type Events = Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>;

pub struct CloudProvider {
    pub accounts: Accounts,
    pub client_id: String,
}

#[derive(Deserialize)]
struct Catalog {
    models: Vec<Value>,
}
#[derive(Deserialize)]
struct Entry {
    slug: String,
    display_name: String,
}

pub(super) async fn models(accounts: &Accounts, id: &str) -> Result<Vec<Model>, ProviderError> {
    let bearer = accounts.access(id, None).await.map_err(auth_error)?;
    let service = accounts.service();
    let response = service
        .http
        .get(format!("{RESOURCE}/models"))
        .header(reqwest::header::ACCEPT, "application/json")
        .bearer_auth(bearer.0.as_str())
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(network)?;
    if !response.status().is_success() {
        return Err(admission(response).await);
    }
    let catalog: Catalog = super::Service::read(response).await.map_err(auth_error)?;
    listed_models(catalog)
}

fn listed_models(catalog: Catalog) -> Result<Vec<Model>, ProviderError> {
    catalog
        .models
        .into_iter()
        .filter(|entry| entry["visibility"] == "list")
        .map(|entry| {
            let model: Entry =
                serde_json::from_value(entry).map_err(|_| ProviderError::InvalidResponse)?;
            if !valid_model(&model.slug)
                || model.display_name.trim().is_empty()
                || model.display_name.chars().count() > 200
            {
                return Err(ProviderError::InvalidResponse);
            }
            Ok(Model {
                id: model.slug,
                label: model.display_name,
            })
        })
        .collect()
}

fn valid_model(slug: &str) -> bool {
    !slug.trim().is_empty() && slug.chars().count() <= 200 && !slug.chars().any(char::is_control)
}

fn auth_error(error: Error) -> ProviderError {
    match error {
        Error::Expired | Error::NoGrant => ProviderError::Unauthorized,
        Error::TimedOut => ProviderError::Timeout,
        Error::Network => ProviderError::Unavailable,
        _ => ProviderError::InvalidResponse,
    }
}

fn network(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Unavailable
    }
}

async fn admission(response: reqwest::Response) -> ProviderError {
    let status = response.status();
    // Match only documented codes. Don't show or log the returned diagnostic body, which may
    // contain submitted content. Some admission failures have only a detail field.
    let parsed: Value = match super::Service::read_body(response).await {
        Ok(body) => serde_json::from_slice(&body).unwrap_or_default(),
        Err(_) => Value::Null,
    };
    structured_error(parsed["error"]["code"].as_str()).unwrap_or(match status.as_u16() {
        401 | 403 => ProviderError::Unauthorized,
        404 => ProviderError::ModelMissing,
        429 => ProviderError::RateLimited { retry_after: None },
        400 => ProviderError::Unsupported,
        _ => ProviderError::Unavailable,
    })
}

fn structured_error(code: Option<&str>) -> Option<ProviderError> {
    Some(match code? {
        "subscription_sharing_usage_limit_exceeded" => {
            ProviderError::RateLimited { retry_after: None }
        }
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            ProviderError::Unavailable
        }
        "subscription_sharing_unsupported_capability"
        | "subscription_sharing_route_not_supported" => ProviderError::Unsupported,
        "subscription_sharing_invalid_user"
        | "subscription_sharing_user_not_eligible"
        | "chatpass_v2_scope_not_authorized"
        | "chatpass_v2_invalid_authorization_context" => ProviderError::Unauthorized,
        _ => return None,
    })
}

fn sse_type(headers: &reqwest::header::HeaderMap) -> bool {
    headers.get(reqwest::header::CONTENT_TYPE).is_none_or(|v| {
        v.to_str()
            .ok()
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"))
    })
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    instructions: &'a str,
    input: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    store: bool,
    stream: bool,
}

fn body(request: &RoundRequest) -> Body<'_> {
    let mut input = vec![
        json!({"role":"developer", "content":format!("The owner's current date is {}.", request.today)}),
    ];
    if let Some(previous) = &request.previous_draft {
        input.push(json!({"role":"user", "content":format!("Untrusted previous draft (not applied): {}", json!(previous))}));
    }
    for message in &request.history {
        match message {
            Message::User { text } => input.push(json!({"role":"user", "content":text})),
            Message::Assistant { text, calls } => {
                if !text.is_empty() { input.push(json!({"role":"assistant", "content":text})); }
                input.extend(calls.iter().map(|call| json!({
                    "type":"function_call", "namespace":NAMESPACE, "name":call.name,
                    "arguments":json!(call.arguments).to_string(), "call_id":call.id,
                })));
            }
            Message::Tool { call_id, untrusted_data } => input.push(json!({
                "type":"function_call_output", "call_id":call_id, "output":untrusted_data.to_string(),
            })),
        }
    }
    let tools = request.tools.iter().map(|tool| json!({
        "type":"function", "name":tool.name, "description":tool.description, "parameters":tool.arguments, "strict":false,
    })).collect::<Vec<_>>();
    Body {
        model: &request.model,
        instructions: request.instructions,
        input,
        parallel_tool_calls: (!tools.is_empty()).then_some(false),
        tools: if tools.is_empty() {
            Vec::new()
        } else {
            vec![json!({
                "type":"namespace", "name":NAMESPACE, "description":"Read tasks or prepare proposals for the owner to review.", "tools":tools,
            })]
        },
        store: false,
        stream: true,
    }
}

fn map_event(value: Value) -> Result<Vec<StreamEvent>, ProviderError> {
    let output = match value["type"]
        .as_str()
        .ok_or(ProviderError::InvalidResponse)?
    {
        "response.output_text.delta" => vec![StreamEvent::Text(
            value["delta"]
                .as_str()
                .ok_or(ProviderError::InvalidResponse)?
                .into(),
        )],
        "response.output_item.done" if value["item"]["type"] == "function_call" => {
            let item = &value["item"];
            if item["namespace"]
                .as_str()
                .is_some_and(|name| name != NAMESPACE)
            {
                return Err(ProviderError::Unsupported);
            }
            let field = |key: &str| item[key].as_str().ok_or(ProviderError::InvalidResponse);
            vec![StreamEvent::Tool(ToolCall::parse(
                field("call_id")?.into(),
                field("name")?.into(),
                field("arguments")?,
            )?)]
        }
        "response.completed" => {
            let usage = &value["response"]["usage"];
            let mut events = Vec::with_capacity(2);
            if let (Some(input_tokens), Some(output_tokens)) = (
                usage["input_tokens"].as_u64(),
                usage["output_tokens"].as_u64(),
            ) {
                events.push(StreamEvent::Usage(Usage {
                    input_tokens,
                    output_tokens,
                }));
            }
            events.push(StreamEvent::Finished);
            events
        }
        "response.failed" | "error" => {
            let code = value["response"]["error"]["code"]
                .as_str()
                .or(value["code"].as_str());
            return Err(structured_error(code).unwrap_or(ProviderError::InvalidResponse));
        }
        "response.incomplete" => return Err(ProviderError::Incomplete),
        _ => Vec::new(),
    };
    Ok(output)
}

impl Provider for CloudProvider {
    type Stream = Events;
    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        models(&self.accounts, &self.client_id).await
    }
    fn capabilities(&self, _: &str) -> Capabilities {
        Capabilities {
            streaming: true,
            tools: true,
            streaming_tools: true,
            parallel_tools: false,
            max_output_tokens: None,
            usage: true,
            model_listing: true,
            data_class: DataClass::Cloud,
            billing: BillingClass::Subscription,
        }
    }
    async fn check_connection(&self) -> Result<(), ProviderError> {
        self.list_models().await.map(|_| ())
    }
    async fn stream_turn(
        &self,
        request: RoundRequest,
        cancel: Cancel,
    ) -> Result<Self::Stream, ProviderError> {
        if !valid_model(&request.model) {
            return Err(ProviderError::ModelMissing);
        }
        let bearer = self
            .accounts
            .access(&self.client_id, Some(&request.list_scope))
            .await
            .map_err(auth_error)?;
        if cancel.is_cancelled() {
            return Err(ProviderError::Incomplete);
        }
        let response = self
            .accounts
            .service()
            .http
            .post(format!("{RESOURCE}/responses"))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .bearer_auth(bearer.0.as_str())
            .json(&body(&request))
            .send()
            .await
            .map_err(network)?;
        if !response.status().is_success() {
            return Err(admission(response).await);
        }
        // The direct route may omit Content-Type; the event parser still requires a completed stream.
        if !sse_type(response.headers()) {
            return Err(ProviderError::InvalidResponse);
        }
        let stream = futures::stream::unfold(
            (
                Box::pin(response.bytes_stream()),
                SseParser::new(),
                VecDeque::new(),
                false,
            ),
            |(mut bytes, mut parser, mut pending, mut done): (
                _,
                _,
                VecDeque<Result<StreamEvent, ProviderError>>,
                _,
            )| async move {
                loop {
                    if let Some(event) = pending.pop_front() {
                        return Some((event, (bytes, parser, pending, done)));
                    }
                    if done {
                        return None;
                    }
                    let parsed = match bytes.next().await {
                        Some(Ok(ref chunk)) => parser.push(chunk).map_err(|_| ProviderError::Limit),
                        Some(Err(error)) => Err(network(error)),
                        None => Err(ProviderError::Incomplete),
                    };
                    match parsed {
                        Err(error) => {
                            done = true;
                            pending.push_back(Err(error));
                        }
                        Ok(frames) => {
                            for frame in frames {
                                let SseFrame::Event { data, .. } = frame else {
                                    continue;
                                };
                                let result = serde_json::from_str(&data)
                                    .map_err(|_| ProviderError::InvalidResponse)
                                    .and_then(map_event);
                                match result {
                                    Err(error) => {
                                        done = true;
                                        pending.push_back(Err(error));
                                        break;
                                    }
                                    Ok(events) => {
                                        done = events
                                            .iter()
                                            .any(|e| matches!(e, StreamEvent::Finished));
                                        pending.extend(events.into_iter().map(Ok));
                                        if done {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        );
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_client::assistant::provider::{RoundMode, ToolDefinition};
    use uuid::Uuid;

    #[test]
    fn the_catalog_keeps_only_listed_models_in_account_order() {
        let sample = serde_json::from_value(json!({"models": [
            {"visibility":"list", "slug":"zeta", "display_name":"Zeta"},
            {"visibility":"hidden"},
            {"visibility":"list", "slug":"alpha", "display_name":"Alpha"}
        ]}))
        .expect("catalog shape");
        let models = listed_models(sample).expect("listed models");
        assert_eq!(
            models
                .iter()
                .map(|m| (m.id.as_str(), m.label.as_str()))
                .collect::<Vec<_>>(),
            [("zeta", "Zeta"), ("alpha", "Alpha")]
        );
        assert!(matches!(
            listed_models(Catalog {
                models: vec![json!({"visibility":"list"})]
            }),
            Err(ProviderError::InvalidResponse)
        ));
    }

    #[test]
    fn responses_accept_a_missing_sse_type_but_not_a_json_body_type() {
        use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
        let mut headers = HeaderMap::new();
        assert!(sse_type(&headers));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream; charset=utf-8"),
        );
        assert!(sse_type(&headers));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(!sse_type(&headers));
    }

    #[test]
    fn cloud_request_uses_stateless_streaming_and_only_supported_tool_namespace() {
        let request = RoundRequest {
            turn: Uuid::nil(),
            round: 0,
            model: "sample-model".into(),
            instructions: "Use the supplied tools.",
            history: vec![
                Message::User {
                    text: "sample question".into(),
                },
                Message::Assistant {
                    text: String::new(),
                    calls: vec![
                        ToolCall::parse("first".into(), "list_lists".into(), "{}")
                            .expect("prior tool call"),
                    ],
                },
                Message::Tool {
                    call_id: "first".into(),
                    untrusted_data: json!({"example":"allowed list"}),
                },
            ],
            today: "2026-11-21".parse().expect("date"),
            previous_draft: None,
            tools: vec![ToolDefinition {
                name: "list_lists",
                description: "List visible lists",
                arguments: Default::default(),
            }],
            mode: RoundMode::Streaming,
            max_output_tokens: 10,
            list_scope: [todo_core::ids::ListId::from_uuid(uuid::Uuid::from_u128(1))].into(),
        };
        let request = serde_json::to_value(body(&request)).expect("adapter body");
        assert_eq!(request["stream"], true);
        assert_eq!(request["store"], false);
        assert!(
            request.get("max_output_tokens").is_none()
                && request.get("previous_response_id").is_none()
                && request.get("list_scope").is_none()
        );
        assert_eq!(request["tools"][0]["type"], "namespace");
        assert_eq!(request["tools"][0]["tools"][0]["name"], "list_lists");
        assert_eq!(request["input"][0]["role"], "developer");
        assert_eq!(request["input"][1]["role"], "user");
        assert_eq!(request["input"][2]["type"], "function_call");
        assert_eq!(request["input"][2]["namespace"], "todo");
        assert_eq!(request["input"][3]["type"], "function_call_output");
        assert_eq!(request["input"][3]["call_id"], "first");
    }

    #[test]
    fn only_completed_is_success_and_limit_after_deltas_stays_terminal() {
        let events = map_event(json!({"type":"response.output_item.done", "item":{
            "type":"function_call", "namespace":"todo", "call_id":"one", "name":"list_lists", "arguments":"{}"
        }})).expect("tool result");
        assert!(matches!(events.as_slice(), [StreamEvent::Tool(_)]));
        let complete =
            map_event(json!({"type":"response.completed", "response":{"status":"completed"}}))
                .expect("terminal success");
        assert!(matches!(complete.as_slice(), [StreamEvent::Finished]));
        assert!(matches!(
            map_event(json!({"type":"response.incomplete"})),
            Err(ProviderError::Incomplete)
        ));
        assert!(matches!(
            map_event(
                json!({"type":"response.failed", "response":{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}})
            ),
            Err(ProviderError::RateLimited { .. })
        ));
    }
}
