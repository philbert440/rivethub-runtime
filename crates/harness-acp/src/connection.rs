use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use agent_client_protocol::Agent;
use agent_client_protocol::ByteStreams;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1 as acp;
use kernel::harness::SessionEvent;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::compat::TokioAsyncReadCompatExt as _;
use tokio_util::compat::TokioAsyncWriteCompatExt as _;
use tokio_util::sync::CancellationToken;
use tokio_util::sync::DropGuard;
use tokio_util::task::TaskTracker;

use crate::error::AcpError;
use crate::launch::AcpCommand;
use crate::options::SessionOptions;
use crate::options::stored_options;
use crate::permissions::PermissionRegistry;
use crate::process;
use crate::process::ChildPipes;

const EVENT_CAPACITY: usize = 128;
const COMMAND_CAPACITY: usize = 32;

#[derive(Debug)]
pub(crate) enum SessionCommand {
    NewSession {
        cwd: PathBuf,
        reply: oneshot::Sender<Result<acp::NewSessionResponse, acp::Error>>,
    },
    LoadSession {
        native: acp::SessionId,
        cwd: PathBuf,
        reply: oneshot::Sender<Result<acp::LoadSessionResponse, acp::Error>>,
    },
    Prompt {
        acp_session: acp::SessionId,
        text: String,
        reply: oneshot::Sender<Result<acp::PromptResponse, acp::Error>>,
    },
    Cancel {
        acp_session: acp::SessionId,
    },
    SetConfig {
        acp_session: acp::SessionId,
        config_id: acp::SessionConfigId,
        value: acp::SessionConfigOptionValue,
        reply: oneshot::Sender<Result<acp::SetSessionConfigOptionResponse, acp::Error>>,
    },
}

#[derive(Debug)]
pub(crate) enum RequestFailure {
    Closed,
    Protocol(acp::Error),
}

pub(crate) fn port_failure(failure: RequestFailure, method: &'static str) -> AcpError {
    match failure {
        RequestFailure::Closed => AcpError::Closed { method },
        RequestFailure::Protocol(source) => AcpError::Protocol { method, source },
    }
}

pub(crate) struct AgentConnection {
    pub(crate) capabilities: acp::AgentCapabilities,
    pub(crate) tasks: TaskTracker,
    pub(crate) reaped: tokio::sync::oneshot::Receiver<Result<(), std::io::Error>>,
    pub(crate) commands: mpsc::Sender<SessionCommand>,
    pub(crate) acp_session: acp::SessionId,
    pub(crate) permissions: Arc<PermissionRegistry>,
    pub(crate) options: Arc<SessionOptions>,
    pub(crate) turn_end: mpsc::UnboundedSender<()>,
    pub(crate) events_rx: mpsc::Receiver<SessionEvent>,
    pub(crate) exited: Arc<AtomicBool>,
    pub(crate) shutdown: CancellationToken,
}

struct Connecting {
    capabilities: acp::AgentCapabilities,
    tasks: TaskTracker,
    reaped: tokio::sync::oneshot::Receiver<Result<(), std::io::Error>>,
    commands: mpsc::Sender<SessionCommand>,
    permissions: Arc<PermissionRegistry>,
    options: Arc<SessionOptions>,
    turn_end: mpsc::UnboundedSender<()>,
    events_rx: mpsc::Receiver<SessionEvent>,
    exited: Arc<AtomicBool>,
    shutdown: CancellationToken,
    guard: DropGuard,
}

impl Connecting {
    fn finish(
        self,
        acp_session: acp::SessionId,
        config_options: Vec<acp::SessionConfigOption>,
    ) -> AgentConnection {
        self.options.replace(config_options);
        let Self {
            capabilities,
            tasks,
            reaped,
            commands,
            permissions,
            options,
            turn_end,
            events_rx,
            exited,
            shutdown,
            guard,
        } = self;
        drop(guard.disarm());
        AgentConnection {
            capabilities,
            tasks,
            reaped,
            commands,
            acp_session,
            permissions,
            options,
            turn_end,
            events_rx,
            exited,
            shutdown,
        }
    }
}

pub(crate) async fn request<R>(
    commands: &mpsc::Sender<SessionCommand>,
    build: impl FnOnce(oneshot::Sender<Result<R, acp::Error>>) -> SessionCommand,
) -> Result<R, RequestFailure> {
    let (reply, answer) = oneshot::channel();
    commands
        .send(build(reply))
        .await
        .map_err(|_| RequestFailure::Closed)?;
    let responded = answer.await.map_err(|_| RequestFailure::Closed)?;
    responded.map_err(RequestFailure::Protocol)
}

