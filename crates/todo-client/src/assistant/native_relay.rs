//! The native assistant borrows its paired REST transport, including the revocation latch.
//! Nothing in this module stores or receives an upstream token or key.
use std::{collections::VecDeque, pin::Pin, time::Duration};

use futures_util::{Stream, stream};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use super::{
    local::LocalProvider,
    openai,
    provider::{
        BillingClass, Cancel, Capabilities, DataClass, Model, Provider, ProviderError,
        RoundRequest, StreamEvent, TurnTimeouts,
    },
    relay, responses,
    wire::RoundDecoder,
};
use crate::{
    native::HttpTransport,
    platform::{Method, Request, Transport, TransportError},
};

pub const VERIFY_URL: &str = "https://auth.openai.com/codex/device";
pub const USAGE_URL: &str = "https://chatgpt.com/settings/usage";
const PATH: &str = "/api/v1/assistant";

type Events = Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>;

#[derive(Clone, Debug)]
pub struct Relay(HttpTransport);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Hosted {
    Private,
    ChatGpt(Uuid),
}

impl Hosted {
    pub fn label(self) -> &'static str {
        match self {
            Self::Private => "Private via your server",
            Self::ChatGpt(_) => "ChatGPT (cloud)",
        }
    }
    fn path(self, action: &str) -> String {
        match self {
            Self::Private => format!("{PATH}/private/{action}"),
            Self::ChatGpt(id) => format!("{PATH}/chatgpt/accounts/{id}/{action}"),
        }
    }
    fn cloud(self) -> bool {
        matches!(self, Self::ChatGpt(_))
    }
}

fn network(err: TransportError) -> ProviderError {
    match err {
        TransportError::Timeout => ProviderError::Timeout,
        _ => ProviderError::Unavailable,
    }
}

fn refused(status: u16, retry_after: Option<Duration>) -> ProviderError {
    match status {
        401 | 424 => ProviderError::Unauthorized,
        403 | 409 => ProviderError::NotPrivate,
        404 => ProviderError::ModelMissing,
        429 => ProviderError::RateLimited { retry_after },
        408 | 504 => ProviderError::Timeout,
        400 | 405 | 415 | 422 | 501 => ProviderError::Unsupported,
        _ => ProviderError::Unavailable,
    }
}

impl Relay {
    pub fn new(transport: HttpTransport) -> Self {
        Self(transport)
    }

    async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: String,
        body: Option<Vec<u8>>,
    ) -> Result<T, ProviderError> {
        let res = self
            .0
            .send(Request { method, path, body })
            .await
            .map_err(network)?;
        if res.status != 200 {
            return Err(refused(res.status, res.retry_after));
        }
        serde_json::from_slice(&res.body).map_err(|_| ProviderError::InvalidResponse)
    }
    pub async fn status(&self) -> Result<relay::Status, ProviderError> {
        self.json(Method::Get, format!("{PATH}/status"), None).await
    }
    pub async fn access(
        &self,
        update: relay::AccessUpdate,
    ) -> Result<relay::Status, ProviderError> {
        self.json(Method::Put, format!("{PATH}/access"), encode(&update)?)
            .await
    }
    pub async fn consent(
        &self,
        id: Uuid,
        consent: relay::Consent,
    ) -> Result<relay::Account, ProviderError> {
        self.json(
            Method::Put,
            Hosted::ChatGpt(id).path("consent"),
            encode(&consent)?,
        )
        .await
    }
    pub async fn start(&self, flow: relay::SignIn) -> Result<relay::SignInStarted, ProviderError> {
        self.json(
            Method::Post,
            format!("{PATH}/chatgpt/start"),
            encode(&flow)?,
        )
        .await
    }
    pub async fn poll(&self, id: Uuid) -> Result<relay::Poll, ProviderError> {
        self.json(Method::Post, format!("{PATH}/chatgpt/poll/{id}"), None)
            .await
    }
    pub async fn handoff(
        &self,
        id: Uuid,
        data: relay::Handoff,
    ) -> Result<relay::Account, ProviderError> {
        self.json(
            Method::Post,
            format!("{PATH}/chatgpt/handoff/{id}"),
            encode(&data)?,
        )
        .await
    }
    pub async fn sign_out(&self, id: Uuid) -> Result<relay::SignOut, ProviderError> {
        self.json(
            Method::Delete,
            format!("{PATH}/chatgpt/accounts/{id}"),
            None,
        )
        .await
    }
    pub fn provider(&self, route: Hosted) -> NativeProvider {
        NativeProvider::Relay(self.clone(), route)
    }
    pub async fn models(&self, route: Hosted) -> Result<Vec<Model>, ProviderError> {
        let data: Vec<relay::Model> = self.json(Method::Get, route.path("models"), None).await?;
        Ok(data
            .into_iter()
            .map(|m| Model {
                id: m.id,
                label: m.label,
            })
            .collect())
    }

    async fn chat(
        &self,
        route: Hosted,
        round: RoundRequest,
        cancel: Cancel,
    ) -> Result<Events, ProviderError> {
        let cloud = route.cloud();
        let request = if cloud {
            responses::payload(&round)
        } else {
            openai::payload(&round)
        };
        let res = self
            .0
            .assistant_body(Request {
                method: Method::Post,
                path: route.path("chat"),
                body: encode(&relay::Chat {
                    destination: if cloud {
                        DataClass::Cloud
                    } else {
                        DataClass::Local
                    },
                    lists: round.list_scope,
                    request,
                })?,
            })
            .await
            .map_err(network)?;
        if res.status() != 200 {
            let retry = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            return Err(refused(res.status().as_u16(), retry));
        }
        let media = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim);
        let decoder = match (cloud, media) {
            (true, Some("text/event-stream")) => Decoder::Responses(Box::default()),
            (false, Some("text/event-stream")) => Decoder::Chat(Box::new(RoundDecoder::sse())),
            (false, Some("application/json")) => Decoder::Chat(Box::new(RoundDecoder::json())),
            _ => return Err(ProviderError::InvalidResponse),
        };
        Ok(Box::pin(stream::unfold(
            (res, cancel, Some(decoder), VecDeque::new()),
            |(mut res, cancel, mut decoder, mut queue)| async move {
                loop {
                    if cancel.is_cancelled() {
                        return None;
                    }
                    if let Some(event) = queue.pop_front() {
                        return Some((event, (res, cancel, decoder, queue)));
                    }
                    let next = decoder.as_mut()?;
                    let result = match res.chunk().await {
                        Ok(Some(bytes)) => next.push(&bytes),
                        Ok(None) => decoder.take()?.finish(),
                        Err(e) if e.is_timeout() => Err(ProviderError::Timeout),
                        Err(_) => Err(ProviderError::Incomplete),
                    };
                    match result {
                        Ok(events) => queue.extend(events.into_iter().map(Ok)),
                        Err(e) => {
                            decoder = None;
                            queue.push_back(Err(e));
                        }
                    }
                }
            },
        )))
    }
}

