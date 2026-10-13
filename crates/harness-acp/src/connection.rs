use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use agent_client_protocol as acp;
use agent_client_protocol::Agent as _;
use kernel::harness::SessionEvent;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::compat::TokioAsyncReadCompatExt as _;
use tokio_util::compat::TokioAsyncWriteCompatExt as _;
use tokio_util::sync::CancellationToken;
use tokio_util::sync::DropGuard;

use crate::client::ClientBridge;
use crate::error::AcpError;
use crate::launch::AcpCommand;
use crate::permissions::PermissionRegistry;
use crate::process;
use crate::process::ChildPipes;

const EVENT_CAPACITY: usize = 128;
const COMMAND_CAPACITY: usize = 32;

#[derive(Debug, Default)]
pub(crate) struct StreamBarrier {
    inflight: Mutex<usize>,
    drained: tokio::sync::Notify,
}

impl StreamBarrier {
    pub(crate) fn enter(&self) {
        *self.inflight.lock() += 1;
    }

    pub(crate) fn leave(&self) {
        let mut waiting = self.inflight.lock();
        *waiting -= 1;
        if *waiting == 0 {
            self.drained.notify_waiters();
        }
    }

    pub(crate) async fn wait(&self) {
        let notified = self.drained.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if *self.inflight.lock() == 0 {
            return;
        }
        notified.await;
    }
}

