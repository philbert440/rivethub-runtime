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
