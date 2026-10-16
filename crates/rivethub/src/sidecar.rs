use std::os::unix::fs::FileTypeExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use config::model::TransportSection;
use config::{Config, Environment};
use kernel::memory::MemoryStore;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::Exit;
use crate::service::{StartupError, load, postgres, report};

#[derive(Debug, Error)]
pub(crate) enum SidecarError {
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Mcp(#[from] mcp::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Agent(#[from] crate::agent::AgentError),
}

impl SidecarError {
    pub(crate) fn exit(&self) -> Exit {
        match self {
            Self::Startup(error) => error.exit(),
            Self::Mcp(error) => mcp_exit(error),
            Self::Join(_) | Self::Invalid(_) => Exit::Failure,
            Self::Agent(error) => error.exit(),
        }
    }
}

fn mcp_exit(error: &mcp::Error) -> Exit {
    match error {
        mcp::Error::Io(error) => io_exit(error),
        mcp::Error::SocketIo { source, .. } => io_exit(source),
        mcp::Error::SocketListening { .. }
        | mcp::Error::SocketNotSocket { .. }
        | mcp::Error::SocketChanged { .. }
        | mcp::Error::SocketProbeTimeout { .. } => Exit::Serve,
        mcp::Error::Arguments(_)
        | mcp::Error::Invalid(_)
        | mcp::Error::Refused(_)
        | mcp::Error::Cancelled
        | mcp::Error::Memory(_)
        | mcp::Error::WikiSlug(_)
        | mcp::Error::WikiMissing(_)
        | mcp::Error::WikiRedirect
        | mcp::Error::Task(_)
        | mcp::Error::Initialize(_)
        | mcp::Error::Service(_)
        | mcp::Error::Format(_) => Exit::Failure,
    }
}

fn io_exit(error: &std::io::Error) -> Exit {
    let kind = error.kind();
    if kind == std::io::ErrorKind::AddrInUse || kind == std::io::ErrorKind::AddrNotAvailable {
        Exit::Bind
    } else {
        Exit::Serve
    }
}

struct TcpEndpoint {
    host: String,
    port: u16,
    bearer: mcp::transport::Bearer,
}

struct UnixEndpoint {
    path: PathBuf,
    bearer: Option<mcp::transport::Bearer>,
}

enum Selected {
    Stdio,
    Network {
        tcp: Option<TcpEndpoint>,
        unix: Option<UnixEndpoint>,
    },
}

pub async fn start(env: &Environment, check: bool) -> Exit {
    if check {
        return finish(validate(env).await, "mcp configuration valid");
    }
    let signals = match crate::signals::install() {
        Ok(signals) => signals,
        Err(exit) => return exit,
    };
    let result = run(env, signals.token()).await;
    if let Err(exit) = signals.finish().await {
        return exit;
    }
    finish(result, "")
}

fn finish(result: Result<(), SidecarError>, ready: &str) -> Exit {
    match result {
        Ok(()) => {
            if !ready.is_empty() {
                tracing::info!("{ready}");
            }
            Exit::Ok
        }
        Err(error) => {
            report_error(&error);
            error.exit()
        }
    }
}

pub(crate) fn report_error(error: &SidecarError) {
    match error {
        SidecarError::Startup(error) => report(error),
        SidecarError::Mcp(_)
        | SidecarError::Join(_)
        | SidecarError::Invalid(_)
        | SidecarError::Agent(_) => {
            tracing::error!("{error}");
        }
    }
}

async fn validate(env: &Environment) -> Result<(), SidecarError> {
    let config = read_config(env).await?;
    let _selected = select(&config)?;
    crate::identity::announce_resolved("mcp", &config, env).map_err(StartupError::from)?;
    Ok(())
}

async fn run(env: &Environment, shutdown: CancellationToken) -> Result<(), SidecarError> {
    let _guard = shutdown.clone().drop_guard();
    let config = read_config(env).await?;
    let selected = select(&config)?;
    crate::identity::announce_resolved("mcp", &config, env).map_err(StartupError::from)?;
    let memory = Arc::new(postgres(&config, env).await?) as Arc<dyn MemoryStore>;
    let result = run_server(&config, env, Arc::clone(&memory), selected, &shutdown).await;
    shutdown.cancel();
    let closed = memory.close().await.map_err(StartupError::from);
    closed?;
    result?;
    tracing::info!("mcp stopped");
    Ok(())
}

async fn run_server(
    config: &Config,
    env: &Environment,
    memory: Arc<dyn MemoryStore>,
    selected: Selected,
    shutdown: &CancellationToken,
) -> Result<(), SidecarError> {
    let delegation = crate::agent::open_delegation(config, env, shutdown).await?;
    let ran = async {
        let server = mcp::Server::builder(memory, false, shutdown.child_token())
            .delegation(Arc::clone(&delegation.port), surface(&selected))
            .build()?;
        serve(server, selected, shutdown.clone()).await
    }
    .await;
    let stopped = delegation.shutdown().await;
    stopped?;
    ran
}

pub(crate) fn memory_future(
    config: &Config,
    memory: Arc<dyn MemoryStore>,
    delegation: Option<Arc<dyn kernel::tasks::Delegation>>,
    shutdown: CancellationToken,
) -> Result<impl std::future::Future<Output = Result<(), SidecarError>> + Send + use<>, SidecarError>
{
    let selected = select(config)?;
    if matches!(selected, Selected::Stdio) {
        return Err(SidecarError::Invalid(
            "rh local serves MCP only when a host or socket is configured".into(),
        ));
    }
    let mut builder = mcp::Server::builder(memory, false, shutdown.child_token());
    if let Some(port) = delegation {
        builder = builder.delegation(port, mcp::Surface::Stdio);
    }
    let server = builder.build()?;
    Ok(async move {
        serve(server, selected, shutdown).await?;
        tracing::info!("mcp stopped");
        Ok(())
    })
}

fn surface(selected: &Selected) -> mcp::Surface {
    match selected {
        Selected::Stdio => mcp::Surface::Stdio,
        Selected::Network { .. } => mcp::Surface::Remote,
    }
}

async fn read_config(env: &Environment) -> Result<Config, SidecarError> {
    let owned = env.clone();
    Ok(tokio::task::spawn_blocking(move || load(&owned)).await??)
}

fn select(config: &Config) -> Result<Selected, SidecarError> {
    let Some(section) = mcp_section(config) else {
        return Ok(Selected::Stdio);
    };
    if section.require_bearer_on_socket == Some(true) && section.auth_token.is_none() {
        return Err(SidecarError::Invalid(
            "mcp socket bearer is required".into(),
        ));
    }
    let tcp = tcp_endpoint(section)?;
    let unix = unix_endpoint(section)?;
    if tcp.is_none() && unix.is_none() {
        Ok(Selected::Stdio)
    } else {
        Ok(Selected::Network { tcp, unix })
    }
}

fn mcp_section(config: &Config) -> Option<&TransportSection> {
    config.transports.as_ref().and_then(|map| map.get("mcp"))
}

fn tcp_endpoint(section: &TransportSection) -> Result<Option<TcpEndpoint>, SidecarError> {
    let Some(host) = section.host.as_deref() else {
        return Ok(None);
    };
    if host.is_empty() {
        return Err(SidecarError::Invalid("mcp host is empty".into()));
    }
    if section.port == 0 {
        return Err(SidecarError::Invalid("mcp port must not be 0".into()));
    }
    let Some(token) = section.auth_token.as_deref() else {
        return Err(SidecarError::Invalid("mcp auth token is required".into()));
    };
    Ok(Some(TcpEndpoint {
        host: host.to_owned(),
        port: section.port,
        bearer: mcp::transport::Bearer::new(token)?,
    }))
}

fn unix_endpoint(section: &TransportSection) -> Result<Option<UnixEndpoint>, SidecarError> {
    let Some(path) = section.socket.as_deref() else {
        return Ok(None);
    };
    absolute_socket(path)?;
    let bearer = match section.auth_token.as_deref() {
        Some(token) => Some(mcp::transport::Bearer::new(token)?),
        None => None,
    };
    Ok(Some(UnixEndpoint {
        path: path.to_path_buf(),
        bearer,
    }))
}

fn absolute_socket(path: &Path) -> Result<(), SidecarError> {
    let traversable = path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir));
    if path.is_absolute() && !traversable {
        Ok(())
    } else {
        Err(SidecarError::Invalid(
            "mcp socket path must be absolute".into(),
        ))
    }
}