pub(crate) async fn start_new(
    command: &AcpCommand,
    working_dir: &Path,
) -> Result<AgentConnection, AcpError> {
    let connected = connect(command, working_dir).await?;
    let response = request(&connected.commands, |reply| SessionCommand::NewSession {
        cwd: working_dir.to_path_buf(),
        reply,
    })
    .await
    .map_err(|failure| port_failure(failure, "session/new"))?;
    let session_id = response.session_id.clone();
    let options = stored_options(response.config_options);
    Ok(connected.finish(session_id, options))
}

pub(crate) async fn start_loaded(
    command: &AcpCommand,
    working_dir: &Path,
    native: &acp::SessionId,
) -> Result<AgentConnection, AcpError> {
    let connected = connect(command, working_dir).await?;
    let response = request(&connected.commands, |reply| SessionCommand::LoadSession {
        native: native.clone(),
        cwd: working_dir.to_path_buf(),
        reply,
    })
    .await
    .map_err(|failure| port_failure(failure, "session/load"))?;
    Ok(connected.finish(native.clone(), stored_options(response.config_options)))
}

pub async fn initialize_handshake(
    command: &AcpCommand,
    working_dir: &Path,
) -> Result<(), AcpError> {
    let connected = connect(command, working_dir).await?;
    let Connecting {
        reaped,
        tasks,
        shutdown,
        guard,
        ..
    } = connected;
    shutdown.cancel();
    drop(guard.disarm());
    let outcome = reaped.await;
    tasks.wait().await;
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(source)) => Err(AcpError::Launch {
            program: command.program.display().to_string(),
            source,
        }),
        Err(_) => Err(AcpError::Closed {
            method: "initialize",
        }),
    }
}

async fn connect(command: &AcpCommand, working_dir: &Path) -> Result<Connecting, AcpError> {
    let pipes = process::spawn_child(command, working_dir)?;
    let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
    let (turn_end, turn_end_rx) = mpsc::unbounded_channel();
    let (commands, commands_rx) = mpsc::channel(COMMAND_CAPACITY);
    let permissions = Arc::new(PermissionRegistry::default());
    let options = Arc::new(SessionOptions::default());
    let exited = Arc::new(AtomicBool::new(false));
    let shutdown = CancellationToken::new();
    let guard = shutdown.clone().drop_guard();
    let ChildPipes {
        child,
        stdin,
        stdout,
        stderr,
    } = pipes;
    let tasks = TaskTracker::new();
    process::watch_stderr(stderr, shutdown.clone(), &tasks);
    let reaped = process::watch_child(
        child,
        Arc::clone(&exited),
        events_tx.clone(),
        shutdown.clone(),
        &tasks,
    );
    let (connection_ended_tx, connection_ended_rx) = oneshot::channel();
    tasks.spawn(forward_turn_ends(
        events_tx.clone(),
        turn_end_rx,
        connection_ended_rx,
        shutdown.clone(),
    ));
    let (ready_tx, ready_rx) = oneshot::channel();
    let client = crate::client::SessionClient {
        events: events_tx,
        permissions: Arc::clone(&permissions),
        options: Arc::clone(&options),
    };
    let protocol_shutdown = shutdown.clone();
    tasks.spawn(async move {
        let protocol = serve(stdin, stdout, client, commands_rx, ready_tx);
        let outcome = tokio::select! {
            () = protocol_shutdown.cancelled() => Ok(()),
            outcome = protocol => outcome,
        };
        drop(connection_ended_tx);
        if let Err(error) = outcome {
            tracing::debug!(%error, "agent protocol stream failed");
        }
    });
    let initialized = ready_rx.await.map_err(|_| AcpError::Closed {
        method: "initialize",
    })?;
    let capabilities = initialized.map_err(|source| AcpError::Protocol {
        method: "initialize",
        source,
    })?;
    tasks.close();
    Ok(Connecting {
        capabilities,
        tasks,
        reaped,
        commands,
        permissions,
        options,
        turn_end,
        events_rx,
        exited,
        shutdown,
        guard,
    })
}

