//! Inference transport: same-origin owner session for the hosted relay, no To-do credentials for loopback.

use futures_util::{Stream, stream};
use js_sys::{Reflect, Uint8Array};
use serde::de::DeserializeOwned;
use std::{collections::VecDeque, pin::Pin};
use todo_client::{
    assistant::{
        endpoint::Destination,
        openai::{self, BrowserEndpointError},
        provider::{
            BillingClass, Cancel, Capabilities, DataClass, Model, Provider, ProviderError,
            RoundRequest, StreamEvent, TurnTimeouts,
        },
        relay, responses,
        wire::RoundDecoder,
    },
    platform::{Method, Request as ApiRequest, StreamOpen, Transport},
    web::FetchBody,
};
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, ReadableStreamDefaultReader, ReferrerPolicy, Request, RequestCredentials,
    RequestInit, RequestMode, RequestRedirect,
};

pub(super) const PATH: &str = "/api/v1/assistant";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Hosted {
    Private,
    ChatGpt(Uuid),
}

impl Hosted {
    fn path(self, action: &str) -> String {
        match self {
            Self::Private => format!("{PATH}/private/{action}"),
            Self::ChatGpt(id) => format!("{PATH}/chatgpt/accounts/{id}/{action}"),
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Private => "Private via your server",
            Self::ChatGpt(_) => "ChatGPT · Cloud",
        }
    }

    pub(super) async fn models(self) -> Result<Vec<relay::Model>, ProviderError> {
        relay_json(Method::Get, self.path("models"), None).await
    }
}

pub(super) async fn relay_json<T: DeserializeOwned>(
    method: Method,
    path: String,
    body: Option<Vec<u8>>,
) -> Result<T, ProviderError> {
    let transport = crate::backend::assistant_transport().ok_or(ProviderError::Unauthorized)?;
    let response = transport
        .send(ApiRequest { method, path, body })
        .await
        .map_err(|_| ProviderError::Unavailable)?;
    if response.status == 200 {
        serde_json::from_slice(&response.body).map_err(|_| ProviderError::InvalidResponse)
    } else {
        Err(refused(response.status, response.retry_after))
    }
}

pub(super) fn encoded(body: &impl serde::Serialize) -> Result<Option<Vec<u8>>, ProviderError> {
    serde_json::to_vec(body)
        .map(Some)
        .map_err(|_| ProviderError::InvalidResponse)
}

struct Direct {
    url: String,
    key: String,
}

pub(super) struct BrowserProvider {
    route: Route,
    still_allowed: Option<Box<dyn Fn() -> bool + Send + Sync>>,
}

enum Route {
    Direct(Direct),
    Hosted(Hosted),
}

impl BrowserProvider {
    pub(super) fn new(address: &str) -> Result<Self, BrowserEndpointError> {
        let (url, destination) = openai::browser_endpoint(address)?;
        debug_assert_eq!(destination, Destination::ThisDevice);
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .ok_or(BrowserEndpointError::Address)?;
        if url.origin().ascii_serialization() == origin {
            return Err(BrowserEndpointError::SameOrigin);
        }
        Ok(Self {
            route: Route::Direct(Direct {
                url: url.into(),
                key: String::new(),
            }),
            still_allowed: None,
        })
    }

    pub(super) fn hosted(hosted: Hosted) -> Self {
        Self {
            route: Route::Hosted(hosted),
            still_allowed: None,
        }
    }

    pub(super) fn bind(
        mut self,
        key: String,
        still_allowed: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        if let Route::Direct(direct) = &mut self.route {
            direct.key = key;
        }
        self.still_allowed = Some(Box::new(still_allowed));
        self
    }
}

struct Abort(AbortController);
impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

// Retain the correct cancellation handle until the body is read or dropped.
enum Fetch {
    Direct(web_sys::Response, Abort),
    Relay(FetchBody),
}
impl Fetch {
    fn response(&self) -> &web_sys::Response {
        match self {
            Self::Direct(res, _guard) => res,
            Self::Relay(body) => body.response(),
        }
    }
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

type Events = Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>>>>;

impl Provider for BrowserProvider {
    type Stream = Events;

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        let Route::Hosted(hosted) = self.route else {
            return Err(ProviderError::Unsupported);
        };
        Ok(hosted
            .models()
            .await?
            .into_iter()
            .map(|m| Model {
                id: m.id,
                label: m.label,
            })
            .collect())
    }

    async fn check_connection(&self) -> Result<(), ProviderError> {
        self.list_models().await.map(|_| ())
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        let cloud = matches!(self.route, Route::Hosted(Hosted::ChatGpt(_)));
        Capabilities {
            streaming: true,
            tools: true,
            streaming_tools: true,
            parallel_tools: false,
            max_output_tokens: (!cloud).then_some(2048),
            usage: true,
            model_listing: matches!(self.route, Route::Hosted(_)),
            data_class: if cloud {
                DataClass::Cloud
            } else {
                DataClass::Local
            },
            billing: if cloud {
                BillingClass::Subscription
            } else {
                BillingClass::NoPerTokenCharge
            },
        }
    }

    fn turn_timeouts(&self) -> TurnTimeouts {
        match self.route {
            Route::Hosted(Hosted::Private) => TurnTimeouts::HOSTED_PRIVATE,
            Route::Hosted(Hosted::ChatGpt(_)) => TurnTimeouts::HOSTED_CHATGPT,
            Route::Direct(_) => TurnTimeouts::default(),
        }
    }

