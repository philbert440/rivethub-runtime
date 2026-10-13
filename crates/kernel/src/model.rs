use crate::BoxError;
use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::mpsc::Receiver;
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
