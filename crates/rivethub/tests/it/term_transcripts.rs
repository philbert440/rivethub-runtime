use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kernel::memory::MemoryStore;
use kernel::shutdown::Activities;
use rivethub::composition::Composition;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use wire::gateway::GatewayError;
use wire::gateway::sessions::{HarnessToolOutput, TranscriptResponse};
use wire::harness::HarnessId;
use wire::message::Role;
use wire::session_id::{SessionId, encode_segment};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SESSION: &str = "33333333-3333-3333-3333-333333333333";

fn place(root: &Path, relative: &str, fixture: &str) -> TestResult {
    let destination = root.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(testkit::recording(fixture), &destination)?;
    Ok(())
}

fn write_config(home: &Path, work: &Path, state: &Path) -> TestResult {
    let commands = home.join("commands.json");
    std::fs::write(
        &commands,
        format!(
            r#"{{"default":"shell","cwd":"{}","commands":{{"shell":{{"label":"Shell","cmd":["/bin/true"],"room":false}}}}}}"#,
            work.display()
        ),
    )?;
    let config = format!(
        "runtime:\n  workspace: {work}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:9/v1\n    model: m\n    probe_models: false\ngateway:\n  host: 127.0.0.1\n  port: 5974\n  terminal:\n    enabled: true\n  commands_file: {commands}\n  files_root: {work}\n",
        work = work.display(),
        commands = commands.display(),
    );
    std::fs::write(state.join("config.yaml"), config)?;
    Ok(())
}

async fn compose(home: &Path, path: &str) -> TestResult<Composition> {
    let work = home.join("work");
    let state = home.join(".rivethub");
    std::fs::create_dir_all(&work)?;
    std::fs::create_dir_all(&state)?;
    write_config(home, &work, &state)?;
    let env = config::Environment::from_vars([
        ("HOME", home.to_str().ok_or("home")?),
        ("PATH", path),
        ("RIVETHUB_TERM_MUX", "none"),
    ]);
    let loaded = config::Config::load(
        &env,
        &config::ConfigLocation::Current(state.join("config.yaml")),
        config::Profile::Node,
    )?;
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    Ok(tokio::time::timeout(
        Duration::from_secs(30),
        Composition::build(
            &loaded.config,
            &env,
            store,
            &CancellationToken::new(),
            &Activities::new(),
        ),
    )
    .await??)
}

async fn get(composition: &Composition, path: &str) -> TestResult<(u16, Vec<u8>)> {
    let mut request = axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", "localhost")
        .body(axum::body::Body::empty())?;
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "127.0.0.1:1234".parse::<std::net::SocketAddr>()?,
    ));
    let response = composition.gateway().router(false).oneshot(request).await?;
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok((status, body.to_vec()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_transcripts_read_the_store_of_the_session_harness() -> TestResult {
    let home = tempfile::tempdir()?;
    place(
        home.path(),
        &format!(".codex/sessions/2026/10/14/rollout-2026-10-14T12-00-00-{SESSION}.jsonl"),
        "capture/stores/codex-rollout.jsonl",
    )?;
    let composition = compose(home.path(), "/nonexistent").await?;
    let (status, body) = get(
        &composition,
        &format!("/api/terminal/harness-sessions/codex:{SESSION}/transcript"),
    )
    .await?;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let response: TranscriptResponse = serde_json::from_slice(&body)?;
    assert_eq!(response.command, "codex");
    let turns: Vec<(Role, &str)> = response
        .turns
        .iter()
        .map(|turn| (turn.role, turn.text.as_str()))
        .collect();
    assert_eq!(
        turns,
        [
            (Role::User, "list the files"),
            (Role::Assistant, "Running ls."),
            (Role::Tool, "a.txt"),
        ]
    );
    tokio::time::timeout(Duration::from_secs(10), composition.close()).await??;
    Ok(())
}

fn tool_lines(output: &str) -> TestResult<String> {
    Ok([
        r#"{"type":"user","message":{"role":"user","content":"run the build"}}"#.to_owned(),
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_build","name":"Bash","input":{"command":"make"}},{"type":"tool_use","id":"toolu_log","name":"Bash","input":{"command":"cat build.log"}}]}}"#.to_owned(),
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_build","is_error":true,"content":"make: *** [all] Error 2"}]}}"#.to_owned(),
        format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_log","content":[{{"type":"text","text":{}}}]}}]}}}}"#,
            serde_json::to_string(output)?
        ),
    ]
    .join("\n"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_call_output_is_read_from_the_claude_session_file() -> TestResult {
    let home = tempfile::tempdir()?;
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin)?;
    let adapter = bin.join("claude-agent-acp");
    std::fs::write(&adapter, "#!/bin/sh\n")?;
    std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o755))?;
    let projects = home.path().join(".claude/projects/-home-user");
    std::fs::create_dir_all(&projects)?;
    let log = format!("{}build finished\n", "é".repeat(40 * 1024));
    std::fs::write(projects.join(format!("{SESSION}.jsonl")), tool_lines(&log)?)?;
    let composition = compose(home.path(), bin.to_str().ok_or("bin")?).await?;
    let segment = encode_segment(&SessionId::new(HarnessId::ClaudeCode, SESSION));
    let output = |tool: &str| format!("/api/harness-sessions/{segment}/tools/{tool}/output");

    let (status, body) = get(&composition, &output("toolu_build")).await?;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        serde_json::from_slice::<HarnessToolOutput>(&body)?,
        HarnessToolOutput {
            text: "make: *** [all] Error 2".to_owned(),
            is_error: true,
            truncated: false,
        }
    );

    let (status, body) = get(&composition, &output("toolu_missing")).await?;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice::<GatewayError>(&body)?;

    let (status, body) = get(&composition, &output("toolu_log")).await?;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let tail: HarnessToolOutput = serde_json::from_slice(&body)?;
    assert!(tail.truncated);
    assert!(!tail.is_error);
    assert!(tail.text.len() <= 64 * 1024);
    assert!(log.ends_with(&tail.text));
    assert!(tail.text.ends_with("build finished\n"));
    tokio::time::timeout(Duration::from_secs(10), composition.close()).await??;
    Ok(())
}
