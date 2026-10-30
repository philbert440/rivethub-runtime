use std::sync::Arc;
use std::time::Duration;

use crate::BoxError;
use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::mpsc::Receiver;
use tokio::sync::watch;
use wire::gateway::ProviderRecovery;
use wire::message::Message;
use wire::stream::LlmUsage;
use wire::stream::ThinkingLevel;

#[derive(Debug, Error)]
pub enum ModelProviderError {
    #[error("provider request failed: {message}")]
    Request {
        message: String,
        #[source]
        source: BoxError,
    },
    #[error("provider stream ended early: {message}")]
    Stream {
        message: String,
        #[source]
        source: BoxError,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub options: ModelOptions,
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<rmcp::model::Tool>,
    pub thinking: Option<ThinkingLevel>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelChunk {
    Text {
        delta: String,
    },
    Reasoning {
        delta: String,
    },
    ToolCallStart {
        index: u32,
        id: String,
        name: String,
    },
    ToolCallDelta {
        index: u32,
        arguments_delta: String,
    },
    Usage {
        usage: LlmUsage,
    },
    Done,
}

pub type ModelStreamItem = Result<ModelChunk, ModelProviderError>;

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn chat(
        &self,
        request: ModelRequest,
    ) -> Result<wire::stream::LlmResponse, ModelProviderError>;

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<Receiver<ModelStreamItem>, ModelProviderError>;

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError>;

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError>;
}

#[derive(Debug, Clone)]
pub enum ProviderStatus {
    Ready,
    Degraded(Degradation),
}

#[derive(Debug, Clone)]
pub struct Degradation {
    pub recovery: ProviderRecovery,
    pub cause: Arc<dyn std::error::Error + Send + Sync>,
}

#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub id: String,
    pub status: watch::Receiver<ProviderStatus>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelOptions {
    pub temperature: Option<f64>,
    pub enable_thinking: Option<bool>,
    pub json_schema: Option<schemars::Schema>,
}

#[derive(Debug, Error)]
pub enum ModelCallFailure {
    #[error("model server returned {status}: {source}")]
    Http {
        status: u16,
        retry_after: Option<Duration>,
        #[source]
        source: BoxError,
    },
    #[error("model transport failed: {source}")]
    Transport {
        #[source]
        source: BoxError,
    },
    #[error("model credentials are unavailable: {source}")]
    Credentials {
        #[source]
        source: BoxError,
    },
}

impl ModelProviderError {
    #[must_use]
    pub fn failure(&self) -> Option<&ModelCallFailure> {
        let mut source = std::error::Error::source(self);
        while let Some(error) = source {
            if let Some(failure) = error.downcast_ref::<ModelCallFailure>() {
                return Some(failure);
            }
            source = error.source();
        }
        None
    }

    #[must_use]
    pub fn retryable_transport(&self) -> bool {
        match self.failure() {
            Some(ModelCallFailure::Http { status, .. }) => *status >= 500,
            Some(ModelCallFailure::Transport { .. }) => true,
            Some(ModelCallFailure::Credentials { .. }) | None => false,
        }
    }
}
