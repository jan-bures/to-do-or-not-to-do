//! A scripted provider for offline tests and future UI fixtures.

use super::provider::{
    BillingClass, Cancel, Capabilities, DataClass, Model, Provider, ProviderError, RoundRequest,
    StreamEvent,
};
use futures_util::Stream;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{Mutex, MutexGuard, PoisonError},
    task::{Context, Poll},
};

pub const MODEL: &str = "test-model";
pub const CAPABILITIES: Capabilities = Capabilities {
    streaming: true,
    tools: true,
    streaming_tools: true,
    parallel_tools: false,
    max_output_tokens: None,
    usage: true,
    model_listing: true,
    data_class: DataClass::Local,
    billing: BillingClass::NoPerTokenCharge,
};

pub struct FakeProvider {
    caps: Capabilities,
    script: Mutex<Script>,
}

opaque_debug!(FakeProvider, FakeRound);

struct Script {
    requests: Vec<RoundRequest>,
    rounds: VecDeque<Result<FakeRound, ProviderError>>,
}

impl FakeProvider {
    #[must_use]
    pub fn new(
        caps: Capabilities,
        rounds: impl IntoIterator<Item = Result<FakeRound, ProviderError>>,
    ) -> Self {
        Self {
            caps,
            script: Mutex::new(Script {
                rounds: rounds.into_iter().collect(),
                requests: Vec::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Script> {
        self.script.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[must_use]
    pub fn requests(&self) -> Vec<RoundRequest> {
        self.lock().requests.clone()
    }
}

impl Provider for FakeProvider {
    type Stream = FakeRound;

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        if self.caps.model_listing {
            Ok(vec![Model {
                id: MODEL.into(),
                label: "Test model".into(),
            }])
        } else {
            Err(ProviderError::Unsupported)
        }
    }

    fn capabilities(&self, _: &str) -> Capabilities {
        self.caps
    }

    async fn check_connection(&self) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn stream_turn(
        &self,
        request: RoundRequest,
        cancel: Cancel,
    ) -> Result<Self::Stream, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Incomplete);
        }
        let mut script = self.lock();
        script.requests.push(request);
        script
            .rounds
            .pop_front()
            .unwrap_or(Err(ProviderError::Unavailable))
    }
}

pub struct FakeRound {
    events: VecDeque<Result<StreamEvent, ProviderError>>,
    stall: bool,
}

impl FakeRound {
    #[must_use]
    pub fn new(events: impl IntoIterator<Item = Result<StreamEvent, ProviderError>>) -> Self {
        Self {
            events: events.into_iter().collect(),
            stall: false,
        }
    }

    /// Simulate a connection that stops producing bytes without closing.
    #[must_use]
    pub fn stall(mut self) -> Self {
        self.stall = true;
        self
    }
}

impl Stream for FakeRound {
    type Item = Result<StreamEvent, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.events.pop_front() {
            None if self.stall => Poll::Pending,
            next => Poll::Ready(next),
        }
    }
}
