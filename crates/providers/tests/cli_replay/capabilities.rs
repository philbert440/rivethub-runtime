use std::sync::Arc;
use std::time::Duration;

use config::provider::{CliProvider, ProviderType};
use kernel::harness::SessionEvent;
use kernel::model::{ModelChunk, ModelProvider};
use tokio_util::sync::CancellationToken;
use wire::harness::{
    HarnessCapabilities, HarnessControlCapabilities, HarnessId, HarnessModelOption,
    HarnessObservationCapabilities, HarnessOffering,
};

use crate::cli_support::{
    Fault, Harness, Plan, TestResult, action, embedder, items, provider, request,
};

#[tokio::test]
async fn model_discovery_uses_the_selected_harness_and_propagates_errors() -> TestResult {
    let (mut harness, actions) = Harness::new(Vec::new());
    harness.offerings = vec![
        offering(HarnessId::Codex, "discovered-model"),
        offering(HarnessId::ClaudeCode, "unrelated-model"),
    ];
    let provider = provider(
        Arc::new(harness),
        CliProvider {
            model: Some("configured-model".into()),
            ..CliProvider::default()
        },
        &CancellationToken::new(),
    );
    assert_eq!(
        provider.list_models().await?,
        vec!["configured-model", "discovered-model"]
    );
    provider.shutdown().await;
    assert!(
        provider
            .list_models()
            .await
            .expect_err("closed provider")
            .to_string()
            .contains("shutting down")
    );
    drop(actions);
    let (mut harness, actions) = Harness::new(Vec::new());
    harness.fault = Some(Fault::Offerings);
    let provider = crate::cli_support::provider(
        Arc::new(harness),
        CliProvider::default(),
        &CancellationToken::new(),
    );
    assert!(provider.list_models().await.is_err());
    provider.shutdown().await;
    drop(actions);
    Ok(())
}

fn offering(harness_id: HarnessId, model: &str) -> HarnessOffering {
    HarnessOffering {
        harness_id,
        chat: true,
        capabilities: HarnessCapabilities {
            control: HarnessControlCapabilities {
                interrupt: true,
                resume: true,
                approvals: true,
                steering: None,
            },
            observation: HarnessObservationCapabilities {
                live_stream: true,
                list_sessions: false,
                drive: None,
            },
            turn_options: None,
            launch_model: None,
            image_attachments: None,
            models: Some(vec![HarnessModelOption {
                input_modalities: None,
                id: model.into(),
                label: model.into(),
                is_default: None,
                efforts: None,
            }]),
            models_source: None,
            efforts: None,
            model_flag: None,
            effort_flag: None,
        },
    }
}

#[tokio::test]
async fn embeddings_use_the_injected_embedder() -> TestResult {
    async fn embeddings(
        axum::Json(body): axum::Json<wire::stream::openai::EmbeddingRequest>,
    ) -> axum::Json<wire::stream::openai::EmbeddingResponse> {
        assert_eq!(body.input, vec!["input"]);
        assert_eq!(body.model, "embedding-model");
        axum::Json(wire::stream::openai::EmbeddingResponse {
            data: vec![wire::stream::openai::EmbeddingRow {
                index: 0,
                embedding: vec![0.1, 0.2],
            }],
        })
    }
    let server = testkit::stub_server(
        axum::Router::new().route("/v1/embeddings", axum::routing::post(embeddings)),
    )
    .await?;
    let (harness, actions) = Harness::new(Vec::new());
    let provider = providers::cli::CliBacked::build(
        ProviderType::CodexCli,
        CliProvider::default(),
        Arc::new(harness),
        embedder(format!("http://{}", server.addr)),
        &CancellationToken::new(),
    )?;
    let vectors = provider.embed(vec!["input".into()]).await?;
    assert_eq!(
        vectors,
        vec![kernel::model::Embedding {
            vector: vec![0.1, 0.2]
        }]
    );
    provider.shutdown().await;
    assert!(
        provider
            .embed(vec!["input".into()])
            .await
            .expect_err("closed provider")
            .to_string()
            .contains("shutting down")
    );
    drop(actions);
    Ok(())
}

#[tokio::test]
async fn shutdown_interrupts_output_backpressure_and_retains_cleanup_failures() -> TestResult {
    let (mut harness, mut actions) = Harness::new(vec![Plan {
        events: (0..1000)
            .map(|_| SessionEvent::AgentMessageChunk {
                text: "chunk".into(),
            })
            .collect(),
        finish: false,
    }]);
    harness.fault = Some(Fault::Cancel);
    let harness = Arc::new(harness);
    let provider = provider(
        Arc::clone(&harness),
        CliProvider::default(),
        &CancellationToken::new(),
    );
    let mut receiver = provider.stream(request()).await?;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await?
            .is_some()
    );
    tokio::time::timeout(Duration::from_secs(5), provider.shutdown()).await?;
    let chunks = items(receiver).await;
    assert!(
        !chunks
            .iter()
            .any(|item| matches!(item, Ok(ModelChunk::Done)))
    );
    assert!(harness.is_empty());
    assert!(matches!(
        action(&mut actions).await,
        crate::cli_support::Action::New(..)
    ));
    assert!(matches!(
        action(&mut actions).await,
        crate::cli_support::Action::Prompt(..)
    ));
    assert!(matches!(
        action(&mut actions).await,
        crate::cli_support::Action::Cancel(_)
    ));
    assert!(matches!(
        action(&mut actions).await,
        crate::cli_support::Action::Release(_)
    ));
    Ok(())
}