async fn serve(
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    client: crate::client::SessionClient,
    commands_rx: mpsc::Receiver<SessionCommand>,
    ready_tx: oneshot::Sender<Result<acp::AgentCapabilities, acp::Error>>,
) -> Result<(), acp::Error> {
    let session_client = client.clone();
    Client
        .builder()
        .name("rivethub")
        .on_receive_notification(
            async move |notification: acp::SessionNotification, _cx| {
                crate::client::on_session_update(
                    notification,
                    &session_client.events,
                    &session_client.options,
                )
                .await
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: acp::RequestPermissionRequest, responder, _cx| {
                crate::client::on_permission(
                    request,
                    responder,
                    &client.permissions,
                    &client.events,
                )
                .await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            ByteStreams::new(stdin.compat_write(), stdout.compat()),
            async move |connection| drive(connection, commands_rx, ready_tx).await,
        )
        .await
}

fn initialize_request() -> acp::InitializeRequest {
    acp::InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            acp::ClientCapabilities::new()
                .session(Some(acp::ClientSessionCapabilities::new().config_options(
                    Some(acp::SessionConfigOptionsCapabilities::new()),
                ))),
        )
        .client_info(acp::Implementation::new(
            "rivethub",
            env!("CARGO_PKG_VERSION"),
        ))
}

async fn drive(
    connection: ConnectionTo<Agent>,
    commands_rx: mpsc::Receiver<SessionCommand>,
    ready_tx: oneshot::Sender<Result<acp::AgentCapabilities, acp::Error>>,
) -> Result<(), acp::Error> {
    let initialized = connection
        .send_request(initialize_request())
        .block_task()
        .await;
    match initialized {
        Ok(response) => {
            tracing::debug!(
                load_session = response.agent_capabilities.load_session,
                "agent initialized"
            );
            if ready_tx.send(Ok(response.agent_capabilities)).is_err() {
                return Ok(());
            }
        }
        Err(error) => {
            if ready_tx.send(Err(error)).is_err() {
                tracing::debug!("the agent failed initialize after its requester left");
            }
            return Ok(());
        }
    }
    command_loop(connection, commands_rx).await;
    Ok(())
}

async fn command_loop(
    connection: ConnectionTo<Agent>,
    mut commands: mpsc::Receiver<SessionCommand>,
) {
    loop {
        tokio::select! {
            biased;
            () = connection.incoming_closed() => break,
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    SessionCommand::Cancel { acp_session } => send_cancel(&connection, acp_session),
                    SessionCommand::NewSession { cwd, reply } => {
                        deliver(reply, new_session(&connection, cwd).await);
                    }
                    SessionCommand::LoadSession { native, cwd, reply } => {
                        deliver(reply, load_session(&connection, native, cwd).await);
                    }
                    SessionCommand::Prompt {
                        acp_session,
                        text,
                        reply,
                    } => spawn_prompt(&connection, acp_session, text, reply),
                    SessionCommand::SetConfig {
                        acp_session,
                        config_id,
                        value,
                        reply,
                    } => deliver(
                        reply,
                        set_config(&connection, acp_session, config_id, value).await,
                    ),
                }
            }
        }
    }
}

fn deliver<T>(reply: oneshot::Sender<T>, value: T) {
    if reply.send(value).is_err() {
        tracing::debug!("the session command outlived its requester");
    }
}

fn send_cancel(connection: &ConnectionTo<Agent>, acp_session: acp::SessionId) {
    if let Err(error) = connection.send_notification(acp::CancelNotification::new(acp_session)) {
        tracing::debug!(%error, "cancel notification did not reach the agent");
    }
}

async fn new_session(
    connection: &ConnectionTo<Agent>,
    cwd: PathBuf,
) -> Result<acp::NewSessionResponse, acp::Error> {
    connection
        .send_request(acp::NewSessionRequest::new(cwd))
        .block_task()
        .await
}

async fn load_session(
    connection: &ConnectionTo<Agent>,
    native: acp::SessionId,
    cwd: PathBuf,
) -> Result<acp::LoadSessionResponse, acp::Error> {
    connection
        .send_request(acp::LoadSessionRequest::new(native, cwd))
        .block_task()
        .await
}

async fn set_config(
    connection: &ConnectionTo<Agent>,
    acp_session: acp::SessionId,
    config_id: acp::SessionConfigId,
    value: acp::SessionConfigOptionValue,
) -> Result<acp::SetSessionConfigOptionResponse, acp::Error> {
    connection
        .send_request(acp::SetSessionConfigOptionRequest::new(
            acp_session,
            config_id,
            value,
        ))
        .block_task()
        .await
}

fn spawn_prompt(
    connection: &ConnectionTo<Agent>,
    acp_session: acp::SessionId,
    text: String,
    reply: oneshot::Sender<Result<acp::PromptResponse, acp::Error>>,
) {
    let prompt_connection = connection.clone();
    if let Err(error) = connection.spawn(async move {
        let response = prompt_connection
            .send_request(acp::PromptRequest::new(
                acp_session,
                vec![acp::ContentBlock::from(text)],
            ))
            .block_task()
            .await;
        deliver(reply, response);
        Ok(())
    }) {
        tracing::warn!(%error, "prompt task failed to start");
    }
}

async fn forward_turn_ends(
    events: mpsc::Sender<SessionEvent>,
    mut turn_ends: mpsc::UnboundedReceiver<()>,
    mut connection_ended: oneshot::Receiver<()>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            turn = turn_ends.recv() => {
                let Some(()) = turn else {
                    break;
                };
                let sent = tokio::select! {
                    () = shutdown.cancelled() => break,
                    sent = events.send(SessionEvent::TurnEnded) => sent,
                };
                if sent.is_err() {
                    tracing::debug!("turn end had no listener");
                    break;
                }
            }
            result = &mut connection_ended => {
                drop(result);
                break;
            }
        }
    }
}
