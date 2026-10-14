use std::time::Duration;

use kernel::model::Embedding;
use thiserror::Error;
use wire::stream::openai::EmbeddingRequest;
use wire::stream::openai::EmbeddingResponse;
use wire::stream::openai::EmbeddingRow;

use crate::error::BuildError;
use crate::error::StatusError;

#[derive(Debug)]
pub struct EmbedConfig {
    pub endpoint: String,
    pub api_prefix: Option<String>,
    pub model: String,
    pub api_key: Option<String>,
    pub expected_dims: Option<u32>,
    pub timeout_ms: Option<u64>,
    pub query_instruction: Option<String>,
}

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("the embeddings request failed: {source}")]
    Transport {
        #[source]
        source: reqwest::Error,
    },
    #[error(transparent)]
    Status(#[from] StatusError),
    #[error("the embeddings response is malformed: {source}")]
    Malformed {
        #[source]
        source: serde_json::Error,
    },
    #[error("embedding index {index} is outside the {count} inputs")]
    IndexOutOfRange { index: usize, count: usize },
    #[error("embedding index {index} appears more than once")]
    DuplicateIndex { index: usize },
    #[error("embedding {index} is missing from the model server response")]
    Missing { index: usize },
    #[error(
        "embedding {index} carries {got} dimensions, the configured embed_expected_dims is {expected}"
    )]
    Dimensions {
        index: usize,
        got: usize,
        expected: usize,
    },
}

#[derive(Debug)]
pub struct Embedder {
    url: String,
    model: String,
    api_key: Option<String>,
    dimensions: Option<usize>,
    query_instruction: Option<String>,
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
        let dimensions = config
            .expected_dims
            .map(usize::try_from)
            .transpose()
            .map_err(|source| BuildError::EmbedDimensions { source })?;
        let mut builder = reqwest::Client::builder();
        if let Some(timeout_ms) = config.timeout_ms {
            builder = builder.timeout(Duration::from_millis(timeout_ms));
        }
        let client = builder
            .build()
            .map_err(|source| BuildError::HttpClient { source })?;
        let url = crate::body::embeddings_url(
            &config.endpoint,
            &crate::body::normalize_api_prefix(config.api_prefix.as_deref()),
        );
        Ok(Self {
            url,
            model: config.model,
            api_key: config.api_key,
            dimensions,
            query_instruction: config.query_instruction,
            client,
        })
    }

    #[must_use]
    pub fn query_instruction(&self) -> Option<&str> {
        self.query_instruction.as_deref()
    }

    pub async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, EmbedError> {
        let count = texts.len();
        let mut request = self.client.post(&self.url).json(&EmbeddingRequest {
            model: self.model.clone(),
            input: texts,
        });
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map_err(|source| EmbedError::Transport { source })?;
        let body = crate::check_status(response)
            .await?
            .bytes()
            .await
            .map_err(|source| EmbedError::Transport { source })?;
        let response: EmbeddingResponse =
            serde_json::from_slice(&body).map_err(|source| EmbedError::Malformed { source })?;
        place(response.data, count, self.dimensions)
    }
}

fn place(
    rows: Vec<EmbeddingRow>,
    count: usize,
    dimensions: Option<usize>,
) -> Result<Vec<Embedding>, EmbedError> {
    let mut slots: Vec<Option<Vec<f32>>> = vec![None; count];
    for row in rows {
        let index = row.index;
        if let Some(expected) = dimensions
            && row.embedding.len() != expected
        {
            return Err(EmbedError::Dimensions {
                index,
                got: row.embedding.len(),
                expected,
            });
        }
        let slot = slots
            .get_mut(index)
            .ok_or(EmbedError::IndexOutOfRange { index, count })?;
        if slot.replace(row.embedding).is_some() {
            return Err(EmbedError::DuplicateIndex { index });
        }
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(index, slot)| {
            slot.map(|vector| Embedding { vector })
                .ok_or(EmbedError::Missing { index })
        })
        .collect()
}
