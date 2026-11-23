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
    failed: Option<ProviderError>,
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
            failed: None,
        }
    }

    /// Feed one chunk. If a later frame fails, return earlier deltas first, then return the error
    /// on the next push or EOF. Chunk boundaries never affect which deltas reach the caller.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let accepted = bytes.len().min(MAX_BODY_BYTES - self.bytes);
        self.bytes += accepted;
        let mut out = Vec::new();
        self.failed = self
            .push_inner(&bytes[..accepted], &mut out)
            .err()
            .or_else(|| (accepted < bytes.len()).then_some(ProviderError::Limit));
        match self.failed {
            Some(error) if out.is_empty() => Err(error),
            _ => Ok(out),
        }
    }

    fn push_inner(
        &mut self,
        bytes: &[u8],
        out: &mut Vec<StreamEvent>,
    ) -> Result<(), ProviderError> {
        match &mut self.body {
            Body::Json(body) => body.extend_from_slice(bytes),
            Body::Sse { parser, utf8_tail } => {
                // The shared sync parser replaces invalid UTF-8. Inference must reject it so an
                // invalid byte cannot silently change a tool argument. Carry at most three bytes.
                for part in bytes.split_inclusive(|b| matches!(b, b'\r' | b'\n')) {
                    let joined;
                    let bytes = if utf8_tail.is_empty() {
                        part
                    } else {
                        joined = [utf8_tail.as_slice(), part].concat();
                        utf8_tail.clear();
                        &joined
                    };
                    let (good, invalid) = match std::str::from_utf8(bytes) {
                        Ok(text) => (text.as_bytes(), false),
                        Err(error) => {
                            let (prefix, tail) = bytes.split_at(error.valid_up_to());
                            let invalid = error.error_len().is_some();
                            if !invalid {
                                utf8_tail.extend_from_slice(tail);
                            }
                            (prefix, invalid)
                        }
                    };
                    for frame in parser.push(good).map_err(|_| ProviderError::Limit)? {
                        if let SseFrame::Event { event, data, .. } = frame {
                            if self.round.done_marker || event != "message" {
                                return Err(ProviderError::InvalidResponse);
                            }
                            if data.trim() == "[DONE]" {
                                self.round.check_finish()?;
                                self.round.done_marker = true;
                            } else {
                                self.round.chunk(data.as_bytes(), false, out)?;
                            }
                        }
                    }
                    if invalid {
                        return Err(ProviderError::InvalidResponse);
                    }
                }
            }
        }
        Ok(())
    }

    /// Call on clean EOF, not a transport failure. A fully framed successful `finish_reason`
    /// suffices when an endpoint omits `[DONE]`. Wait for EOF even with `[DONE]` so tool delivery
    /// cannot depend on whether malformed trailing data was in the same network chunk.
    pub fn finish(mut self) -> Result<Vec<StreamEvent>, ProviderError> {
        if let Some(error) = self.failed {
            return Err(error);
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
    arguments: Option<String>,
}

#[derive(Default)]
struct Assembly {
    calls: Vec<PartialCall>,
    finish: Option<bool>, // Whether the model explicitly finished with tool calls.
    done_marker: bool,
    usage: Option<Usage>,
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
        let mut text = None;
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
            text = content.content.filter(|text| !text.is_empty());
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
        if let Some(text) = text {
            out.push(StreamEvent::Text(text));
        }
        if let Some(usage) = chunk.usage {
            // Some servers send usage both with finish_reason and in a final standalone
            // chunk. These are cumulative counters, not two separate charges.
            let previous = self.usage.unwrap_or_default();
            self.usage = Some(Usage {
                input_tokens: previous.input_tokens.max(usage.prompt_tokens),
                output_tokens: previous.output_tokens.max(usage.completion_tokens),
            });
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
                let arguments = call.arguments.get_or_insert_with(String::new);
                if arguments.len().saturating_add(fragment.len()) > super::MAX_ARGUMENT_BYTES {
                    return Err(ProviderError::Limit);
                }
                arguments.push_str(&fragment);
            }
        }
        Ok(())
    }

    fn check_finish(&self) -> Result<(), ProviderError> {
        let tools = self.finish.ok_or(ProviderError::Incomplete)?;
        if tools == self.calls.is_empty() {
            return Err(ProviderError::Incomplete);
        }
        Ok(())
    }

    fn end(&mut self, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        self.check_finish()?;
        let failure = ProviderError::Incomplete;
        let mut calls = Vec::new();
        for partial in self.calls.drain(..) {
            let arguments = partial.arguments.ok_or(failure)?;
            let call = ToolCall::parse(
                partial.id.ok_or(failure)?,
                partial.name.ok_or(failure)?,
                if arguments.is_empty() {
                    "{}"
                } else {
                    &arguments
                },
            )?;
            if calls
                .iter()
                .any(|previous: &ToolCall| previous.id == call.id)
            {
                return Err(ProviderError::InvalidResponse);
            }
            calls.push(call);
        }
        if let Some(usage) = self.usage {
            out.push(StreamEvent::Usage(usage));
        }
        out.extend(calls.into_iter().map(StreamEvent::Tool));
        out.push(StreamEvent::Finished);
        Ok(())
    }
}
