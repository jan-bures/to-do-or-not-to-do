//! A direct browser transport, separate from the authenticated To-do API transport.

use futures_util::{Stream, stream};
use js_sys::{Reflect, Uint8Array};
use std::{collections::VecDeque, pin::Pin};
use todo_client::assistant::{
    openai,
    provider::{
        BillingClass, Cancel, Capabilities, DataClass, Model, Provider, ProviderError,
        RoundRequest, StreamEvent,
    },
    wire::RoundDecoder,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, ReadableStreamDefaultReader, ReferrerPolicy, Request, RequestCredentials,
    RequestInit, RequestMode, RequestRedirect,
};

pub(super) struct BrowserProvider {
    url: String,
    pub(super) class: DataClass,
    still_allowed: Option<Box<dyn Fn() -> bool + Send + Sync>>,
}

impl BrowserProvider {
    pub(super) fn new(address: &str) -> Option<Self> {
        let (url, class) = openai::browser_endpoint(address)?;
        // The provider is never allowed to address the authenticated app itself.
        if url.origin().ascii_serialization() == web_sys::window()?.location().origin().ok()? {
            return None;
        }
        Some(Self {
            url: url.into(),
            class,
            still_allowed: None,
        })
    }

    pub(super) fn bind(mut self, still_allowed: impl Fn() -> bool + Send + Sync + 'static) -> Self {
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

type Events = Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>>>>;

impl Provider for BrowserProvider {
    type Stream = Events;

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    async fn check_connection(&self) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported)
    }

    fn capabilities(&self, _model: &str) -> Capabilities {
        Capabilities {
            streaming: true,
            tools: true,
            streaming_tools: true,
            parallel_tools: false,
            max_output_tokens: Some(2048),
            usage: true,
            model_listing: false,
            data_class: self.class,
            billing: if self.class == DataClass::Cloud {
                BillingClass::Metered
            } else {
                BillingClass::NoPerTokenCharge
            },
        }
    }

    async fn stream_turn(
        &self,
        round: RoundRequest,
        cancel: Cancel,
    ) -> Result<Events, ProviderError> {
        if cancel.is_cancelled() || !self.still_allowed.as_ref().is_some_and(|allowed| allowed()) {
            // Recheck the local profile at the actual network boundary, so another tab's
            // revocation takes effect even before its StorageEvent is delivered here.
            return Err(ProviderError::Incomplete);
        }
        let abort = Abort(AbortController::new().map_err(|_| ProviderError::Unavailable)?);
        let init = RequestInit::new();
        init.set_method("POST");
        init.set_credentials(RequestCredentials::Omit);
        init.set_mode(RequestMode::Cors);
        init.set_redirect(RequestRedirect::Error);
        init.set_referrer_policy(ReferrerPolicy::NoReferrer);
        init.set_signal(Some(&abort.0.signal()));
        init.set_body(&JsValue::from_str(&openai::payload(&round).to_string()));
        let req = Request::new_with_str_and_init(&self.url, &init)
            .map_err(|_| ProviderError::Unavailable)?;
        req.headers()
            .set("Content-Type", "application/json")
            .map_err(|_| ProviderError::Unavailable)?;
        req.headers()
            .set("Accept", "text/event-stream, application/json")
            .map_err(|_| ProviderError::Unavailable)?;
        let window = web_sys::window().ok_or(ProviderError::Unavailable)?;
        let response = JsFuture::from(window.fetch_with_request(&req))
            .await
            .map_err(|_| ProviderError::Unavailable)?
            .dyn_into::<web_sys::Response>()
            .map_err(|_| ProviderError::InvalidResponse)?;
        if !response.ok() {
            return Err(match response.status() {
                401 | 403 => ProviderError::Unauthorized,
                404 => ProviderError::ModelMissing,
                429 => ProviderError::RateLimited { retry_after: None },
                408 | 504 => ProviderError::Timeout,
                400 | 405 | 415 | 422 | 501 => ProviderError::Unsupported,
                500..=599 => ProviderError::Unavailable,
                _ => ProviderError::InvalidResponse,
            });
        }
        let content_type = response
            .headers()
            .get("content-type")
            .ok()
            .flatten()
            .unwrap_or_default();
        let decoder = match content_type
            .split(';')
            .next()
            .map(|s| s.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("text/event-stream") => RoundDecoder::sse(),
            Some("application/json") => RoundDecoder::json(),
            _ => return Err(ProviderError::InvalidResponse),
        };
        let reader = response
            .body()
            .ok_or(ProviderError::Incomplete)?
            .get_reader()
            .dyn_into::<ReadableStreamDefaultReader>()
            .map_err(|_| ProviderError::InvalidResponse)?;
        Ok(Box::pin(stream::unfold(
            (reader, abort, cancel, Some(decoder), VecDeque::new()),
            |(reader, abort, cancel, mut decoder, mut events)| async move {
                loop {
                    if cancel.is_cancelled() {
                        return None;
                    }
                    if let Some(event) = events.pop_front() {
                        return Some((event, (reader, abort, cancel, decoder, events)));
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
