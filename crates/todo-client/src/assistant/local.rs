//! Native, owner-configured Chat Completions transport. Building a provider is entirely offline.
//! This client has no access to the todo server, device credentials, sync, or write authority.

pub use super::provider::TailnetInterface;
use super::{
    openai::payload,
    provider::{
        BillingClass, Cancel, Capabilities, DataClass, Message, Model, Provider, ProviderError,
        RoundMode, RoundRequest, StreamEvent, ToolDefinition,
    },
    wire::RoundDecoder,
};
use futures_util::{Stream, StreamExt, future::AbortHandle, stream};
use reqwest::{
    Client, Response, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, RETRY_AFTER},
};
use serde::Deserialize;
#[cfg(test)]
use serde_json::Value;
use serde_json::json;
use std::{
    collections::{BTreeMap, VecDeque},
    net::IpAddr,
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use unicode_general_category::{GeneralCategory, get_general_category};
use url::{Host, Url};
use zeroize::Zeroizing;

const CONNECT: Duration = Duration::from_secs(4);
const CHECK: Duration = Duration::from_secs(8);
const IDLE: Duration = Duration::from_secs(20);
const ROUND: Duration = Duration::from_secs(75);
const CATALOG_BYTES: usize = 128 * 1024;
const MAX_MODELS: usize = 256;
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

#[cfg(test)]
mod tests;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    #[error("Enter an HTTP or HTTPS base URL without credentials, a query, or a fragment.")]
    Address,
    #[error(
        "Use HTTPS unless the server is on this computer or reachable directly over Tailscale."
    )]
    Insecure,
    #[error("The API key contains unsupported characters or is too long.")]
    Credential,
    #[error("Couldn't initialize the model connection.")]
    Client,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cleartext {
    Loopback,
    Tailnet,
}

/// Classifies an address, not its route. The platform check separately looks for an up interface.
pub fn is_tailnet_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            a == 100 && (64..=127).contains(&b)
        }
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    ThisDevice,
    Tailscale,
    YourServer,
    NotPrivate,
}

impl Destination {
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisDevice => "This device",
            Self::Tailscale => "Tailscale",
            Self::YourServer => "Your server",
            Self::NotPrivate => "Not private",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EndpointInfo {
    pub destination: Destination,
    pub tailnet_http: bool,
}

pub fn endpoint_info(input: &str, attested: bool) -> Result<EndpointInfo, SetupError> {
    base(input, attested).map(|(_, destination, cleartext)| EndpointInfo {
        destination,
        tailnet_http: cleartext == Some(Cleartext::Tailnet),
    })
}

fn base(input: &str, attested: bool) -> Result<(Url, Destination, Option<Cleartext>), SetupError> {
    let input = input.trim();
    let (_, location) = input.split_once("://").ok_or(SetupError::Address)?;
    let authority = location.split(['/', '?', '#']).next().unwrap_or_default();
    if input.len() > 2048
        || input
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        || authority.is_empty()
        || authority.contains('@')
        || authority.ends_with(':')
    {
        return Err(SetupError::Address);
    }
    let mut url = Url::parse(input).map_err(|_| SetupError::Address)?;
    if url.query().is_some() || url.fragment().is_some() || url.port() == Some(0) {
        return Err(SetupError::Address);
    }
    let destination = match url.host().ok_or(SetupError::Address)? {
        Host::Domain(host) if host.trim_end_matches('.') == "localhost" => Destination::ThisDevice,
        Host::Ipv4(ip) if ip.is_loopback() => Destination::ThisDevice,
        Host::Ipv6(ip) if ip.is_loopback() => Destination::ThisDevice,
        Host::Domain(host) if host.trim_end_matches('.').ends_with(".ts.net") => {
            Destination::Tailscale
        }
        Host::Ipv4(ip) if is_tailnet_ip(ip.into()) => Destination::Tailscale,
        Host::Ipv6(ip) if is_tailnet_ip(ip.into()) => Destination::Tailscale,
        _ if attested => Destination::YourServer,
        _ => Destination::NotPrivate,
    };
    let policy = match (url.scheme(), destination) {
        ("https", _) => None,
        ("http", Destination::ThisDevice) => Some(Cleartext::Loopback),
        ("http", Destination::Tailscale) => Some(Cleartext::Tailnet),
        ("http", _) => return Err(SetupError::Insecure),
        _ => return Err(SetupError::Address),
    };
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/v1") {
        format!("{path}/")
    } else {
        format!("{path}/v1/")
    };
    url.set_path(&path);
    Ok((url, destination, policy))
}