#[derive(Debug)]
pub(crate) enum SessionCommand {
    NewSession {
        cwd: PathBuf,
        reply: oneshot::Sender<Result<acp::SessionId, acp::Error>>,
    },
    LoadSession {
        native: acp::SessionId,
        cwd: PathBuf,
        reply: oneshot::Sender<Result<(), acp::Error>>,
    },
    Prompt {
        acp_session: acp::SessionId,
        text: String,
        reply: oneshot::Sender<Result<acp::PromptResponse, acp::Error>>,
    },
    Cancel {
        acp_session: acp::SessionId,
    },
    SetMode {
        acp_session: acp::SessionId,
        mode: String,
        reply: oneshot::Sender<Result<(), acp::Error>>,
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
    pub(crate) commands: mpsc::Sender<SessionCommand>,
    pub(crate) acp_session: acp::SessionId,
    pub(crate) permissions: Arc<PermissionRegistry>,
    pub(crate) barrier: Arc<StreamBarrier>,
    pub(crate) turn_end: mpsc::UnboundedSender<()>,
    pub(crate) events_rx: mpsc::Receiver<SessionEvent>,
    pub(crate) exited: Arc<AtomicBool>,
    pub(crate) shutdown: CancellationToken,
}

struct Connecting {
    commands: mpsc::Sender<SessionCommand>,
    permissions: Arc<PermissionRegistry>,
    barrier: Arc<StreamBarrier>,
    turn_end: mpsc::UnboundedSender<()>,
    events_rx: mpsc::Receiver<SessionEvent>,
    exited: Arc<AtomicBool>,
    shutdown: CancellationToken,
    guard: DropGuard,
}

impl Connecting {
    fn finish(self, acp_session: acp::SessionId) -> AgentConnection {
        let Self {
            commands,
            permissions,
            barrier,
            turn_end,
            events_rx,
            exited,
            shutdown,
            guard,
        } = self;
        drop(guard.disarm());
        AgentConnection {
            commands,
            acp_session,
            permissions,
            barrier,
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
    let acp_session = request(&connected.commands, |reply| SessionCommand::NewSession {
        cwd: working_dir.to_path_buf(),
        reply,
    })
    .await
    .map_err(|failure| port_failure(failure, "session/new"))?;
    Ok(connected.finish(acp_session))
}

pub(crate) async fn start_loaded(
    command: &AcpCommand,
    working_dir: &Path,
    native: &acp::SessionId,
) -> Result<AgentConnection, AcpError> {
    let connected = connect(command, working_dir).await?;
    request(&connected.commands, |reply| SessionCommand::LoadSession {
        native: native.clone(),
        cwd: working_dir.to_path_buf(),
        reply,
    })
    .await
    .map_err(|failure| port_failure(failure, "session/load"))?;
    Ok(connected.finish(native.clone()))
}

async fn connect(command: &AcpCommand, working_dir: &Path) -> Result<Connecting, AcpError> {
    let pipes = process::spawn_child(command, working_dir)?;
    let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
    let (turn_end, turn_end_rx) = mpsc::unbounded_channel();
    let (commands, commands_rx) = mpsc::channel(COMMAND_CAPACITY);
    let permissions = Arc::new(PermissionRegistry::default());
    let barrier = Arc::new(StreamBarrier::default());
    let exited = Arc::new(AtomicBool::new(false));
    let shutdown = CancellationToken::new();
    let guard = shutdown.clone().drop_guard();

    let ChildPipes {
        child,
        stdin,
        stdout,
        stderr,
    } = pipes;
    tokio::spawn(process::pump_stderr(stderr));
    process::watch_child(child, exited.clone(), events_tx.clone(), shutdown.clone());

    let bridge = ClientBridge {
        events: events_tx.clone(),
        permissions: permissions.clone(),
        barrier: barrier.clone(),
    };
    let (ready_tx, ready_rx) = oneshot::channel();
    let wiring = ConnectionWiring {
        stdin,
        stdout,
        bridge,
        events_tx,
        turn_end_rx,
        commands_rx,
        ready_tx,
    };
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let local = tokio::task::LocalSet::new();
        runtime.block_on(local.run_until(serve_connection(wiring)));
    });
    let initialized = ready_rx.await.map_err(|_| AcpError::Closed {
        method: "initialize",
    })?;
    initialized.map_err(|source| AcpError::Protocol {
        method: "initialize",
        source,
    })?;
    Ok(Connecting {
        commands,
        permissions,
        barrier,
        turn_end,
        events_rx,
        exited,
        shutdown,
        guard,
    })
}

struct ConnectionWiring {
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    bridge: ClientBridge,
    events_tx: mpsc::Sender<SessionEvent>,
    turn_end_rx: mpsc::UnboundedReceiver<()>,
    commands_rx: mpsc::Receiver<SessionCommand>,
    ready_tx: oneshot::Sender<Result<(), acp::Error>>,
}

async fn serve_connection(wiring: ConnectionWiring) {
    let ConnectionWiring {
        stdin,
        stdout,
        bridge,
        events_tx,
        turn_end_rx,
        commands_rx,
        ready_tx,
    } = wiring;
    let (conn, io) =
        acp::ClientSideConnection::new(bridge, stdin.compat_write(), stdout.compat(), |future| {
            tokio::task::spawn_local(future);
        });
    let conn = Arc::new(conn);
    let mut io = tokio::task::spawn_local(io);
    tokio::task::spawn_local(forward_turn_ends(events_tx, turn_end_rx));
    let initialized = conn
        .initialize(acp::InitializeRequest {
            protocol_version: acp::V1,
            client_capabilities: acp::ClientCapabilities::default(),
            meta: None,
        })
        .await
        .map(|response| response.agent_capabilities.load_session);
    match initialized {
        Ok(load_session) => {
            tracing::debug!(load_session, "agent initialized");
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
        }
        Err(error) => {
            if ready_tx.send(Err(error)).is_err() {
                tracing::debug!("the agent failed initialize after its requester left");
            }
            return;
        }
    }
    tokio::select! {
        result = &mut io => match result {
            Ok(Ok(())) => tracing::debug!("agent closed the protocol stream"),
            Ok(Err(error)) => tracing::debug!(%error, "agent protocol stream failed"),
            Err(error) => tracing::debug!(%error, "agent protocol task failed"),
        },
        () = actor_loop(conn, commands_rx) => {
            tracing::debug!("the session was retired by its owner");
        }
    }
}

async fn actor_loop(
    conn: Arc<acp::ClientSideConnection>,
    mut commands: mpsc::Receiver<SessionCommand>,
) {
    while let Some(command) = commands.recv().await {
        match command {
            SessionCommand::Cancel { acp_session } => {
                let sent = conn
                    .cancel(acp::CancelNotification {
                        session_id: acp_session,
                        meta: None,
                    })
                    .await;
                if let Err(error) = sent {
                    tracing::debug!(%error, "cancel notification did not reach the agent");
                }
            }
            SessionCommand::NewSession { cwd, reply } => {
                tokio::task::spawn_local(new_session_task(conn.clone(), cwd, reply));
            }
            SessionCommand::LoadSession { native, cwd, reply } => {
                tokio::task::spawn_local(load_session_task(conn.clone(), native, cwd, reply));
            }
            SessionCommand::Prompt {
                acp_session,
                text,
                reply,
            } => {
                tokio::task::spawn_local(prompt_task(conn.clone(), acp_session, text, reply));
            }
            SessionCommand::SetMode {
                acp_session,
                mode,
                reply,
            } => {
                tokio::task::spawn_local(set_mode_task(conn.clone(), acp_session, mode, reply));
            }
        }
    }
}

async fn new_session_task(
    conn: Arc<acp::ClientSideConnection>,
    cwd: PathBuf,
    reply: oneshot::Sender<Result<acp::SessionId, acp::Error>>,
) {
    let response = conn
        .new_session(acp::NewSessionRequest {
            cwd,
            mcp_servers: Vec::new(),
            meta: None,
        })
        .await
        .map(|response| response.session_id);
    if reply.send(response).is_err() {
        tracing::debug!("the session command outlived its requester");
    }
}

async fn load_session_task(
    conn: Arc<acp::ClientSideConnection>,
    native: acp::SessionId,
    cwd: PathBuf,
    reply: oneshot::Sender<Result<(), acp::Error>>,
) {
    let response = conn
        .load_session(acp::LoadSessionRequest {
            cwd,
            mcp_servers: Vec::new(),
            session_id: native,
            meta: None,
        })
        .await
        .map(|_| ());
    if reply.send(response).is_err() {
        tracing::debug!("the session command outlived its requester");
    }
}

async fn prompt_task(
    conn: Arc<acp::ClientSideConnection>,
    acp_session: acp::SessionId,
    text: String,
    reply: oneshot::Sender<Result<acp::PromptResponse, acp::Error>>,
) {
    let response = conn
        .prompt(acp::PromptRequest {
            session_id: acp_session,
            prompt: vec![acp::ContentBlock::from(text)],
            meta: None,
        })
        .await;
    if reply.send(response).is_err() {
        tracing::debug!("the session command outlived its requester");
    }
}

async fn set_mode_task(
    conn: Arc<acp::ClientSideConnection>,
    acp_session: acp::SessionId,
    mode: String,
    reply: oneshot::Sender<Result<(), acp::Error>>,
) {
    let response = conn
        .set_session_mode(acp::SetSessionModeRequest {
            session_id: acp_session,
            mode_id: acp::SessionModeId(mode.into()),
            meta: None,
        })
        .await
        .map(|_| ());
    if reply.send(response).is_err() {
        tracing::debug!("the session command outlived its requester");
    }
}

async fn forward_turn_ends(
    events: mpsc::Sender<SessionEvent>,
    mut turn_ends: mpsc::UnboundedReceiver<()>,
) {
    while turn_ends.recv().await.is_some() {
        if events.send(SessionEvent::TurnEnded).await.is_err() {
            tracing::debug!("turn end had no listener");
        }
    }
}
