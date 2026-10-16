use std::ffi::OsStr;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use wire::gateway::v1::{
    ChatRequest, Completion, CompletionChunk, Conversations, ErrorKind, ErrorResponse, Models,
};
use wire::message::Role;
use wire::stream::openai::{ChatCompletionRequest, FinishReason, Message};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
struct Seen {
    models: Arc<Mutex<Vec<String>>>,
    body: String,
}

async fn provider(content: &str) -> TestResult<(testkit::Stub, Arc<Mutex<Vec<String>>>)> {
    let models = Arc::new(Mutex::new(Vec::new()));
    let seen = Seen {
        models: Arc::clone(&models),
        body: frame(content)?,
    };
    let router = axum::Router::new()
        .route("/v1/chat/completions", axum::routing::post(accept))
        .with_state(seen);
    Ok((testkit::stub_server(router).await?, models))
}

async fn accept(
    axum::extract::State(seen): axum::extract::State<Seen>,
    axum::Json(request): axum::Json<ChatCompletionRequest>,
) -> ([(axum::http::HeaderName, &'static str); 1], String) {
    seen.models.lock().await.push(request.model);
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        seen.body,
    )
}

fn frame(content: &str) -> TestResult<String> {
    let chunk = CompletionChunk {
        id: "test-completion".into(),
        created: 0,
        model: "provider-model".into(),
        choices: vec![wire::gateway::v1::ChunkChoice {
            index: 0,
            delta: wire::gateway::v1::CompletionDelta {
                content: Some(content.into()),
                ..wire::gateway::v1::CompletionDelta::default()
            },
            finish_reason: Some(FinishReason::Stop),
        }],
        usage: None,
    };
    Ok(format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::to_string(&chunk)?
    ))
}

fn configured(
    home: &std::path::Path,
    alpha: SocketAddr,
    beta: SocketAddr,
) -> TestResult<(config::Config, config::Environment)> {
    let env = config::Environment::from_vars([(OsStr::new("HOME"), home.as_os_str())]);
    let text = format!(
        "runtime:\n  workspace: {home}\n  default_agent: alpha\nagents:\n  alpha:\n    provider: alpha-provider\n    model: alpha-model\n  beta:\n    provider: beta-provider\nproviders:\n  alpha-provider:\n    type: openai-compatible\n    base_url: http://{alpha}\n    model: ignored-alpha\n    probe_models: false\n  beta-provider:\n    type: openai-compatible\n    base_url: http://{beta}\n    model: beta-model\n    probe_models: false\nmesh:\n  enabled: false\n  node_name: test-node\ntasks:\n  enabled: false\n",
        home = home.display(),
    );
    let path = home.join("config.yaml");
    std::fs::write(&path, text)?;
    Ok((
        config::Config::load(
            &env,
            &config::ConfigLocation::Current(path),
            config::Profile::Node,
        )?
        .config,
        env,
    ))
}

fn request(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.to_owned(),
        messages: vec![Message {
            role: Role::User,
            content: Some("hello".to_owned()),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        stream: None,
        max_tokens: None,
        stream_options: None,
    }
}

async fn post(
    client: &reqwest::Client,
    address: SocketAddr,
    model: &str,
) -> TestResult<Completion> {
    Ok(client
        .post(format!("http://{address}/v1/chat/completions"))
        .json(&request(model))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn taken(models: &Mutex<Vec<String>>) -> Vec<String> {
    std::mem::take(&mut *models.lock().await)
}

async fn assert_unknown(client: &reqwest::Client, address: SocketAddr, model: &str) -> TestResult {
    let response = client
        .post(format!("http://{address}/v1/chat/completions"))
        .json(&request(model))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let error: ErrorResponse = response.json().await?;
    assert_eq!(error.error.kind, ErrorKind::InvalidRequestError);
    assert_eq!(
        error.error.message,
        format!("Unknown agent: \"{model}\". Registered: [alpha,beta]")
    );
    Ok(())
}

#[tokio::test]
async fn each_agent_id_reaches_its_provider_model() -> TestResult {
    let (alpha, alpha_models) = provider("alpha-says").await?;
    let (beta, beta_models) = provider("beta-says").await?;
    let home = tempfile::tempdir()?;
    let (config, env) = configured(home.path(), alpha.addr, beta.addr)?;
    let cancel = tokio_util::sync::CancellationToken::new();
    let composition = rivethub::composition::Composition::with_models(
        &config,
        &env,
        Arc::new(testkit::FakeMemoryStore::default()),
        &cancel,
        &rivethub::models::ModelFactory::default(),
    )
    .await?;
    let server = testkit::stub_server(composition.router(false).layer(axum::Extension(
        axum::extract::ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))),
    )))
    .await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    assert_unknown(&client, server.addr, "nope").await?;
    assert_unknown(&client, server.addr, "ignored-alpha").await?;
    let records: Conversations = client
        .get(format!("http://{}/api/conversations", server.addr))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(records.conversations, Vec::new());
    let reply = post(&client, server.addr, "alpha").await?;
    assert_eq!(reply.model, "alpha");
    assert_eq!(
        reply
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref()),
        Some("alpha-says")
    );
    assert_eq!(taken(&alpha_models).await, vec!["alpha-model".to_owned()]);
    assert_eq!(taken(&beta_models).await, Vec::<String>::new());
    let reply = post(&client, server.addr, "beta").await?;
    assert_eq!(reply.model, "beta");
    assert_eq!(
        reply
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref()),
        Some("beta-says")
    );
    assert_eq!(taken(&beta_models).await, vec!["beta-model".to_owned()]);
    assert_eq!(taken(&alpha_models).await, Vec::<String>::new());
    let models: Models = client
        .get(format!("http://{}/v1/models", server.addr))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        models
            .data
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    assert!(
        models
            .data
            .iter()
            .all(|model| model.created == 0 && model.owned_by == "rivetos")
    );
    server.close().await?;
    tokio::time::timeout(Duration::from_secs(5), composition.close()).await??;
    alpha.close().await?;
    beta.close().await?;
    Ok(())
}
