use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const CONFIGURED: &str = "rr-configured-node";
const FROM_ENV: &str = "from-env";
const FROM_HOSTNAME_ENV: &str = "from-hostname-env";
const DEAD_POSTGRES: &str = "postgres://127.0.0.1:1/none";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Started {
    child: tokio::process::Child,
    logs: tokio::task::JoinHandle<std::io::Result<String>>,
}

fn provider_config(home: &Path, postgres: impl std::fmt::Display, extra: &str) -> String {
    format!(
        "runtime:\n  workspace: {}\n  default_agent: main\nagents:\n  main:\n    provider: p\n    model: m\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:1\n    model: m\nmemory:\n  postgres:\n    connection_string: {postgres}\n{extra}",
        home.display()
    )
}

fn write_state(home: &Path, body: &str) -> TestResult {
    let state = home.join(".rivethub");
    std::fs::create_dir_all(&state)?;
    std::fs::write(state.join("config.yaml"), body)?;
    Ok(())
}

fn node_line(root: &str) -> String {
    format!("service node {root} {CONFIGURED} hostname ")
}

fn logged_host(text: &str, root: &str) -> Result<String, String> {
    let marker = node_line(root);
    let Some(rest) = text.split(&marker).nth(1) else {
        return Err(format!("missing node line: {text}"));
    };
    let host = rest.lines().next().unwrap_or(rest).trim();
    if host.is_empty() || host == CONFIGURED || host == FROM_ENV || host == FROM_HOSTNAME_ENV {
        return Err(format!("unexpected hostname {host:?} in {text}"));
    }
    Ok(host.to_owned())
}

fn command(home: &Path, args: &[&str]) -> Command {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rh"));
    child
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("RIVETHUB_NODE_NAME", FROM_ENV)
        .env("RIVETHUB_NODE_ID", "from-id")
        .env("HOSTNAME", FROM_HOSTNAME_ENV);
    child
}

async fn run_checked(home: &Path, args: &[&str]) -> TestResult<std::process::Output> {
    Ok(tokio::time::timeout(Duration::from_secs(30), command(home, args).output()).await??)
}

fn assert_checked(output: &std::process::Output, root: &str, ready: &str) -> TestResult {
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}");
    assert!(text.contains(ready), "{text}");
    logged_host(&text, root)?;
    Ok(())
}

async fn start_logged(home: &Path, args: &[&str], ready: &str) -> TestResult<Started> {
    let mut child = command(home, args);
    child.stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = child.spawn()?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let needle = ready.to_owned();
    let logs = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut text = String::new();
        let mut sender = Some(ready_tx);
        while let Some(line) = lines.next_line().await? {
            if line.contains(&needle)
                && let Some(sender) = sender.take()
                && sender.send(()).is_err()
            {
                return Err(std::io::Error::other("readiness listener dropped"));
            }
            text.push_str(&line);
            text.push('\n');
        }
        Ok(text)
    });
    if tokio::time::timeout(Duration::from_secs(30), ready_rx)
        .await
        .is_ok_and(|result| result.is_ok())
    {
        return Ok(Started { child, logs });
    }
    stop_early(&mut child)?;
    let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
    Err(format!("{ready} did not appear: {text}").into())
}

fn stop_early(child: &mut tokio::process::Child) -> TestResult {
    if child.try_wait()?.is_none()
        && let Some(pid) = child.id()
    {
        terminate(pid)?;
    }
    Ok(())
}

fn terminate(pid: u32) -> TestResult {
    let pid = i32::try_from(pid)?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    )?;
    Ok(())
}

async fn finish(started: Started) -> TestResult<String> {
    let Started { mut child, logs } = started;
    let pid = child.id().ok_or("process already stopped")?;
    terminate(pid)?;
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
    let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
    assert!(status.success(), "exit: {status}\n{text}");
    Ok(text)
}

fn free_port() -> TestResult<u16> {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = probe.local_addr()?.port();
    drop(probe);
    Ok(port)
}