fn encode(input: &impl Serialize) -> Result<Option<Vec<u8>>, ProviderError> {
    serde_json::to_vec(input)
        .map(Some)
        .map_err(|_| ProviderError::InvalidResponse)
}

enum Decoder {
    Chat(Box<RoundDecoder>),
    Responses(Box<responses::Decoder>),
}
impl Decoder {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        match self {
            Self::Chat(d) => d.push(bytes),
            Self::Responses(d) => d.push(bytes),
        }
    }
    fn finish(self) -> Result<Vec<StreamEvent>, ProviderError> {
        match self {
            Self::Chat(d) => d.finish(),
            Self::Responses(d) => d.finish(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum NativeProvider {
    Direct(LocalProvider),
    Relay(Relay, Hosted),
}

impl Provider for NativeProvider {
    type Stream = Events;

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        match self {
            Self::Direct(p) => p.list_models().await,
            Self::Relay(relay, route) => relay.models(*route).await,
        }
    }
    async fn check_connection(&self) -> Result<(), ProviderError> {
        self.list_models().await.map(|_| ())
    }
    fn capabilities(&self, model: &str) -> Capabilities {
        match self {
            Self::Direct(p) => p.capabilities(model),
            Self::Relay(_, route) => Capabilities {
                streaming: true,
                tools: true,
                streaming_tools: true,
                parallel_tools: false,
                max_output_tokens: (!route.cloud()).then_some(2048),
                usage: true,
                model_listing: true,
                data_class: if route.cloud() {
                    DataClass::Cloud
                } else {
                    DataClass::Local
                },
                billing: if route.cloud() {
                    BillingClass::Subscription
                } else {
                    BillingClass::NoPerTokenCharge
                },
            },
        }
    }
    fn turn_timeouts(&self) -> TurnTimeouts {
        match self {
            Self::Relay(_, Hosted::Private) => TurnTimeouts::HOSTED_PRIVATE,
            Self::Relay(_, Hosted::ChatGpt(_)) => TurnTimeouts::HOSTED_CHATGPT,
            Self::Direct(_) => TurnTimeouts::default(),
        }
    }
    async fn stream_turn(
        &self,
        request: RoundRequest,
        cancel: Cancel,
    ) -> Result<Self::Stream, ProviderError> {
        match self {
            Self::Direct(p) => p.stream_turn(request, cancel).await,
            Self::Relay(relay, route) => relay.chat(*route, request, cancel).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_routes_use_longer_decoded_event_timeouts() -> Result<(), Box<dyn std::error::Error>> {
        let relay = Relay::new(HttpTransport::new(
            "https://todo.example.com",
            "test-token",
        )?);
        assert_eq!(
            relay.provider(Hosted::Private).turn_timeouts(),
            TurnTimeouts {
                idle: Duration::from_secs(600),
                total: Duration::from_secs(600),
            }
        );
        assert_eq!(
            relay.provider(Hosted::ChatGpt(Uuid::nil())).turn_timeouts(),
            TurnTimeouts {
                idle: Duration::from_secs(75),
                total: Duration::from_secs(240),
            }
        );
        Ok(())
    }
}
