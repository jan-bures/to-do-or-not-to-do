//! Byte framing for OpenAI-compatible chat completions. No HTTP, retry or provider credentials.
//! Streaming POST bodies use SSE; a buffered tools-only fallback can use one chunked JSON body.

use super::provider::{ProviderError, StreamEvent, ToolCall, Usage};
use crate::{platform::SseFrame, sse::SseParser};
use serde::Deserialize;

const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_FRAME_BYTES: usize = 32 * 1024;
const MAX_WIRE_CALLS: usize = 8;

/// Treat every decoder error as terminal; no tool calls are produced before a complete round.
pub struct RoundDecoder {
    body: Body,
    bytes: usize,
    round: Assembly,
    failed: bool,
}

opaque_debug!(RoundDecoder);

enum Body {
    Sse {
        parser: SseParser,
        utf8_tail: Vec<u8>,
    },
    Json(Vec<u8>),
}

impl RoundDecoder {
    #[must_use]
    pub fn sse() -> Self {
        Self::new(Body::Sse {
            parser: SseParser::with_max_frame_bytes(MAX_FRAME_BYTES),
            utf8_tail: Vec::new(),
        })
    }

    #[must_use]
    pub fn json() -> Self {
        Self::new(Body::Json(Vec::new()))
    }

    fn new(body: Body) -> Self {
        Self {
            body,
            bytes: 0,
            round: Assembly::default(),
            failed: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        if self.failed {
            return Err(ProviderError::InvalidResponse);
        }
        self.bytes = self.bytes.saturating_add(bytes.len());
        let result = if self.bytes > MAX_BODY_BYTES {
            Err(ProviderError::Limit)
        } else {
            self.push_inner(bytes)
        };
        self.failed = result.is_err();
        result
    }

    fn push_inner(&mut self, bytes: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        let mut out = Vec::new();
        match &mut self.body {
            Body::Json(body) => body.extend_from_slice(bytes),
            Body::Sse { parser, utf8_tail } => {
                // The shared sync parser replaces invalid UTF-8. Inference must reject it so an
                // invalid byte cannot silently change a tool argument. Carry at most three bytes.
                let joined;
                let bytes = if utf8_tail.is_empty() {
                    bytes
                } else {
                    joined = [utf8_tail.as_slice(), bytes].concat();
                    utf8_tail.clear();
                    &joined
                };
                let good = match std::str::from_utf8(bytes) {
                    Ok(text) => text.as_bytes(),
                    Err(error) if error.error_len().is_none() => {
                        utf8_tail.extend_from_slice(&bytes[error.valid_up_to()..]);
                        &bytes[..error.valid_up_to()]
                    }
                    Err(_) => return Err(ProviderError::InvalidResponse),
                };
                for frame in parser.push(good).map_err(|_| ProviderError::Limit)? {
                    if let SseFrame::Event { event, data, .. } = frame {
                        if self.round.ended || event != "message" {
                            return Err(ProviderError::InvalidResponse);
                        }
                        if data.trim() == "[DONE]" {
                            self.round.end(&mut out)?;
                        } else {
                            self.round.chunk(data.as_bytes(), false, &mut out)?;
                        }
                    }
                }
                if self.round.ended && (!utf8_tail.is_empty() || !parser.at_frame_boundary()) {
                    return Err(ProviderError::InvalidResponse);
                }
            }
        }
        Ok(out)
    }

    /// Call on clean EOF, not a transport failure. A fully framed successful `finish_reason`
    /// suffices when an endpoint omits `[DONE]`; a truncated response is never a successful draft.
    pub fn finish(mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        if self.failed {
            return Err(ProviderError::InvalidResponse);
        }
        if self.round.ended {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        match &self.body {
            Body::Json(body) => self.round.chunk(body, true, &mut out)?,
            Body::Sse { parser, utf8_tail } => {
                if !utf8_tail.is_empty() || !parser.at_frame_boundary() {
                    return Err(ProviderError::Incomplete);
                }
            }
        }
        self.round.end(&mut out)?;
        Ok(out)
    }
}

#[derive(Deserialize)]
struct Chunk {
    choices: Vec<Choice>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct Choice {
    index: usize,
    delta: Option<Content>,
    message: Option<Content>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct Content {
    content: Option<String>,
    tool_calls: Option<Vec<ToolDelta>>,
    function_call: Option<serde::de::IgnoredAny>,
    refusal: Option<String>,
}

#[derive(Deserialize)]
struct ToolDelta {
    index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    function: Option<Function>,
}

#[derive(Deserialize)]
struct Function {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Default)]
struct PartialCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

#[derive(Default)]
struct Assembly {
    calls: Vec<PartialCall>,
    finish: Option<bool>, // Whether the model explicitly finished with tool calls.
    ended: bool,
}

impl Assembly {
    fn chunk(
        &mut self,
        bytes: &[u8],
        buffered: bool,
        out: &mut Vec<StreamEvent>,
    ) -> Result<(), ProviderError> {
        let bad = ProviderError::InvalidResponse;
        let chunk: Chunk = serde_json::from_slice(bytes).map_err(|_| bad)?;
        if chunk.choices.len() > 1 || (buffered && chunk.choices.len() != 1) {
            return Err(bad);
        }
        for choice in chunk.choices {
            if choice.index != 0 || self.finish.is_some() {
                return Err(bad);
            }
            let content = match (buffered, choice.delta, choice.message) {
                (true, None, Some(content)) | (false, Some(content), None) => content,
                (false, None, None) if choice.finish_reason.is_some() => Content {
                    content: None,
                    tool_calls: None,
                    function_call: None,
                    refusal: None,
                },
                _ => return Err(bad),
            };
            if content.function_call.is_some() || content.refusal.is_some() {
                return Err(bad);
            }
            if let Some(text) = content.content.filter(|text| !text.is_empty()) {
                out.push(StreamEvent::Text(text));
            }
            for (position, delta) in content
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                let index = if buffered {
                    position
                } else {
                    delta.index.ok_or(bad)?
                };
                self.add(index, delta)?;
            }
            if let Some(finish) = choice.finish_reason {
                self.finish = Some(match finish.as_str() {
                    "stop" => false,
                    "tool_calls" => true,
                    "length" => return Err(ProviderError::Limit),
                    _ => return Err(bad),
                });
            }
        }
        if let Some(usage) = chunk.usage {
            out.push(StreamEvent::Usage(Usage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
            }));
        }
        Ok(())
    }

    fn add(&mut self, index: usize, delta: ToolDelta) -> Result<(), ProviderError> {
        let bad = ProviderError::InvalidResponse;
        if index >= MAX_WIRE_CALLS {
            return Err(ProviderError::Limit);
        }
        if delta.kind.is_some_and(|kind| kind != "function") || index > self.calls.len() {
            return Err(bad);
        }
        if index == self.calls.len() {
            self.calls.push(PartialCall::default());
        }
        let call = &mut self.calls[index];
        fn set(slot: &mut Option<String>, value: Option<String>) -> Result<(), ProviderError> {
            if let Some(value) = value
                && (value.is_empty()
                    || value.len() > super::MAX_CALL_LABEL_BYTES
                    || slot.replace(value).is_some())
            {
                return Err(ProviderError::InvalidResponse);
            }
            Ok(())
        }
        set(&mut call.id, delta.id)?;
        if let Some(function) = delta.function {
            set(&mut call.name, function.name)?;
            if let Some(fragment) = function.arguments {
                if call.arguments.len().saturating_add(fragment.len()) > super::MAX_ARGUMENT_BYTES {
                    return Err(ProviderError::Limit);
                }
                call.arguments.push_str(&fragment);
            }
        }
        Ok(())
    }

    fn end(&mut self, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        let failure = ProviderError::Incomplete;
        let tools = self.finish.ok_or(failure)?;
        if tools == self.calls.is_empty() {
            return Err(failure);
        }
        for partial in self.calls.drain(..) {
            out.push(StreamEvent::Tool(ToolCall::parse(
                partial.id.ok_or(failure)?,
                partial.name.ok_or(failure)?,
                &partial.arguments,
            )?));
        }
        self.ended = true;
        out.push(StreamEvent::Finished);
        Ok(())
    }
}
