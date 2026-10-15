use std::process::Stdio;
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use wire::gateway::agents::AgentListResponse;
use wire::gateway::agents::AgentStoreBackend;
use wire::gateway::devices::PhonePairingAvailability;

use super::{TestResult, init_authority, provider_config, run_rh, stderr, terminate, write_state};

const GATEWAY_URL: &str = "http://127.0.0.1:9";

#[sqlx::test(migrations = false)]
async fn gateway_with_an_unusable_authority_directory_refuses_to_start(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = probe.local_addr()?.port();
    drop(probe);
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            &url,
            &format!("den:\n  host: 127.0.0.1\n  port: {port}\n"),
        ),
    )?;
    std::fs::create_dir_all(home.path().join(".rivethub").join("ca"))?;
    let output = run_rh(home.path(), &["gateway"]).await?;
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("CA state directory"), "{text}");
    Ok(())
}

struct Started {
    child: tokio::process::Child,
    logs: tokio::task::JoinHandle<std::io::Result<String>>,
}

async fn start_gateway(home: &std::path::Path) -> TestResult<Started> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rh"))
        .arg("gateway")
        .env_clear()
        .env("HOME", home)
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
            if line.contains("gateway listening on ")
                && let Some(sender) = ready.take()
                && sender.send(()).is_err()
            {
                return Err(std::io::Error::other("gateway readiness listener dropped"));
            }
            text.push_str(&line);
            text.push('\n');
        }
        Ok(text)
    });
    if tokio::time::timeout(Duration::from_secs(30), ready_rx).await == Ok(Ok(())) {
        Ok(Started { child, logs })
    } else {
        if child.try_wait()?.is_none()
            && let Some(pid) = child.id()
        {
            terminate(pid)?;
        }
        let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
        Err(format!("gateway did not listen: {text}").into())
    }
}

#[sqlx::test(migrations = false)]
async fn phone_pairing_uses_the_authority_and_agents_stay_on_the_file_store(
    pool: PgPool,
) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = probe.local_addr()?.port();
    drop(probe);
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            &url,
            &format!(
                "den:\n  host: 127.0.0.1\n  port: {port}\n  devices:\n    gateway_url: {GATEWAY_URL}\n"
            ),
        ),
    )?;
    init_authority(home.path()).await?;
    let Started { mut child, logs } = start_gateway(home.path()).await?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let base = format!("http://127.0.0.1:{port}");
    let pairing = client
        .get(format!("{base}/api/phone-pairing"))
        .send()
        .await?
        .error_for_status()?
        .json::<PhonePairingAvailability>()
        .await?;
    assert!(pairing.available, "{pairing:?}");
    assert_eq!(pairing.gateway.as_deref(), Some(GATEWAY_URL));
    let agents = client
        .get(format!("{base}/api/agents"))
        .send()
        .await?
        .error_for_status()?
        .json::<AgentListResponse>()
        .await?;
    assert_eq!(agents.backend, Some(AgentStoreBackend::File));
    let pid = child.id().ok_or("gateway already stopped")?;
    terminate(pid)?;
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
    let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
    assert!(status.success(), "gateway exit: {status}\n{text}");
    assert!(text.contains("SIGTERM received — shutting down"), "{text}");
    assert!(text.contains("gateway stopped"), "{text}");
    Ok(())
}
