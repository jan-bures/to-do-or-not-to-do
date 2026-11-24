//! Shared Responses adapter for the server relay. No credentials or network access.

use super::provider::{Message, ProviderError, RoundRequest, StreamEvent, ToolCall, Usage};
use crate::{platform::SseFrame, sse::SseParser};
use serde_json::{Value, json};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_FRAME: usize = 256 * 1024;
const MAX_CALLS: usize = 8;

pub fn payload(request: &RoundRequest) -> Value {
    let mut input = Vec::new();
    if let Some(previous) = &request.previous_draft {
        input.push(json!({"role":"user", "content":format!("Untrusted previous draft (not applied): {}", json!(previous))}));
    }
    for message in &request.history {
        match message {
            Message::User { text } => input.push(json!({"role":"user", "content":text})),
            Message::Assistant { text, calls } => {
                if !text.is_empty() { input.push(json!({"role":"assistant", "content":text})); }
                input.extend(calls.iter().map(|call| json!({
                    "type":"function_call", "name":call.name, "arguments":json!(call.arguments).to_string(), "call_id":call.id,
                })));
            }
            Message::Tool { call_id, untrusted_data } => input.push(json!({
                "type":"function_call_output", "call_id":call_id, "output":untrusted_data.to_string(),
            })),
        }
    }
    json!({
        "model": request.model,
        "instructions": format!("{} Owner's local date: {}.", request.instructions, request.today),
        "input": input,
        "tools": request.tools.iter().map(|tool| json!({
            "type":"function", "name":tool.name, "description":tool.description, "parameters":tool.arguments, "strict":false,
        })).collect::<Vec<_>>(),
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
    })
}

/// A reply may stream prose immediately, but tool calls are only published on confirmed EOF
/// after `response.completed`. A truncated reply cannot queue a tool call.
pub struct Decoder {
    parser: SseParser,
    tail: Vec<u8>,
    bytes: usize,
    calls: Vec<ToolCall>,
    complete: bool,
    usage: Option<Usage>,
}

opaque_debug!(Decoder);

impl Default for Decoder {
    fn default() -> Self {
        Self {
            parser: SseParser::with_max_frame_bytes(MAX_FRAME),
            tail: Vec::new(),
            bytes: 0,
            calls: Vec::new(),
            complete: false,
            usage: None,
        }
    }
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        if bytes.len() > MAX_BYTES - self.bytes {
            return Err(ProviderError::Limit);
        }
        self.bytes += bytes.len();
        let joined;
        let bytes = if self.tail.is_empty() {
            bytes
        } else {
            joined = [self.tail.as_slice(), bytes].concat();
            self.tail.clear();
            &joined
        };
        let bytes = match std::str::from_utf8(bytes) {
            Ok(_) => bytes,
            Err(err) if err.error_len().is_none() => {
                let (prefix, rest) = bytes.split_at(err.valid_up_to());
                self.tail.extend_from_slice(rest);
                prefix
            }
            Err(_) => return Err(ProviderError::InvalidResponse),
        };
        let mut out = Vec::new();
        for frame in self.parser.push(bytes).map_err(|_| ProviderError::Limit)? {
            let SseFrame::Event { data, .. } = frame else {
                continue;
            };
            if data == "[DONE]" && self.complete {
                continue;
            }
            if self.complete {
                return Err(ProviderError::InvalidResponse);
            }
            let event: Value =
                serde_json::from_str(&data).map_err(|_| ProviderError::InvalidResponse)?;
            match event["type"].as_str() {
                Some("response.output_text.delta") => out.push(StreamEvent::Text(
                    event["delta"]
                        .as_str()
                        .ok_or(ProviderError::InvalidResponse)?
                        .into(),
                )),
                Some("response.output_item.done") if event["item"]["type"] == "function_call" => {
                    if self.calls.len() >= MAX_CALLS {
                        return Err(ProviderError::Limit);
                    }
                    let item = &event["item"];
                    if item["namespace"]
                        .as_str()
                        .is_some_and(|namespace| namespace != "todo")
                    {
                        return Err(ProviderError::Unsupported);
                    }
                    let field =
                        |key: &str| item[key].as_str().ok_or(ProviderError::InvalidResponse);
                    let call = ToolCall::parse(
                        field("call_id")?.into(),
                        field("name")?.into(),
                        field("arguments")?,
                    )?;
                    if self.calls.iter().any(|old| old.id == call.id) {
                        return Err(ProviderError::InvalidResponse);
                    }
                    self.calls.push(call);
                }
                Some("response.completed") => {
                    self.complete = true;
                    let usage = &event["response"]["usage"];
                    if let (Some(input_tokens), Some(output_tokens)) = (
                        usage["input_tokens"].as_u64(),
                        usage["output_tokens"].as_u64(),
                    ) {
                        self.usage = Some(Usage {
                            input_tokens,
                            output_tokens,
                        });
                    }
                }
                Some("response.failed" | "error") => {
                    let code = event["response"]["error"]["code"]
                        .as_str()
                        .or(event["code"].as_str());
                    return Err(match code {
                        Some(
                            "subscription_sharing_usage_limit_exceeded" | "rate_limit_exceeded",
                        ) => ProviderError::RateLimited { retry_after: None },
                        Some(
                            "chatpass_v2_scope_not_authorized"
                            | "subscription_sharing_user_not_eligible",
                        ) => ProviderError::Unauthorized,
                        _ => ProviderError::InvalidResponse,
                    });
                }
                Some("response.incomplete") => return Err(ProviderError::Incomplete),
                Some(_) => {}
                None => return Err(ProviderError::InvalidResponse),
            }
        }
        Ok(out)
    }

    pub fn finish(self) -> Result<Vec<StreamEvent>, ProviderError> {
        if !self.complete || !self.tail.is_empty() || !self.parser.at_frame_boundary() {
            return Err(ProviderError::Incomplete);
        }
        let mut events = self
            .calls
            .into_iter()
            .map(StreamEvent::Tool)
            .collect::<Vec<_>>();
        events.extend(self.usage.map(StreamEvent::Usage));
        events.push(StreamEvent::Finished);
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_utf8_and_tools_only_after_completion() {
        let reply = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"é\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"name\":\"search\",\"arguments\":\"{}\",\"call_id\":\"test\"}}\n\n",
        );
        let mut decoder = Decoder::default();
        let events = reply
            .bytes()
            .flat_map(|b| decoder.push(&[b]).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(events.as_slice(), [StreamEvent::Text(text)] if text == "é"));
        assert!(matches!(decoder.finish(), Err(ProviderError::Incomplete)));
        let mut decoder = Decoder::default();
        decoder.push(reply.as_bytes()).unwrap();
        decoder
            .push(b"data: {\"type\":\"response.completed\"}\n\n")
            .unwrap();
        assert!(matches!(
            decoder.finish().unwrap().as_slice(),
            [StreamEvent::Tool(_), StreamEvent::Finished]
        ));
    }
}