#[tokio::test]
async fn worker_check_uses_the_configured_node_name() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            DEAD_POSTGRES,
            "mesh:\n  node_name: rr-configured-node\n",
        ),
    )?;
    let output = run_checked(home.path(), &["worker", "--role", "summarizer", "--check"]).await?;
    assert_checked(&output, "worker", "worker configuration valid")
}

#[tokio::test]
async fn mcp_check_uses_the_configured_node_name() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            DEAD_POSTGRES,
            "mesh:\n  node_name: rr-configured-node\n",
        ),
    )?;
    let output = run_checked(home.path(), &["mcp", "--check"]).await?;
    assert_checked(&output, "mcp", "mcp configuration valid")
}

#[sqlx::test(migrations = false)]
async fn gateway_healthz_reports_the_configured_node(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    let port = free_port()?;
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            &url,
            &format!(
                "den:\n  host: 127.0.0.1\n  port: {port}\nmesh:\n  node_name: \" {CONFIGURED} \"\n"
            ),
        ),
    )?;
    let started = start_logged(home.path(), &["gateway"], "gateway listening on ").await?;
    let health: wire::gateway::HealthzResponse = reqwest::Client::builder()
        .no_proxy()
        .build()?
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(health.ok);
    assert_eq!(health.node, CONFIGURED);
    assert_ne!(health.name, health.node);
    assert_ne!(health.name, FROM_ENV);
    assert_ne!(health.name, FROM_HOSTNAME_ENV);
    let text = finish(started).await?;
    assert!(
        text.contains(&format!("{}{}", node_line("gateway"), health.name)),
        "{text}"
    );
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn agent_claims_the_configured_queue_and_not_the_hostname(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    install_queue(&pool).await?;
    install_attempt_notify(&pool).await?;
    let mut attempts_heard = sqlx::postgres::PgListener::connect_with(&pool).await?;
    attempts_heard.listen("rh_node_name_attempts").await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            &url,
            "mesh:\n  node_name: rr-configured-node\n",
        ),
    )?;
    let started = start_logged(home.path(), &["agent"], "agent running").await?;
    let host = os_hostname()?;
    enqueue(&pool, &format!("run-task:{CONFIGURED}")).await?;
    enqueue(&pool, &format!("run-task:{host}")).await?;
    let claimed = wait_for_claim(&pool, &host, &mut attempts_heard, &started.child).await;
    let (success, text) = stop_and_logs(started).await?;
    claimed.map_err(|error| format!("{error}\n{text}"))?;
    assert!(success, "{text}");
    assert!(
        text.contains(&format!("{}{host}", node_line("agent"))),
        "{text}"
    );
    Ok(())
}

fn os_hostname() -> TestResult<String> {
    let text = nix::unistd::gethostname()?
        .into_string()
        .map_err(|_| "hostname is not unicode")?;
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed == CONFIGURED
        || trimmed == FROM_ENV
        || trimmed == FROM_HOSTNAME_ENV
    {
        return Err(format!("hostname {trimmed:?} cannot prove the configured name").into());
    }
    Ok(trimmed.to_owned())
}

async fn stop_and_logs(started: Started) -> TestResult<(bool, String)> {
    let Started { mut child, logs } = started;
    if child.try_wait()?.is_none()
        && let Some(pid) = child.id()
    {
        terminate(pid)?;
    }
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
    let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
    Ok((status.success(), text))
}

async fn install_queue(pool: &PgPool) -> Result<(), sqlx::Error> {
    let present: bool =
        sqlx::query_scalar("SELECT to_regclass('graphile_worker._private_jobs') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if present {
        return Ok(());
    }
    sqlx::raw_sql(include_str!("fixtures/graphile_worker.txt"))
        .execute(pool)
        .await?;
    Ok(())
}

async fn enqueue(pool: &PgPool, identifier: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT (graphile_worker.add_job($1, '{}'::json, null, null, 1, null, 0, null::text[], 'replace')).id",
    )
    .bind(identifier)
    .execute(pool)
    .await?;
    Ok(())
}

