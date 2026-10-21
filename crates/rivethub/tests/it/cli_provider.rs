#[path = "cli_provider/degraded.rs"]
mod degraded;
#[path = "cli_provider/embed.rs"]
mod embed;
#[path = "cli_provider/harness.rs"]
mod harness;
#[path = "cli_provider/worker.rs"]
mod worker;

use std::ffi::OsStr;
use std::sync::Arc;
use std::time::Duration;

use config::provider::ProviderType;
use kernel::harness::SessionEvent;
use rivethub::composition::Composition;
use rivethub::models::ModelFactory;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use wire::gateway::v1::{ChatRequest, Completion};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const KINDS: &[ProviderType] = &[
    ProviderType::ClaudeCli,
    ProviderType::CodexCli,
    ProviderType::GrokCli,
    ProviderType::HermesCli,
    ProviderType::KimiCode,
    ProviderType::OpencodeCli,
];
const SKIPPED: &[(&str, &str)] = &[];

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Chunk {
    Text { text: String },
    Done,
}

fn recorded() -> TestResult<(Vec<SessionEvent>, String)> {
    let chunks: Vec<Chunk> = serde_json::from_str(include_str!(
        "../../../testkit/tests/fixtures/provider/A-text/chunks.json"
    ))?;
    let mut events = Vec::new();
    let mut expected = String::new();
    for chunk in chunks {
        match chunk {
            Chunk::Text { text } => {
                expected.push_str(&text);
                events.push(SessionEvent::AgentMessageChunk { text });
            }
            Chunk::Done => events.push(SessionEvent::TurnEnded),
        }
    }
    Ok((events, expected))
}

fn configured(
    home: &std::path::Path,
    kind: ProviderType,
    extra: &str,
) -> TestResult<(config::Config, config::Environment)> {
    let env = config::Environment::from_vars([(OsStr::new("HOME"), home.as_os_str())]);
    let text = format!(
        "runtime:\n  workspace: {}\n  default_agent: main\nagents:\n  main:\n    provider: p\n    model: agent-model\nproviders:\n  p:\n    type: {kind}\n    model: provider-model\n    effort: high\n    permission_mode: plan\n    timeout_ms: 5000\n    permission_timeout_ms: 1000\ntasks:\n  enabled: false\nmesh:\n  enabled: false\n  node_name: test-node\n{extra}",
        home.display()
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

#[test]
fn every_provider_recording_is_replayed_or_explicitly_skipped() -> TestResult {
    let mut cases = Vec::new();
    for entry in std::fs::read_dir(testkit::recording("provider"))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            cases.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "fixture name")?,
            );
        }
    }
    let mut covered = vec!["A-text".to_owned()];
    for (name, reason) in SKIPPED {
        assert_ne!(*reason, "");
        covered.push((*name).to_owned());
    }
    cases.sort();
    covered.sort();
    assert_eq!(cases, covered);
    Ok(())
}

fn say_hi() -> wire::gateway::sessions::SessionPostRequest {
    wire::gateway::sessions::SessionPostRequest {
        text: "Say hi.".into(),
        system_prompt: None,
        user_id: None,
        exclude_tools: None,
        agent: None,
        thinking: None,
        attachments: None,
        harness: None,
        model: None,
        working_dir: None,
    }
}
#[tokio::test]
async fn each_cli_provider_starts_gateway_and_replays_chat_and_task_turn() -> TestResult {
    for &kind in KINDS {
        let home = tempfile::tempdir()?;
        let (config, env) = configured(home.path(), kind, "")?;
        let (events, expected) = recorded()?;
        let harness = Arc::new(harness::Harness::new(events));
        let factory = ModelFactory::default().with_harness("p".into(), harness.clone());
        let cancel = CancellationToken::new();
        let composition = Composition::with_models(
            &config,
            &env,
            Arc::new(testkit::FakeMemoryStore::default()),
            &cancel,
            &factory,
        )
        .await?;
        let server = testkit::stub_server(composition.router(false).layer(axum::Extension(
            axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 12345))),
        )))
        .await?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        let request = chat_request();
        let reply: Completion = client
            .post(format!("http://{}/v1/chat/completions", server.addr))
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(reply.model, "main");
        assert_eq!(
            reply
                .choices
                .first()
                .ok_or("completion choice")?
                .message
                .content
                .as_deref(),
            Some(expected.as_str()),
            "{kind}"
        );
        let request = say_hi();
        let reply: wire::gateway::sessions::SessionPostReply = client
            .post(format!(
                "http://{}/api/sessions/task-test/messages?wait=1",
                server.addr
            ))
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(reply.message.text, expected);
        let requests = harness.requests.lock().await;
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(request.harness, providers::cli::harness_id(kind)?);
            assert_eq!(request.model.as_deref(), Some("agent-model"));
            assert_eq!(request.effort, Some(wire::stream::ThinkingLevel::High));
            assert_eq!(request.working_dir.as_deref(), Some(home.path()));
        }
        drop(requests);
        assert_eq!(
            *harness.options.lock().await,
            vec![kernel::harness::SessionOption::Mode("plan".into()); 2]
        );
        server.close().await?;
        tokio::time::timeout(Duration::from_secs(5), composition.close()).await??;
    }
    Ok(())
}

fn chat_request() -> ChatRequest {
    ChatRequest {
        model: "main".into(),
        messages: vec![wire::stream::openai::Message {
            role: wire::message::Role::User,
            content: Some("Say hi.".into()),
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        stream: None,
        max_tokens: None,
        stream_options: None,
    }
}
