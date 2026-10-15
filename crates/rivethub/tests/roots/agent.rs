use std::process::Stdio;
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::{TestResult, provider_config, stderr, terminate, write_state};

#[sqlx::test(migrations = false)]
async fn agent_runs_against_postgres_and_stops_on_sigterm(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    write_state(home.path(), &provider_config(home.path(), &url, ""))?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rh"))
        .arg("agent")
        .env_clear()
        .env("HOME", home.path())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let logs = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut text = String::new();
        let mut ready = Some(ready_tx);
        while let Some(line) = lines.next_line().await? {
            if line.contains("agent running")
                && let Some(sender) = ready.take()
                && sender.send(()).is_err()
            {
                return Err(std::io::Error::other("agent readiness listener dropped"));
            }
            text.push_str(&line);
            text.push('\n');
        }
        Ok(text)
    });
    if tokio::time::timeout(Duration::from_secs(30), ready_rx).await != Ok(Ok(())) {
        if child.try_wait()?.is_none()
            && let Some(pid) = child.id()
        {
            terminate(pid)?;
        }
        let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
        return Err(format!("agent did not run: {text}").into());
    }
    let pid = child.id().ok_or("agent already stopped")?;
    terminate(pid)?;
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
    let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
    assert!(status.success(), "agent exit: {status}\n{text}");
    assert!(text.contains("SIGTERM received — shutting down"), "{text}");
    assert!(text.contains("agent stopped"), "{text}");
    Ok(())
}

#[tokio::test]
async fn agent_rejects_invalid_preset_json() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(home.path(), super::DEAD_POSTGRES, ""),
    )?;
    std::fs::write(home.path().join(".rivethub").join("agents.json"), "{")?;
    let output = super::run_rh(home.path(), &["agent"]).await?;
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("task configuration JSON is invalid"),
        "{text}"
    );
    Ok(())
}
