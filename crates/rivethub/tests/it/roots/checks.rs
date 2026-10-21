use std::net::TcpListener;

use super::{DEAD_POSTGRES, TestResult, provider_config, run_rh, stderr, write_state};

#[tokio::test]
async fn worker_check_accepts_an_unreachable_postgres_url() -> TestResult {
    let home = tempfile::tempdir()?;
    let legacy = home.path().join(".rivetos");
    std::fs::create_dir_all(&legacy)?;
    std::fs::write(legacy.join("marker"), "kept")?;
    write_state(
        home.path(),
        &provider_config(home.path(), DEAD_POSTGRES, ""),
    )?;
    let output = run_rh(home.path(), &["worker", "--role", "summarizer", "--check"]).await?;
    let text = stderr(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("worker configuration valid"), "{text}");
    assert_eq!(std::fs::read_to_string(legacy.join("marker"))?, "kept");
    Ok(())
}

#[tokio::test]
async fn the_wiki_builder_role_has_a_handler() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(home.path(), DEAD_POSTGRES, ""),
    )?;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_rh"))
            .args(["worker", "--role", "wiki-builder", "--check"])
            .env_clear()
            .env("HOME", home.path())
            .env("RIVETHUB_WIKI_EXTRACTION", "1")
            .output(),
    )
    .await??;
    let text = stderr(&output);
    assert!(output.status.success(), "wiki-builder: {text}");
    Ok(())
}

#[tokio::test]
async fn mcp_check_does_not_bind_or_connect() -> TestResult {
    let home = tempfile::tempdir()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let extra = format!(
        "transports:\n  mcp:\n    host: 127.0.0.1\n    port: {port}\n    auth_token: token\n"
    );
    write_state(
        home.path(),
        &provider_config(home.path(), DEAD_POSTGRES, &extra),
    )?;
    let output = run_rh(home.path(), &["mcp", "--check"]).await?;
    let text = stderr(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("mcp configuration valid"), "{text}");
    assert_eq!(listener.local_addr()?.port(), port);
    Ok(())
}

#[tokio::test]
async fn mcp_check_rejects_an_empty_host() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            DEAD_POSTGRES,
            "transports:\n  mcp:\n    host: \"\"\n    auth_token: token\n",
        ),
    )?;
    let output = run_rh(home.path(), &["mcp", "--check"]).await?;
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("mcp host is empty"), "{text}");
    Ok(())
}

#[tokio::test]
async fn mcp_check_rejects_a_relative_socket_and_a_missing_bearer() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            DEAD_POSTGRES,
            "transports:\n  mcp:\n    socket: relative.sock\n",
        ),
    )?;
    let relative = run_rh(home.path(), &["mcp", "--check"]).await?;
    let text = stderr(&relative);
    assert!(!relative.status.success(), "{text}");
    assert!(text.contains("mcp socket path must be absolute"), "{text}");

    write_state(
        home.path(),
        &provider_config(
            home.path(),
            DEAD_POSTGRES,
            "transports:\n  mcp:\n    require_bearer_on_socket: true\n",
        ),
    )?;
    let bearer = run_rh(home.path(), &["mcp", "--check"]).await?;
    let text = stderr(&bearer);
    assert_eq!(bearer.status.code(), Some(1), "{text}");
    assert!(text.contains("mcp socket bearer is required"), "{text}");
    Ok(())
}

#[tokio::test]
async fn agent_check_accepts_an_unreachable_postgres_url() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(home.path(), DEAD_POSTGRES, ""),
    )?;
    let output = run_rh(home.path(), &["agent", "--check"]).await?;
    let text = stderr(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("agent configuration valid"), "{text}");
    Ok(())
}

#[tokio::test]
async fn agent_check_rejects_disabled_tasks_and_partial_pricing() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(home.path(), DEAD_POSTGRES, "tasks:\n  enabled: false\n"),
    )?;
    let disabled = run_rh(home.path(), &["agent", "--check"]).await?;
    let text = stderr(&disabled);
    assert_eq!(disabled.status.code(), Some(1), "{text}");
    assert!(text.contains("tasks are disabled"), "{text}");

    write_state(
        home.path(),
        &provider_config(
            home.path(),
            DEAD_POSTGRES,
            "tasks:\n  pricing:\n    m:\n      input_per_mtok: 1.5\n",
        ),
    )?;
    let pricing = run_rh(home.path(), &["agent", "--check"]).await?;
    let text = stderr(&pricing);
    assert_eq!(pricing.status.code(), Some(1), "{text}");
    assert!(
        text.contains("pricing for m needs input_per_mtok and output_per_mtok"),
        "{text}"
    );
    Ok(())
}

#[tokio::test]
async fn agent_refuses_a_symlinked_preset_file_without_connecting() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(home.path(), DEAD_POSTGRES, ""),
    )?;
    let state = home.path().join(".rivethub");
    std::os::unix::fs::symlink(state.join("config.yaml"), state.join("agents.json"))?;
    let output = run_rh(home.path(), &["agent"]).await?;
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("agents.json escapes the state directory"),
        "{text}"
    );
    Ok(())
}

#[tokio::test]
async fn gateway_check_succeeds_without_a_certificate_authority() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            DEAD_POSTGRES,
            "gateway:\n  host: 127.0.0.1\n  port: 9\n",
        ),
    )?;
    let output = run_rh(home.path(), &["gateway", "--check"]).await?;
    let text = stderr(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("gateway configuration valid"), "{text}");
    assert!(!home.path().join(".rivethub").join("ca").exists());
    Ok(())
}