async fn attempts(pool: &PgPool, identifier: &str) -> Result<i32, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT j.attempts::int \
           FROM graphile_worker._private_jobs j \
           JOIN graphile_worker._private_tasks t ON t.id = j.task_id \
          WHERE t.identifier = $1",
    )
    .bind(identifier)
    .fetch_optional(pool)
    .await?;
    Ok(row.map_or(0, |row| row.0))
}

async fn install_attempt_notify(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(ATTEMPT_NOTIFY).execute(pool).await?;
    Ok(())
}

async fn wait_for_claim(
    pool: &PgPool,
    host: &str,
    heard: &mut sqlx::postgres::PgListener,
    child: &tokio::process::Child,
) -> Result<(), String> {
    let configured = format!("run-task:{CONFIGURED}");
    let hostname = format!("run-task:{host}");
    let child_id = child.id();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            heard.recv().await.map_err(|error| error.to_string())?;
            if let Some(pid) = child_id {
                let running = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(
                        i32::try_from(pid).map_err(|error| error.to_string())?,
                    ),
                    None,
                );
                if running.is_err() {
                    return Err("agent exited before the configured job was claimed".to_owned());
                }
            }
            let configured_attempts = attempts(pool, &configured)
                .await
                .map_err(|error| error.to_string())?;
            let hostname_attempts = attempts(pool, &hostname)
                .await
                .map_err(|error| error.to_string())?;
            if hostname_attempts != 0 {
                return Err(format!(
                    "hostname queue {hostname} was claimed ({hostname_attempts} attempts)"
                ));
            }
            if configured_attempts == 1 {
                return Ok(());
            }
        }
    })
    .await
    .map_err(|_| "configured queue was not claimed".to_owned())?
}

const ATTEMPT_NOTIFY: &str = "\
CREATE OR REPLACE FUNCTION rh_node_name_attempt_notify() RETURNS trigger AS $fn$
BEGIN
  PERFORM pg_notify('rh_node_name_attempts', NEW.attempts::text);
  RETURN NEW;
END;
$fn$ LANGUAGE plpgsql;
DROP TRIGGER IF EXISTS rh_node_name_attempts ON graphile_worker._private_jobs;
CREATE TRIGGER rh_node_name_attempts
AFTER UPDATE OF attempts ON graphile_worker._private_jobs
FOR EACH ROW EXECUTE FUNCTION rh_node_name_attempt_notify();
";

fn write_certs(directory: &Path) -> TestResult {
    let root_key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(vec!["root.example.test".into()])?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let root = params.self_signed(&root_key)?;
    let issuer = rcgen::Issuer::from_params(&params, &root_key);
    let node_key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])?;
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let node = params.signed_by(&node_key, &issuer)?;
    std::fs::write(directory.join("node.pem"), node.pem())?;
    std::fs::write(directory.join("node.key"), node_key.serialize_pem())?;
    std::fs::write(directory.join("ca.pem"), root.pem())?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn mesh_registry_records_the_configured_node(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    let state = home.path().join(".rivethub");
    std::fs::create_dir_all(&state)?;
    write_certs(&state)?;
    let shared = home.path().join("shared");
    std::fs::create_dir_all(&shared)?;
    let port = free_port()?;
    let channel = free_port()?;
    let body = provider_config(
        home.path(),
        &url,
        &format!(
            "den:\n  host: 127.0.0.1\n  port: {port}\n  tls_cert: {}\n  tls_key: {}\n  tls_ca: {}\nmesh:\n  enabled: true\n  node_name: \" {CONFIGURED} \"\n  tls: true\n  advertise_host: 127.0.0.1\n  agent_channel_port: {channel}\n",
            state.join("node.pem").display(),
            state.join("node.key").display(),
            state.join("ca.pem").display(),
        ),
    );
    std::fs::write(state.join("config.yaml"), body)?;
    let mut child = command(home.path(), &["gateway"]);
    child
        .env("RIVETHUB_SHARED_DIR", &shared)
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = child.spawn()?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let logs = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut text = String::new();
        while let Some(line) = lines.next_line().await? {
            text.push_str(&line);
            text.push('\n');
        }
        Ok::<String, std::io::Error>(text)
    });
    let registry = shared.join("mesh.json");
    let recorded = wait_for_registry(&registry, &mut child).await;
    let text = match recorded {
        Ok(file) => {
            let started = Started { child, logs };
            let text = finish(started).await?;
            assert!(file.nodes.contains_key(CONFIGURED), "{file:?}");
            assert_eq!(
                file.nodes.get(CONFIGURED).map(|node| node.name.as_str()),
                Some(CONFIGURED)
            );
            text
        }
        Err(error) => {
            stop_early(&mut child)?;
            let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
            return Err(format!("{error}\n{text}").into());
        }
    };
    logged_host(&text, "gateway")?;
    Ok(())
}

