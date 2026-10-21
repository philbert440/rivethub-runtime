use std::sync::Arc;

use kernel::model::{Embedding, ModelProviderError};
use providers::cli::CliError;
use rivethub::models::ModelFactory;
use tokio_util::sync::CancellationToken;

use super::{TestResult, configured, harness};

#[tokio::test]
async fn absent_embedder_is_only_an_error_for_embedding_requests() -> TestResult {
    let home = tempfile::tempdir()?;
    let (config, _) = configured(home.path(), config::ProviderType::CodexCli, "")?;
    let factory =
        ModelFactory::default().with_harness("p".into(), Arc::new(harness::Harness::default()));
    let binding = factory.build(&config, "p", None, &CancellationToken::new())?;
    assert_eq!(binding.model, "provider-model");
    assert!(!binding.embeddings);
    let error = binding
        .provider
        .embed(vec!["input".into()])
        .await
        .expect_err("no embedder");
    let ModelProviderError::Stream { source, .. } = error else {
        panic!("typed CLI error");
    };
    assert!(matches!(
        source.downcast_ref::<CliError>(),
        Some(CliError::MissingEmbedder)
    ));
    Ok(())
}

#[tokio::test]
async fn embeddings_use_the_selected_memory_backend_endpoint_model_and_key() -> TestResult {
    async fn embed(
        headers: axum::http::HeaderMap,
        axum::Json(request): axum::Json<wire::stream::openai::EmbeddingRequest>,
    ) -> axum::Json<wire::stream::openai::EmbeddingResponse> {
        assert_eq!(
            headers.get("authorization").expect("key"),
            "Bearer test-embed-key"
        );
        assert_eq!(request.model, "embedding-model");
        assert_eq!(request.input, vec!["input"]);
        axum::Json(wire::stream::openai::EmbeddingResponse {
            data: vec![wire::stream::openai::EmbeddingRow {
                index: 0,
                embedding: vec![0.25, 0.5],
            }],
        })
    }
    let server = testkit::stub_server(
        axum::Router::new().route("/v1/embeddings", axum::routing::post(embed)),
    )
    .await?;
    for backend in [
        config::MemoryBackend::Postgres,
        config::MemoryBackend::Sqlite,
    ] {
        let home = tempfile::tempdir()?;
        let path = match backend {
            config::MemoryBackend::Sqlite => "    path: ':memory:'\n",
            config::MemoryBackend::Postgres => "",
        };
        let extra = format!(
            "memory:\n  backend: {backend}\n  {backend}:\n    embed_endpoint: http://{}\n    embed_model: embedding-model\n    embed_api_key: test-embed-key\n{path}",
            server.addr
        );
        let (config, _) = configured(home.path(), config::ProviderType::ClaudeCli, &extra)?;
        let factory =
            ModelFactory::default().with_harness("p".into(), Arc::new(harness::Harness::default()));
        let cancel = CancellationToken::new();
        let binding = factory.build(&config, "p", None, &cancel)?;
        assert!(binding.embeddings);
        assert_eq!(
            binding.provider.embed(vec!["input".into()]).await?,
            vec![Embedding {
                vector: vec![0.25, 0.5]
            }]
        );
        cancel.cancel();
    }
    server.close().await?;
    Ok(())
}

#[tokio::test]
async fn an_openai_provider_embeds_through_the_memory_endpoint_not_its_chat_server() -> TestResult {
    async fn embed(
        axum::Json(request): axum::Json<wire::stream::openai::EmbeddingRequest>,
    ) -> axum::Json<wire::stream::openai::EmbeddingResponse> {
        assert_eq!(request.model, "embedding-model");
        axum::Json(wire::stream::openai::EmbeddingResponse {
            data: vec![wire::stream::openai::EmbeddingRow {
                index: 0,
                embedding: vec![0.25, 0.5],
            }],
        })
    }
    let chat = testkit::stub_server(axum::Router::new()).await?;
    let embeddings = testkit::stub_server(
        axum::Router::new().route("/v1/embeddings", axum::routing::post(embed)),
    )
    .await?;
    for backend in [
        config::MemoryBackend::Postgres,
        config::MemoryBackend::Sqlite,
    ] {
        let home = tempfile::tempdir()?;
        let path = match backend {
            config::MemoryBackend::Sqlite => "    path: ':memory:'\n",
            config::MemoryBackend::Postgres => "",
        };
        let text = format!(
            "runtime:\n  workspace: {home}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://{chat}\n    model: chat-model\n    probe_models: false\nmemory:\n  backend: {backend}\n  {backend}:\n    embed_endpoint: http://{embed}\n    embed_model: embedding-model\n{path}",
            home = home.path().display(),
            chat = chat.addr,
            embed = embeddings.addr,
        );
        let env = config::Environment::default();
        let config = config::Config::from_str(&env, &text)?.config;
        let cancel = CancellationToken::new();
        let binding = ModelFactory::default().build(&config, "p", None, &cancel)?;
        assert!(binding.embeddings);
        assert_eq!(
            binding.provider.embed(vec!["input".into()]).await?,
            vec![Embedding {
                vector: vec![0.25, 0.5]
            }],
            "{backend}"
        );
        cancel.cancel();
    }
    chat.close().await?;
    embeddings.close().await?;
    Ok(())
}

#[tokio::test]
async fn the_memory_embedder_authenticates_with_the_embed_provider_token_command() -> TestResult {
    async fn embed(
        headers: axum::http::HeaderMap,
        axum::Json(request): axum::Json<wire::stream::openai::EmbeddingRequest>,
    ) -> axum::Json<wire::stream::openai::EmbeddingResponse> {
        assert_eq!(
            headers.get("authorization").expect("bearer"),
            "Bearer minted-embed-token"
        );
        assert_eq!(request.model, "embedding-model");
        axum::Json(wire::stream::openai::EmbeddingResponse {
            data: vec![wire::stream::openai::EmbeddingRow {
                index: 0,
                embedding: vec![0.25, 0.5],
            }],
        })
    }
    let chat = testkit::stub_server(axum::Router::new()).await?;
    let embeddings = testkit::stub_server(
        axum::Router::new().route("/v1/embeddings", axum::routing::post(embed)),
    )
    .await?;
    for backend in [
        config::MemoryBackend::Postgres,
        config::MemoryBackend::Sqlite,
    ] {
        let home = tempfile::tempdir()?;
        let path = match backend {
            config::MemoryBackend::Sqlite => "    path: ':memory:'\n",
            config::MemoryBackend::Postgres => "",
        };
        let text = format!(
            "runtime:\n  workspace: {home}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://{chat}\n    model: chat-model\n    probe_models: false\n  embedder:\n    type: vllm\n    base_url: http://{embed}\n    model: served-model\n    token_command:\n      - /bin/echo\n      - minted-embed-token\nmemory:\n  backend: {backend}\n  {backend}:\n    embed_provider: embedder\n    embed_model: embedding-model\n{path}",
            home = home.path().display(),
            chat = chat.addr,
            embed = embeddings.addr,
        );
        let env = config::Environment::default();
        let config = config::Config::from_str(&env, &text)?.config;
        let cancel = CancellationToken::new();
        let binding = ModelFactory::default().build(&config, "p", None, &cancel)?;
        assert!(binding.embeddings, "{backend}");
        assert_eq!(
            binding.provider.embed(vec!["input".into()]).await?,
            vec![Embedding {
                vector: vec![0.25, 0.5]
            }],
            "{backend}"
        );
        cancel.cancel();
    }
    chat.close().await?;
    embeddings.close().await?;
    Ok(())
}