async fn serve(
    server: mcp::Server,
    selected: Selected,
    shutdown: CancellationToken,
) -> Result<(), SidecarError> {
    match selected {
        Selected::Stdio => serve_stdio(server, shutdown).await,
        Selected::Network { tcp, unix } => serve_network(server, tcp, unix, shutdown).await,
    }
}

async fn serve_stdio(server: mcp::Server, shutdown: CancellationToken) -> Result<(), SidecarError> {
    tracing::info!("mcp serving stdio");
    let tracker = TaskTracker::new();
    let token = shutdown.child_token();
    let task = tracker.spawn(async move { mcp::transport::serve_stdio(server, token).await });
    let joined = join_task(&shutdown, task).await;
    tracker.close();
    tracker.wait().await;
    joined??;
    Ok(())
}

async fn serve_network(
    server: mcp::Server,
    tcp: Option<TcpEndpoint>,
    unix: Option<UnixEndpoint>,
    shutdown: CancellationToken,
) -> Result<(), SidecarError> {
    let tracker = TaskTracker::new();
    let token = shutdown.clone();
    let driver = tracker.spawn(async move { drive_network(server, tcp, unix, token).await });
    let joined = join_task(&shutdown, driver).await;
    tracker.close();
    tracker.wait().await;
    joined??;
    Ok(())
}

