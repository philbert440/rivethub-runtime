use std::time::Duration;

use async_trait::async_trait;
use kernel::model::Embedding;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use kernel::model::ModelRequest;
use tokio::sync::mpsc::Receiver;
use wire::stream::LlmResponse;

use crate::check_status;
use crate::embed_wire::EmbedPayload;
use crate::embed_wire::NormalizeRejection;
use crate::embed_wire::parse_embed_response;
use crate::error::BuildError;
use crate::request_error;

#[derive(Debug)]
pub struct EmbedConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key: Option<String>,
    pub expected_dims: Option<u32>,
    pub timeout_ms: Option<u64>,
    pub query_instruction: Option<String>,
}

#[derive(Debug)]
pub struct Embedder {
    config: EmbedConfig,
    client: reqwest::Client,
}

impl Embedder {
    pub fn build(config: EmbedConfig) -> Result<Self, BuildError> {
        if config.endpoint.is_empty() {
            return Err(BuildError::MissingEmbedEndpoint);
        }
        if config.model.is_empty() {
            return Err(BuildError::MissingEmbedModel);
        }
        let parsed =
            url::Url::parse(&config.endpoint).map_err(|source| BuildError::BaseUrl { source })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(BuildError::BaseUrlScheme);
        }
        let mut builder = reqwest::Client::builder();
        if let Some(timeout_ms) = config.timeout_ms {
            builder = builder.timeout(Duration::from_millis(timeout_ms));
        }
        let client = builder
            .build()
            .map_err(|source| BuildError::HttpClient { source })?;
        Ok(Self { config, client })
    }

    #[must_use]
    pub fn query_instruction(&self) -> Option<&str> {
        self.config.query_instruction.as_deref()
    }

    pub async fn send(&self, texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        let count = texts.len();
        let body = EmbedRequestBody {
            model: self.config.model.clone(),
            input: texts,
        };
        let mut builder = self
            .client
            .post(crate::body::embeddings_url(&self.config.endpoint))
            .json(&body);
        if let Some(key) = &self.config.api_key {
            builder = builder.bearer_auth(key);
        }
        let response = builder.send().await.map_err(request_error)?;
        let text = match check_status(response).await {
            Ok(response) => response.text().await.map_err(request_error)?,
            Err(error) => return Err(error),
        };
        let payload: Option<EmbedPayload> =
            serde_json::from_str(&text).map_err(|source| ModelProviderError::Request {
                message: format!("the embeddings response is not valid JSON: {source}\n{text}"),
                source: Box::new(source),
            })?;
        let vectors = parse_embed_response(payload.as_ref(), count);
        let mut out = Vec::with_capacity(count);
        for (slot, vector) in vectors.into_iter().enumerate() {
            let vector = vector.ok_or_else(|| missing_slot(slot))?;
            crate::embed_wire::check_dims(vector.len(), self.config.expected_dims)
                .map_err(|rejection| embed_rejection(slot, rejection))?;
            out.push(Embedding {
                vector: to_f32(&vector),
            });
        }
        Ok(out)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "embedding storage is f32 (pgvector halfvec); the measured f64 value is narrowed on purpose"
)]
fn to_f32(values: &[f64]) -> Vec<f32> {
    values.iter().map(|value| *value as f32).collect()
}

fn missing_slot(slot: usize) -> ModelProviderError {
    ModelProviderError::Request {
        message: format!("embedding {slot} is missing from the model server response"),
        source: format!("embedding {slot}").into(),
    }
}

fn embed_rejection(slot: usize, rejection: NormalizeRejection) -> ModelProviderError {
    let reason = match rejection {
        NormalizeRejection::NotNumbers => "is not a number vector".to_owned(),
        NormalizeRejection::DimsMismatch { got, expected } => {
            format!("carries {got} dimensions, the configured embed_expected_dims is {expected}")
        }
    };
    ModelProviderError::Request {
        message: format!("embedding {slot} {reason}"),
        source: format!("embedding {slot}").into(),
    }
}

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
pub struct EmbedRequestBody {
    pub model: String,
    pub input: Vec<String>,
}

fn not_a_chat() -> ModelProviderError {
    ModelProviderError::Request {
        message: "the embeddings endpoint is not a chat provider".to_owned(),
        source: "chat on the embedder".into(),
    }
}

#[async_trait]
impl ModelProvider for Embedder {
    async fn chat(&self, _request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        Err(not_a_chat())
    }

    async fn stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Receiver<kernel::model::ModelStreamItem>, ModelProviderError> {
        Err(not_a_chat())
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        self.send(texts).await
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Err(not_a_chat())
    }
}
