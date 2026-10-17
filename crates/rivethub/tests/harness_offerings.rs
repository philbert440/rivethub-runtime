use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, header};
use kernel::memory::MemoryStore;
use kernel::shutdown::Activities;
use rivethub::composition::Composition;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn executable(dir: &Path, name: &str, body: &str) -> TestResult {
    let binary = dir.join(name);
    std::fs::write(&binary, body)?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gateway_finds_a_harness_adapter_on_its_path() -> TestResult {
    let home = tempfile::tempdir()?;
    let work = home.path().join("work");
    let bin = home.path().join("bin");
    let state = home.path().join(".rivethub");
    for dir in [&work, &bin, &state] {
        std::fs::create_dir_all(dir)?;
    }
    executable(&bin, "herdr", "#!/bin/sh\necho 0.8.2\n")?;
    let launched = home.path().join("launched");
    executable(
        &bin,
        "claude-agent-acp",
        &format!("#!/bin/sh\ntouch '{}'\n", launched.display()),
    )?;
    let commands = home.path().join("commands.json");
    std::fs::write(
        &commands,
        format!(
            r#"{{"default":"shell","cwd":"{}","commands":{{"shell":{{"label":"Shell","cmd":["/bin/true"],"room":false}}}}}}"#,
            work.display()
        ),
    )?;
    std::fs::write(
        state.join("config.yaml"),
        format!(
            "runtime:\n  workspace: {work}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:9/v1\n    model: m\n    probe_models: false\nden:\n  host: 127.0.0.1\n  port: 5976\n  terminal:\n    enabled: true\n  commands_file: {commands}\n  files_root: {work}\n",
            work = work.display(),
            commands = commands.display(),
        ),
    )?;
    let env = config::Environment::from_vars([
        ("HOME", home.path().to_str().ok_or("home")?),
        ("PATH", bin.to_str().ok_or("path")?),
    ]);
    let loaded = config::Config::load(
        &env,
        &config::ConfigLocation::Current(state.join("config.yaml")),
        config::Profile::Node,
    )?;
    let shutdown = CancellationToken::new();
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let composition = tokio::time::timeout(
        Duration::from_secs(30),
        Composition::build(&loaded.config, &env, store, &shutdown, &Activities::new()),
    )
    .await??;
    let router = composition.gateway().router(false);
    let mut request = Request::builder()
        .uri("/api/harnesses")
        .header(header::HOST, "127.0.0.1:5976")
        .body(Body::empty())?;
    request
        .extensions_mut()
        .insert(ConnectInfo("127.0.0.1:40000".parse::<SocketAddr>()?));
    router.oneshot(request).await?;
    assert!(launched.is_file(), "claude-agent-acp was never launched");
    tokio::time::timeout(Duration::from_secs(10), composition.close()).await??;
    Ok(())
}
