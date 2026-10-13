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
}