async fn drive_network(
    server: mcp::Server,
    tcp: Option<TcpEndpoint>,
    unix: Option<UnixEndpoint>,
    shutdown: CancellationToken,
) -> Result<(), mcp::Error> {
    let mut tasks = tokio::task::JoinSet::new();
    if let Some(endpoint) = tcp {
        let server = server.clone();
        let token = shutdown.child_token();
        tasks.spawn(async move { listen_tcp(server, endpoint, token).await });
    }
    if let Some(endpoint) = unix {
        let token = shutdown.child_token();
        tasks.spawn(async move { listen_unix(server, endpoint, token).await });
    }
    let mut failure = None;
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            joined = tasks.join_next() => {
                match joined {
                    None => break,
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(error))) => {
                        failure = Some(error);
                        break;
                    }
                    Some(Err(error)) => {
                        failure = Some(mcp::Error::Task(error));
                        break;
                    }
                }
            }
        }
    }
    shutdown.cancel();
    while let Some(joined) = tasks.join_next().await {
        if failure.is_none() {
            failure = match joined {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(error) => Some(mcp::Error::Task(error)),
            };
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn listen_tcp(
    server: mcp::Server,
    endpoint: TcpEndpoint,
    shutdown: CancellationToken,
) -> Result<(), mcp::Error> {
    let listener = tokio::net::TcpListener::bind((endpoint.host.as_str(), endpoint.port)).await?;
    let address = listener.local_addr()?;
    tracing::info!("mcp listening on {address}");
    mcp::transport::serve_tcp(listener, server, endpoint.bearer, shutdown).await
}

async fn listen_unix(
    server: mcp::Server,
    endpoint: UnixEndpoint,
    shutdown: CancellationToken,
) -> Result<(), mcp::Error> {
    let bound = mcp::transport::bind_unix(&endpoint.path).await?;
    tracing::info!("mcp listening on {}", endpoint.path.display());
    let transport = mcp::transport::serve_unix(bound, server, endpoint.bearer, shutdown).await;
    let removed = remove_socket(&endpoint.path).await;
    combine(transport, removed)
}

fn combine(
    served: Result<(), mcp::Error>,
    removed: Result<(), mcp::Error>,
) -> Result<(), mcp::Error> {
    match (served, removed) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(served), Err(removed)) => Err(mcp::Error::Invalid(format!("{served}; {removed}"))),
    }
}

async fn remove_socket(path: &Path) -> Result<(), mcp::Error> {
    match tokio::fs::symlink_metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(mcp::Error::Io(error)),
        Ok(metadata) if metadata.file_type().is_socket() => {
            tokio::fs::remove_file(path).await.map_err(mcp::Error::Io)
        }
        Ok(_) => Err(mcp::Error::SocketNotSocket {
            path: path.to_path_buf(),
        }),
    }
}

async fn join_task<T: Send>(
    shutdown: &CancellationToken,
    task: tokio::task::JoinHandle<T>,
) -> Result<T, tokio::task::JoinError> {
    tokio::pin!(task);
    tokio::select! {
        biased;
        () = shutdown.cancelled() => (&mut task).await,
        result = &mut task => result,
    }
}