struct DirectDns(Cleartext);

impl reqwest::dns::Resolve for DirectDns {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let name = name.as_str().to_owned();
        let class = self.0;
        Box::pin(async move {
            // Restrict the actual addresses given to the connector, not just a preflight lookup.
            let peers: Vec<_> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|peer| match class {
                    Cleartext::Loopback => peer.ip().is_loopback(),
                    Cleartext::Tailnet => is_tailnet_ip(peer.ip()),
                })
                .collect();
            if peers.is_empty() {
                return Err(std::io::Error::other("no address in the permitted network").into());
            }
            Ok(Box::new(peers.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Measured for a single endpoint/model by an explicit, task-free test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
pub enum Support {
    StreamingTools,
    BufferedTools,
    TextOnly,
}

impl Support {
    pub fn label(self) -> &'static str {
        match self {
            Self::StreamingTools => "Streaming + task tools",
            Self::BufferedTools => "Task tools · buffered replies",
            Self::TextOnly => "Text only · no task access",
        }
    }
}

#[derive(Clone)]
pub struct LocalProvider {
    client: Client,
    base: Url,
    auth: Option<HeaderValue>,
    destination: Destination,
    cleartext: Option<Cleartext>,
    tailnet_interface: Option<Arc<dyn TailnetInterface>>,
    tested: Option<(String, Support)>,
}

opaque_debug!(LocalProvider);

type Events = Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>;

impl LocalProvider {
    pub fn new(address: &str, key: &str) -> Result<Self, SetupError> {
        Self::configured(address, key, false)
    }

    /// `attested` must come from an explicit owner decision for this exact configured URL.
    pub fn configured(address: &str, key: &str, attested: bool) -> Result<Self, SetupError> {
        let (base, destination, cleartext) = base(address, attested)?;
        let key = key.trim();
        let auth = if key.is_empty() {
            None
        } else {
            if key.len() > 4096 || !key.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(SetupError::Credential);
            }
            let bearer = Zeroizing::new(format!("Bearer {key}"));
            let mut header = HeaderValue::from_str(&bearer).map_err(|_| SetupError::Credential)?;
            header.set_sensitive(true);
            Some(header)
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut client = Client::builder()
            .tls_backend_rustls()
            .connect_timeout(CONNECT)
            .read_timeout(IDLE)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy();
        // A DNS-derived local classification must remain local at the connector, also for HTTPS.
        let direct = match destination {
            Destination::ThisDevice => Some(Cleartext::Loopback),
            Destination::Tailscale => Some(Cleartext::Tailnet),
            _ => None,
        };
        if let Some(direct) = direct {
            client = client.dns_resolver(Arc::new(DirectDns(direct)));
        }
        Ok(Self {
            client: client.build().map_err(|_| SetupError::Client)?,
            base,
            auth,
            destination,
            cleartext,
            tailnet_interface: None,
            tested: None,
        })
    }

    pub fn base_url(&self) -> &str {
        self.base.as_str()
    }

    pub fn with_tailnet_interface(mut self, interface: impl TailnetInterface + 'static) -> Self {
        self.tailnet_interface = Some(Arc::new(interface));
        self
    }

    pub fn with_support(mut self, model: String, support: Support) -> Self {
        self.tested = Some((model, support));
        self
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        if self.destination == Destination::NotPrivate {
            return Err(ProviderError::NotPrivate);
        }
        // Check immediately before every request, even if the connector can reuse a socket.
        if self.cleartext == Some(Cleartext::Tailnet)
            && !self
                .tailnet_interface
                .as_ref()
                .is_some_and(|interface| interface.connected())
        {
            return Err(ProviderError::TailnetDisconnected);
        }
        let url = self
            .base
            .join(path)
            .map_err(|_| ProviderError::InvalidResponse)?;
        let mut request = self.client.request(method, url);
        if let Some(auth) = &self.auth {
            request = request.header(AUTHORIZATION, auth.clone());
        }
        Ok(request)
    }

    /// Run only from a dedicated button. Payloads here are invented, even when tools are tested.
    /// Probe text only after a protocol/tool incompatibility, never after a network or usage error.
    pub async fn probe(&self, model: &str) -> Result<Support, ProviderError> {
        if !valid_model(model) {
            return Err(ProviderError::ModelMissing);
        }
        for (mode, tools, support) in [
            (RoundMode::Streaming, true, Support::StreamingTools),
            (RoundMode::Buffered, true, Support::BufferedTools),
            (RoundMode::Streaming, false, Support::TextOnly),
        ] {
            match tokio::time::timeout(IDLE * 2, self.probe_mode(model, mode, tools)).await {
                Ok(Ok(())) => return Ok(support),
                Ok(Err(
                    ProviderError::Unsupported
                    | ProviderError::InvalidResponse
                    | ProviderError::Incomplete,
                )) if tools => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(ProviderError::Timeout),
            }
        }
        Err(ProviderError::Unsupported)
    }

    async fn probe_mode(
        &self,
        model: &str,
        mode: RoundMode,
        tools: bool,
    ) -> Result<(), ProviderError> {
        const NAME: &str = "connection_check";
        let token = uuid::Uuid::new_v4().simple().to_string();
        let (cancel, _) = AbortHandle::new_pair();
        let mut request = RoundRequest {
            turn: uuid::Uuid::nil(),
            round: 0,
            model: model.into(),
            instructions: "This is a connection test with invented data only. If given a tool, call it with an empty object, then echo the marker from the tool result verbatim. If not given a tool, say ready.",
            history: vec![Message::User {
                text: "Check the connection using the sample task 'Water the plant'.".into(),
            }],
            today: todo_core::time::Date::ZERO,
            previous_draft: None,
            tools: if tools {
                vec![ToolDefinition {
                    name: NAME,
                    description: "Return a test marker without reading or changing anything.",
                    arguments: serde_json::Map::from_iter([
                        ("type".into(), json!("object")),
                        ("properties".into(), json!({})),
                        ("additionalProperties".into(), json!(false)),
                    ]),
                }]
            } else {
                Vec::new()
            },
            mode,
            max_output_tokens: 128,
            list_scope: Default::default(),
        };
        let (text, mut calls) = collect(
            self.stream_turn(request.clone(), Cancel(cancel.clone()))
                .await?,
        )
        .await?;
        if !tools {
            return if !text.trim().is_empty() && calls.is_empty() {
                Ok(())
            } else {
                Err(ProviderError::Unsupported)
            };
        }
        if calls.len() != 1 {
            return Err(ProviderError::Unsupported);
        }
        let call = calls.pop().ok_or(ProviderError::Unsupported)?;
        if call.name != NAME || !call.arguments.is_empty() {
            return Err(ProviderError::Unsupported);
        }
        request.round = 1;
        request.history.push(Message::Assistant {
            text,
            calls: vec![call.clone()],
        });
        request.history.push(Message::Tool {
            call_id: call.id,
            untrusted_data: json!({"marker": token}),
        });
        let (text, calls) = collect(self.stream_turn(request, Cancel(cancel)).await?).await?;
        if calls.is_empty() && text.contains(&token) {
            Ok(())
        } else {
            Err(ProviderError::Unsupported)
        }
    }
}

async fn collect(
    mut events: Events,
) -> Result<(String, Vec<super::provider::ToolCall>), ProviderError> {
    let mut text = String::new();
    let mut calls = Vec::new();
    while let Some(event) = events.next().await {
        match event? {
            StreamEvent::Text(delta) => text.push_str(&delta),
            StreamEvent::Tool(call) => calls.push(call),
            StreamEvent::Finished => return Ok((text, calls)),
            StreamEvent::Usage(_) => {}
        }
    }
    Err(ProviderError::Incomplete)
}

pub fn valid_model(id: &str) -> bool {
    !id.trim().is_empty()
        && id.len() <= 256
        && !id.chars().any(|c| {
            c.is_control()
                || matches!(
                    get_general_category(c),
                    GeneralCategory::Format
                        | GeneralCategory::LineSeparator
                        | GeneralCategory::ParagraphSeparator
                )
        })
}

impl Provider for LocalProvider {
    type Stream = Events;

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        let mut response = self
            .request(reqwest::Method::GET, "models")?
            .header(ACCEPT, "application/json")
            .timeout(CHECK)
            .send()
            .await
            .map_err(network)?;
        check_status(&response, false)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            if bytes.len().saturating_add(chunk.len()) > CATALOG_BYTES {
                return Err(ProviderError::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct Catalog {
            data: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            id: String,
        }
        let catalog: Catalog =
            serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)?;
        let models: BTreeMap<_, _> = catalog
            .data
            .into_iter()
            .filter(|entry| valid_model(&entry.id))
            .map(|entry| {
                (
                    entry.id.clone(),
                    Model {
                        label: entry.id.clone(),
                        id: entry.id,
                    },
                )
            })
            .collect();
        Ok(models.into_values().take(MAX_MODELS).collect())
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        let support = self
            .tested
            .as_ref()
            .filter(|(id, _)| id == model)
            .map(|(_, support)| *support)
            .unwrap_or(Support::TextOnly);
        Capabilities {
            streaming: true,
            tools: support != Support::TextOnly,
            streaming_tools: support == Support::StreamingTools,
            parallel_tools: false,
            max_output_tokens: Some(2048),
            usage: true,
            model_listing: true,
            data_class: if self.destination == Destination::NotPrivate {
                DataClass::Cloud
            } else {
                DataClass::Local
            },
            billing: if self.destination == Destination::NotPrivate {
                BillingClass::Unknown
            } else {
                BillingClass::NoPerTokenCharge
            },
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
        if cancel.is_cancelled() {
            return Err(ProviderError::Incomplete);
        }
        if !valid_model(&request.model) {
            return Err(ProviderError::ModelMissing);
        }
        let response = self
            .request(reqwest::Method::POST, "chat/completions")?
            .header(
                ACCEPT,
                if request.mode == RoundMode::Streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .timeout(ROUND)
            .json(&payload(&request))
            .send()
            .await
            .map_err(network)?;
        check_status(&response, true)?;
        let sse = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"))
            });
        let decoder = if sse {
            RoundDecoder::sse()
        } else if request.mode == RoundMode::Buffered {
            RoundDecoder::json()
        } else {
            return Err(ProviderError::InvalidResponse);
        };
        Ok(Box::pin(stream::unfold(
            (response, Some(decoder), VecDeque::new(), cancel),
            |(mut response, mut decoder, mut ready, cancel)| async move {
                loop {
                    if let Some(event) = ready.pop_front() {
                        return Some((event, (response, decoder, ready, cancel)));
                    }
                    if cancel.is_cancelled() {
                        return None;
                    }
                    let next = decoder.as_mut()?;
                    let result = match response.chunk().await {
                        Ok(Some(bytes)) => next.push(&bytes),
                        Ok(None) => decoder.take()?.finish(),
                        Err(error) => {
                            decoder = None;
                            Err(if error.is_timeout() {
                                ProviderError::Timeout
                            } else {
                                ProviderError::Incomplete
                            })
                        }
                    };
                    match result {
                        Ok(events) => ready.extend(events.into_iter().map(Ok)),
                        Err(error) => {
                            decoder = None;
                            ready.push_back(Err(error));
                        }
                    }
                }
            },
        )))
    }
}

fn network(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        return ProviderError::Timeout;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(cause) = source {
        if cause.is::<rustls::Error>() {
            return ProviderError::Tls;
        }
        // An io::Error can wrap rustls directly but delegate source() past it.
        source = cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .map(|error| error as &(dyn std::error::Error + 'static))
            .or_else(|| cause.source());
    }
    ProviderError::Unavailable
}

fn check_status(response: &Response, chat: bool) -> Result<(), ProviderError> {
    if response.status().is_success() {
        return Ok(());
    }
    Err(match response.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::Unauthorized,
        StatusCode::NOT_FOUND if chat => ProviderError::ModelMissing,
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited {
            retry_after: response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(|seconds| Duration::from_secs(seconds).min(MAX_RETRY_AFTER)),
        },
        StatusCode::NOT_FOUND
        | StatusCode::METHOD_NOT_ALLOWED
        | StatusCode::NOT_IMPLEMENTED
        | StatusCode::BAD_REQUEST
        | StatusCode::UNPROCESSABLE_ENTITY => ProviderError::Unsupported,
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => ProviderError::Timeout,
        status if status.is_server_error() => ProviderError::Unavailable,
        _ => ProviderError::InvalidResponse,
    })
}