    async fn stream_turn(
        &self,
        round: RoundRequest,
        cancel: Cancel,
    ) -> Result<Events, ProviderError> {
        if cancel.is_cancelled() || !self.still_allowed.as_ref().is_some_and(|allowed| allowed()) {
            return Err(ProviderError::Incomplete);
        }
        let cloud = matches!(self.route, Route::Hosted(Hosted::ChatGpt(_)));
        let payload = if cloud {
            responses::payload(&round)
        } else {
            openai::payload(&round)
        };
        let fetch = match &self.route {
            Route::Direct(direct) => direct.send(&payload.to_string()).await?,
            Route::Hosted(hosted) => {
                let transport =
                    crate::backend::assistant_transport().ok_or(ProviderError::Unauthorized)?;
                let response = transport
                    .open_body(
                        ApiRequest {
                            method: Method::Post,
                            path: hosted.path("chat"),
                            body: encoded(&relay::Chat {
                                destination: if cloud {
                                    DataClass::Cloud
                                } else {
                                    DataClass::Local
                                },
                                lists: round.list_scope,
                                request: payload,
                            })?,
                        },
                        "text/event-stream, application/json",
                    )
                    .await
                    .map_err(|_| ProviderError::Unavailable)?;
                match response {
                    StreamOpen::Open(body) => Fetch::Relay(body),
                    StreamOpen::Refused(res) => return Err(refused(res.status, res.retry_after)),
                }
            }
        };
        let response = fetch.response();
        let media = response
            .headers()
            .get("content-type")
            .ok()
            .flatten()
            .unwrap_or_default();
        let decoder = match (media.split(';').next().map(str::trim), cloud) {
            (Some("text/event-stream"), true) => Decoder::Responses(Box::default()),
            (Some("text/event-stream"), false) => Decoder::Chat(Box::new(RoundDecoder::sse())),
            (Some("application/json"), false) => Decoder::Chat(Box::new(RoundDecoder::json())),
            _ => return Err(ProviderError::InvalidResponse),
        };
        let reader = response
            .body()
            .ok_or(ProviderError::Incomplete)?
            .get_reader()
            .dyn_into::<ReadableStreamDefaultReader>()
            .map_err(|_| ProviderError::InvalidResponse)?;
        Ok(Box::pin(stream::unfold(
            (reader, fetch, cancel, Some(decoder), VecDeque::new()),
            |(reader, fetch, cancel, mut decoder, mut events)| async move {
                loop {
                    if cancel.is_cancelled() {
                        return None;
                    }
                    if let Some(event) = events.pop_front() {
                        return Some((event, (reader, fetch, cancel, decoder, events)));
                    }
                    let next = decoder.as_mut()?;
                    let result = match read_chunk(&reader).await {
                        Ok(Some(bytes)) => next.push(&bytes),
                        Ok(None) => decoder.take()?.finish(),
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(decoded) => events.extend(decoded.into_iter().map(Ok)),
                        Err(error) => {
                            decoder = None;
                            events.push_back(Err(error));
                        }
                    }
                }
            },
        )))
    }
}

impl Direct {
    async fn send(&self, payload: &str) -> Result<Fetch, ProviderError> {
        let abort = Abort(AbortController::new().map_err(|_| ProviderError::Unavailable)?);
        let init = RequestInit::new();
        init.set_method("POST");
        init.set_credentials(RequestCredentials::Omit);
        init.set_mode(RequestMode::Cors);
        init.set_redirect(RequestRedirect::Error);
        init.set_referrer_policy(ReferrerPolicy::NoReferrer);
        init.set_signal(Some(&abort.0.signal()));
        init.set_body(&JsValue::from_str(payload));
        let req = Request::new_with_str_and_init(&self.url, &init)
            .map_err(|_| ProviderError::Unavailable)?;
        req.headers()
            .set("Content-Type", "application/json")
            .map_err(|_| ProviderError::Unavailable)?;
        req.headers()
            .set("Accept", "text/event-stream, application/json")
            .map_err(|_| ProviderError::Unavailable)?;
        if !self.key.is_empty() {
            req.headers()
                .set("Authorization", &format!("Bearer {}", self.key))
                .map_err(|_| ProviderError::Unauthorized)?;
        }
        let window = web_sys::window().ok_or(ProviderError::Unavailable)?;
        let response = JsFuture::from(window.fetch_with_request(&req))
            .await
            .map_err(|_| ProviderError::Unavailable)?
            .dyn_into::<web_sys::Response>()
            .map_err(|_| ProviderError::InvalidResponse)?;
        if !response.ok() {
            return Err(refused(response.status(), None));
        }
        Ok(Fetch::Direct(response, abort))
    }
}

fn refused(status: u16, retry_after: Option<std::time::Duration>) -> ProviderError {
    match status {
        401 | 424 => ProviderError::Unauthorized,
        403 | 409 => ProviderError::NotPrivate,
        404 => ProviderError::ModelMissing,
        429 => ProviderError::RateLimited { retry_after },
        408 | 504 => ProviderError::Timeout,
        400 | 405 | 415 | 422 | 501 => ProviderError::Unsupported,
        500..=599 => ProviderError::Unavailable,
        _ => ProviderError::InvalidResponse,
    }
}

async fn read_chunk(
    reader: &ReadableStreamDefaultReader,
) -> Result<Option<Vec<u8>>, ProviderError> {
    let item = JsFuture::from(reader.read())
        .await
        .map_err(|_| ProviderError::Incomplete)?;
    let field = |key| {
        Reflect::get(&item, &JsValue::from_str(key)).map_err(|_| ProviderError::InvalidResponse)
    };
    if field("done")?.as_bool() == Some(true) {
        return Ok(None);
    }
    let value = field("value")?
        .dyn_into::<Uint8Array>()
        .map_err(|_| ProviderError::InvalidResponse)?;
    Ok(Some(value.to_vec()))
}
