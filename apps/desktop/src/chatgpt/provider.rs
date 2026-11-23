use super::{CloudProvider, Events};
use todo_client::assistant::{
    local::LocalProvider,
    provider::{Cancel, Capabilities, Model, Provider, ProviderError, RoundRequest},
};

pub enum DesktopProvider {
    Private(LocalProvider),
    ChatGPT(CloudProvider),
}

impl Provider for DesktopProvider {
    type Stream = Events;
    fn capabilities(&self, model: &str) -> Capabilities {
        match self {
            Self::Private(p) => p.capabilities(model),
            Self::ChatGPT(p) => p.capabilities(model),
        }
    }
    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        match self {
            Self::Private(p) => p.list_models().await,
            Self::ChatGPT(p) => p.list_models().await,
        }
    }
    async fn check_connection(&self) -> Result<(), ProviderError> {
        match self {
            Self::Private(p) => p.check_connection().await,
            Self::ChatGPT(p) => p.check_connection().await,
        }
    }
    async fn stream_turn(
        &self,
        request: RoundRequest,
        cancel: Cancel,
    ) -> Result<Self::Stream, ProviderError> {
        match self {
            Self::Private(p) => Ok(Box::pin(p.stream_turn(request, cancel).await?)),
            Self::ChatGPT(p) => p.stream_turn(request, cancel).await,
        }
    }
}