async fn wait_for_registry(
    path: &Path,
    child: &mut tokio::process::Child,
) -> Result<wire::mesh::MeshFile, String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("mesh registry {} has no directory", path.display()))?;
    let watched = mesh_watch(directory)?;
    if let Some(file) = read_registry(path).await? {
        return Ok(file);
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            mesh_event(&watched).await?;
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!("gateway exited before mesh.json: {status}"));
                }
                Ok(None) => {}
                Err(error) => return Err(error.to_string()),
            }
            if let Some(file) = read_registry(path).await? {
                return Ok(file);
            }
        }
    })
    .await
    .map_err(|_| "mesh registry did not record the configured node".to_owned())?
}

#[derive(Debug)]
struct MeshWatch {
    inotify: nix::sys::inotify::Inotify,
}

impl std::os::fd::AsRawFd for MeshWatch {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        std::os::fd::AsRawFd::as_raw_fd(&std::os::fd::AsFd::as_fd(&self.inotify))
    }
}

fn mesh_watch(directory: &Path) -> Result<tokio::io::unix::AsyncFd<MeshWatch>, String> {
    let inotify = nix::sys::inotify::Inotify::init(
        nix::sys::inotify::InitFlags::IN_CLOEXEC | nix::sys::inotify::InitFlags::IN_NONBLOCK,
    )
    .map_err(|error| error.to_string())?;
    inotify
        .add_watch(
            directory,
            nix::sys::inotify::AddWatchFlags::IN_CLOSE_WRITE
                | nix::sys::inotify::AddWatchFlags::IN_CREATE
                | nix::sys::inotify::AddWatchFlags::IN_MOVED_TO,
        )
        .map_err(|error| error.to_string())?;
    tokio::io::unix::AsyncFd::new(MeshWatch { inotify }).map_err(|error| error.to_string())
}

async fn mesh_event(watched: &tokio::io::unix::AsyncFd<MeshWatch>) -> Result<(), String> {
    loop {
        let mut ready = watched
            .readable()
            .await
            .map_err(|error| error.to_string())?;
        match ready.try_io(|inner| read_inotify(&inner.get_ref().inotify)) {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => return Err(error.to_string()),
            Err(_blocked) => {}
        }
    }
}

fn read_inotify(instance: &nix::sys::inotify::Inotify) -> std::io::Result<()> {
    match instance.read_events() {
        Ok(_) => Ok(()),
        Err(error)
            if error == nix::errno::Errno::EAGAIN || error == nix::errno::Errno::EWOULDBLOCK =>
        {
            Err(std::io::Error::from(std::io::ErrorKind::WouldBlock))
        }
        Err(error) => Err(std::io::Error::other(error)),
    }
}

async fn read_registry(path: &Path) -> Result<Option<wire::mesh::MeshFile>, String> {
    match tokio::fs::read(path).await {
        Ok(bytes) => match serde_json::from_slice::<wire::mesh::MeshFile>(&bytes) {
            Ok(file) if file.nodes.contains_key(CONFIGURED) => Ok(Some(file)),
            Ok(_) | Err(_) => Ok(None),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
