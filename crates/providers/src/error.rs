use kernel::model::ModelProviderError;
use reqwest::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("provider type is not an OpenAI-compatible model server")]
    NotOpenaiCompatible,
    #[error("provider is missing \"model\"")]
    MissingModel,
    #[error("provider is missing \"base_url\"")]
    MissingBaseUrl,
    #[error("provider \"base_url\" is not a valid URL: {source}")]
    BaseUrl {
        #[source]
        source: url::ParseError,
    },
    #[error("provider \"base_url\" must be an http(s) URL")]
    BaseUrlScheme,
    #[error("the HTTP client could not be built: {source}")]
    HttpClient {
        #[source]
        source: reqwest::Error,
    },
    #[error("embeddings are missing \"embed_endpoint\"")]
    MissingEmbedEndpoint,
    #[error("embeddings are missing \"embed_model\"")]
    MissingEmbedModel,
    #[error("\"embed_expected_dims\" does not fit a vector length on this platform: {source}")]
    EmbedDimensions {
        #[source]
        source: std::num::TryFromIntError,
    },
}

#[derive(Debug, Error)]
pub enum StatusError {
    #[error("model server returned {status}: {body}")]
    Body { status: StatusCode, body: String },
    #[error("model server returned {status}, and the body was unreadable")]
    Unreadable {
        status: StatusCode,
        #[source]
        source: reqwest::Error,
    },
}

#[derive(Debug, Error)]
pub enum StreamFault {
    #[error("the model server restarted tool call index {index}")]
    RestartedToolCall { index: u32 },
    #[error("the model server streamed arguments for unknown tool call {index}")]
    UnknownToolCall { index: u32 },
    #[error("the model server sent a tool call chunk without an index")]
    ToolCallWithoutIndex,
    #[error("the model server started a tool call without an id")]
    ToolCallWithoutId,
    #[error("the model stream ended before Done")]
    EndedBeforeDone,
}

#[derive(Debug, Error)]
pub enum Unsupported {
    #[error(
        "message content parts are not spoken to the model server: video URL splicing is out of \
         scope and no recording pins an image shape"
    )]
    ContentParts,
    #[error(
        "the provider config has no embeddings endpoint; embeddings are configured under memory \
         (embed_endpoint)"
    )]
    Embeddings,
}

impl From<StatusError> for ModelProviderError {
    fn from(error: StatusError) -> Self {
        Self::Request {
            message: error.to_string(),
            source: Box::new(error),
        }
    }
}

impl From<StreamFault> for ModelProviderError {
    fn from(fault: StreamFault) -> Self {
        Self::Stream {
            message: fault.to_string(),
            source: Box::new(fault),
        }
    }
}

impl From<Unsupported> for ModelProviderError {
    fn from(error: Unsupported) -> Self {
        Self::Request {
            message: error.to_string(),
            source: Box::new(error),
        }
    }
}
