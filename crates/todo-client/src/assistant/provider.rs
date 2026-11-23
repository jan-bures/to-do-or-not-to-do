//! Platform-neutral boundary for inference. Credentials and wire-specific history belong to the adapter.

use crate::platform::{MaybeSend, MaybeSync};
use futures_util::{Stream, future::AbortHandle};
use serde::{Serialize, de};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, fmt, future::Future, time::Duration};
use todo_core::{ids::ListId, time::Date};
use uuid::Uuid;

use super::ProposalItem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataClass {
    Local,
    Cloud,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BillingClass {
    NoPerTokenCharge,
    Included,
    Subscription,
    Metered,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub streaming: bool,
    pub tools: bool,
    pub streaming_tools: bool,
    pub parallel_tools: bool,
    pub max_output_tokens: Option<u32>,
    pub usage: bool,
    pub model_listing: bool,
    pub data_class: DataClass,
    pub billing: BillingClass,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Safe to display: no provider payloads, tool arguments, credentials or endpoint addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("the model provider isn't responding")]
    Unavailable,
    #[error("the model provider took too long to respond")]
    Timeout,
    #[error("the model provider's TLS connection could not be verified")]
    Tls,
    #[error("Tailscale isn't connected on this PC")]
    TailnetDisconnected,
    #[error("this destination isn't verified as private")]
    NotPrivate,
    #[error("the model provider rejected sign-in")]
    Unauthorized,
    #[error("the model provider's limit was reached")]
    RateLimited { retry_after: Option<Duration> },
    #[error("the selected model is not available")]
    ModelMissing,
    #[error("the model provider doesn't support this operation")]
    Unsupported,
    #[error("the model response was invalid")]
    InvalidResponse,
    #[error("the model response ended before it finished")]
    Incomplete,
    #[error("the model response exceeded its limit")]
    Limit,
}

/// The host drops in-flight futures/streams on cancellation or timeout. An adapter must tie any
/// separately running request to those lifetimes; it can also check this token before doing work.
#[derive(Clone, Debug)]
pub struct Cancel(pub(super) AbortHandle);

impl Cancel {
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_aborted()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: Map<String, Value>,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Map<String, Value>,
}

impl ToolCall {
    /// Require exactly one object with unique fields. Decoding into Value directly would silently
    /// accept duplicate keys (including security-sensitive fields like `task_id`). The restricted
    /// schemas only accept scalar values; nested values are rejected by typed tool decoding.
    pub fn parse(id: String, name: String, arguments: &str) -> Result<Self, ProviderError> {
        use serde::Deserializer;
        struct UniqueObject;
        impl<'de> de::Visitor<'de> for UniqueObject {
            type Value = Map<String, Value>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object with unique fields")
            }

            fn visit_map<A: de::MapAccess<'de>>(
                self,
                mut reader: A,
            ) -> Result<Self::Value, A::Error> {
                let mut map = Map::new();
                while let Some((key, value)) = reader.next_entry()? {
                    if map.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate tool argument"));
                    }
                }
                Ok(map)
            }
        }
        if arguments.len() > super::MAX_ARGUMENT_BYTES {
            return Err(ProviderError::Limit);
        }
        let mut decoder = serde_json::Deserializer::from_str(arguments);
        let arguments = decoder
            .deserialize_map(UniqueObject)
            .map_err(|_| ProviderError::InvalidResponse)?;
        decoder.end().map_err(|_| ProviderError::InvalidResponse)?;
        Ok(Self {
            id,
            name,
            arguments,
        })
    }
}

/// Only trusted orchestration metadata is separated from content. A provider must send a tool
/// result as a tool message, never interpolate its JSON into system instructions.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        text: String,
    },
    Assistant {
        text: String,
        calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        untrusted_data: Value,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundMode {
    Streaming,
    Buffered,
}

#[derive(Clone)]
pub struct RoundRequest {
    pub turn: Uuid,
    pub round: usize,
    pub model: String,
    pub instructions: &'static str,
    /// Pinned list IDs for local transport authorization; not part of the inference payload.
    pub list_scope: BTreeSet<ListId>,
    pub history: Vec<Message>,
    /// Owner-local date pinned for this turn.
    pub today: Date,
    /// If the user is refining a pending proposal, its selected effects. This is untrusted
    /// context, not system instructions or an applied change. Do not reuse provisional IDs.
    pub previous_draft: Option<Vec<ProposalItem>>,
    pub tools: Vec<ToolDefinition>,
    pub mode: RoundMode,
    pub max_output_tokens: u32,
}

impl fmt::Debug for RoundRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoundRequest")
            .field("turn", &self.turn)
            .field("round", &self.round)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub enum StreamEvent {
    Text(String),
    Tool(ToolCall),
    Usage(Usage),
    /// Exactly once, only when the provider has positively indicated a complete round. A stream
    /// error is a terminal failure. EOF alone is not success.
    Finished,
}

opaque_debug!(Message, ToolCall, StreamEvent);

/// A platform check for direct cleartext Tailscale. Implementations inspect current local
/// interface state without performing network I/O; an unsupported platform should refuse it.
pub trait TailnetInterface: MaybeSend + MaybeSync {
    fn connected(&self) -> bool;
}

pub trait Provider: MaybeSend + MaybeSync + 'static {
    type Stream: Stream<Item = Result<StreamEvent, ProviderError>> + Unpin + MaybeSend;

    fn list_models(&self) -> impl Future<Output = Result<Vec<Model>, ProviderError>> + MaybeSend;
    fn capabilities(&self, model: &str) -> Capabilities;
    fn check_connection(&self) -> impl Future<Output = Result<(), ProviderError>> + MaybeSend;

    fn stream_turn(
        &self,
        request: RoundRequest,
        cancel: Cancel,
    ) -> impl Future<Output = Result<Self::Stream, ProviderError>> + MaybeSend;
}
