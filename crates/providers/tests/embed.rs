#[path = "common/mod.rs"]
mod common;

use common::fixed_status_server;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use providers::embed::EmbedConfig;
use providers::embed::EmbedRequestBody;
use providers::embed::Embedder;
use providers::embed_wire::Cell;
use providers::embed_wire::EmbedPayload;
use providers::embed_wire::normalize_embed_vector;
use providers::embed_wire::parse_embed_response;
use serde::Deserialize;
use serde::Serialize;
use testkit::Json;
use testkit::Thrown;

#[expect(
    clippy::expect_used,
    reason = "test helper; the clippy.toml exemption only reaches #[test] functions"
)]
fn vectors(rel: &str) -> Vec<testkit::cases::DiffCase> {
    let path = testkit::recording(rel);
    testkit::cases::cases(&path).expect("the diff vectors load")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildIn {
    endpoint: String,
    model: String,
    input: Vec<String>,
}

#[derive(Serialize)]
struct BuildOut {
    url: String,
    body: EmbedRequestBody,
}

fn openai_row(case: &testkit::cases::DiffCase) -> bool {
    case.input
        .get("wireShape")
        .and_then(Json::as_str)
        .is_some_and(|shape| shape == "openai")
}

#[test]
fn build_embed_request_matches_the_openai_rows() {
    let cases = vectors("providers/diff/build_embed_request.jsonl");
    assert_eq!(cases.len(), 80);
    let openai = cases
        .iter()
        .filter(|case| openai_row(case))
        .collect::<Vec<_>>();
    assert_eq!(openai.len(), 33);
    for case in &openai {
        testkit::cases::replay(case, |function, input| {
            if function != "build_embed_request" {
                return Err(Thrown("unexpected function".to_owned()));
            }
            let case: BuildIn =
                serde_json::from_value(input.clone()).map_err(|error| Thrown(error.to_string()))?;
            let out = BuildOut {
                url: providers::body::embeddings_url(
                    &case.endpoint,
                    &providers::body::normalize_api_prefix(None),
                ),
                body: EmbedRequestBody {
                    model: case.model,
                    input: case.input,
                },
            };
            serde_json::to_value(&out).map_err(|error| Thrown(error.to_string()))
        })
        .expect("the embed request vector replays");
    }
}

#[derive(Serialize)]
struct ParseOut {
    vectors: Vec<Option<Vec<f64>>>,
}

#[test]
fn parse_embed_response_matches_the_recorded_vectors() {
    let cases = vectors("providers/diff/parse_embed_response.jsonl");
    assert_eq!(cases.len(), 200);
    for case in &cases {
        testkit::cases::replay(case, |function, input| {
            if function != "parse_embed_response" {
                return Err(Thrown("unexpected function".to_owned()));
            }
            let (payload, count): (Option<EmbedPayload>, usize) =
                serde_json::from_value(input.clone()).map_err(|error| Thrown(error.to_string()))?;
            let out = ParseOut {
                vectors: parse_embed_response(payload.as_ref(), count),
            };
            serde_json::to_value(&out).map_err(|error| Thrown(error.to_string()))
        })
        .expect("the embed response vector replays");
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NormalizeOpts {
    expected_dims: Option<u32>,
    truncate_dims: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum NormalizeIn {
    WithOpts((Option<Vec<Option<Cell>>>, NormalizeOpts)),
    VectorOnly((Option<Vec<Option<Cell>>>,)),
}

#[test]
fn normalize_embed_vector_matches_the_recorded_vectors() {
    let cases = vectors("providers/diff/normalize_embed_vector.jsonl");
    assert_eq!(cases.len(), 120);
    for case in &cases {
        testkit::cases::replay(case, |function, input| {
            if function != "normalize_embed_vector" {
                return Err(Thrown("unexpected function".to_owned()));
            }
            let (vector, opts) = match serde_json::from_value::<NormalizeIn>(input.clone())
                .map_err(|error| Thrown(error.to_string()))?
            {
                NormalizeIn::WithOpts((vector, opts)) => (vector, opts),
                NormalizeIn::VectorOnly((vector,)) => (vector, NormalizeOpts::default()),
            };
            serde_json::to_value(normalize_embed_vector(
                vector.as_deref(),
                opts.expected_dims,
                opts.truncate_dims,
            ))
            .map_err(|error| Thrown(error.to_string()))
        })
        .expect("the normalize vector replays");
    }
}

const EIGHT_DIMS: &str = "{\"object\":\"list\",\"data\":[{\"object\":\"embedding\",\"embedding\":[0.544317,-0.861921,-0.400774,0.1,-0.2,0.3,0.7,-0.9],\"index\":0}]}";

#[expect(
    clippy::expect_used,
    reason = "test helper; the clippy.toml exemption only reaches #[test] functions"
)]
fn embedder(addr: std::net::SocketAddr) -> Embedder {
    Embedder::build(EmbedConfig {
        endpoint: format!("http://{addr}"),
        api_prefix: None,
        model: "bge-m3".to_owned(),
        api_key: Some("embed-key".to_owned()),
        expected_dims: Some(8),
        timeout_ms: Some(5_000),
        query_instruction: Some("Instruct: retrieve\nQuery: ".to_owned()),
    })
    .expect("the embedder builds")
}

#[tokio::test]
async fn embed_speaks_the_openai_embeddings_shape() {
    let server = fixed_status_server(200, EIGHT_DIMS.to_owned()).await;
    let embedder = embedder(server.addr);
    let out = embedder
        .embed(vec!["hello world".to_owned()])
        .await
        .expect("the embed succeeds");
    let values = out
        .iter()
        .flat_map(|embedding| embedding.vector.iter().copied())
        .collect::<Vec<_>>();
    let recorded = [
        0.544_317_f32,
        -0.861_921,
        -0.400_774,
        0.1,
        -0.2,
        0.3,
        0.7,
        -0.9,
    ];
    assert_eq!(values, recorded);
    let call = server
        .recorder
        .lock()
        .first()
        .cloned()
        .expect("the embed call is captured");
    assert_eq!(call.method, "POST");
    assert_eq!(call.target, "/v1/embeddings");
    assert_eq!(call.authorization.as_deref(), Some("Bearer embed-key"));
    let sent: Json = serde_json::from_str(&call.body).expect("sent body parses");
    assert!(testkit::json_eq(
        &sent,
        &serde_json::from_str::<Json>("{\"model\":\"bge-m3\",\"input\":[\"hello world\"]}")
            .expect("expected body parses")
    ));
}

#[tokio::test]
async fn embed_honours_the_api_prefix() {
    let server = fixed_status_server(200, EIGHT_DIMS.to_owned()).await;
    let embedder = Embedder::build(EmbedConfig {
        endpoint: format!("http://{}/", server.addr),
        api_prefix: Some("openai/v1".to_owned()),
        model: "bge-m3".to_owned(),
        api_key: None,
        expected_dims: None,
        timeout_ms: None,
        query_instruction: None,
    })
    .expect("the embedder builds");
    embedder
        .embed(vec!["hello".to_owned()])
        .await
        .expect("the embed succeeds");
    let call = server
        .recorder
        .lock()
        .first()
        .cloned()
        .expect("the embed call is captured");
    assert_eq!(call.target, "/openai/v1/embeddings");
}

#[tokio::test]
async fn embed_without_a_key_sends_no_auth_header() {
    let server = fixed_status_server(200, EIGHT_DIMS.to_owned()).await;
    let embedder = Embedder::build(EmbedConfig {
        endpoint: format!("http://{}", server.addr),
        api_prefix: None,
        model: "bge-m3".to_owned(),
        api_key: None,
        expected_dims: None,
        timeout_ms: None,
        query_instruction: None,
    })
    .expect("the embedder builds");
    embedder
        .embed(vec!["hello".to_owned()])
        .await
        .expect("the embed succeeds");
    let call = server
        .recorder
        .lock()
        .first()
        .cloned()
        .expect("the embed call is captured");
    assert_eq!(call.authorization, None);
}

#[tokio::test]
async fn a_dims_mismatch_is_an_error() {
    let server = fixed_status_server(200, EIGHT_DIMS.to_owned()).await;
    let embedder = Embedder::build(EmbedConfig {
        endpoint: format!("http://{}", server.addr),
        api_prefix: None,
        model: "bge-m3".to_owned(),
        api_key: None,
        expected_dims: Some(1024),
        timeout_ms: None,
        query_instruction: None,
    })
    .expect("the embedder builds");
    let error = embedder
        .embed(vec!["hello".to_owned()])
        .await
        .expect_err("the dims mismatch fails");
    assert!(
        error
            .to_string()
            .contains("the configured embed_expected_dims is 1024"),
        "the error names the configured dims: {error}"
    );
}

#[tokio::test]
async fn a_missing_embedding_slot_is_an_error() {
    let server = fixed_status_server(
        200,
        "{\"data\":[{\"embedding\":null,\"index\":0}]}".to_owned(),
    )
    .await;
    let embedder = Embedder::build(EmbedConfig {
        endpoint: format!("http://{}", server.addr),
        api_prefix: None,
        model: "bge-m3".to_owned(),
        api_key: None,
        expected_dims: None,
        timeout_ms: None,
        query_instruction: None,
    })
    .expect("the embedder builds");
    let error = embedder
        .embed(vec!["hello".to_owned()])
        .await
        .expect_err("the missing vector fails");
    assert!(
        error
            .to_string()
            .contains("embedding 0 is missing from the model server response"),
        "the error names the missing slot: {error}"
    );
}

#[tokio::test]
async fn the_embedder_is_not_a_chat_provider() {
    let server = fixed_status_server(200, EIGHT_DIMS.to_owned()).await;
    let embedder = embedder(server.addr);
    let request = kernel::model::ModelRequest {
        model: "bge-m3".to_owned(),
        messages: Vec::new(),
        tools: Vec::new(),
        thinking: None,
        max_tokens: None,
    };
    let error = embedder
        .chat(request.clone())
        .await
        .expect_err("the embedder refuses chat");
    assert!(matches!(error, ModelProviderError::Request { .. }));
    let error = embedder
        .stream(request)
        .await
        .expect_err("the embedder refuses stream");
    assert!(matches!(error, ModelProviderError::Request { .. }));
    let error = embedder
        .list_models()
        .await
        .expect_err("the embedder refuses list_models");
    assert!(matches!(error, ModelProviderError::Request { .. }));
    assert_eq!(
        embedder.query_instruction(),
        Some("Instruct: retrieve\nQuery: ")
    );
}

#[tokio::test]
async fn the_chat_provider_is_not_an_embedder() {
    let server = fixed_status_server(200, EIGHT_DIMS.to_owned()).await;
    let provider = config::provider::OpenaiProvider {
        model: Some("test-model".to_owned()),
        base_url: Some(format!("http://{}", server.addr)),
        ..config::provider::OpenaiProvider::default()
    };
    let built = providers::OpenAiCompatible::build(config::provider::ProviderType::Vllm, &provider)
        .expect("the provider builds");
    let error = built
        .embed(vec!["hello".to_owned()])
        .await
        .expect_err("the chat provider refuses embed");
    assert!(matches!(error, ModelProviderError::Request { .. }));
}
