use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use kernel::terminal::TermSize;
use kernel::terminal::TerminalId;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::error::Error;
use crate::manager::registry::Runtime;
use crate::mux::MuxPlan;
use crate::session::RosterKey;
use crate::session::SessionKey;
use crate::settings::TerminalSettings;

mod adopt;
mod attach;
mod port;
mod reaper;
mod registry;
mod spawn;

pub use adopt::AdoptedSession;
pub use adopt::Adoption;

const CHUNK_QUEUE: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub session: Option<SessionKey>,
    pub roster_key: RosterKey,
    pub argv: Vec<String>,
    pub cwd: std::path::PathBuf,
    pub size: TermSize,
    pub entry_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalState {
    Running,
    Failed,
    Exited { code: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backing {
    Pty,
    Herdr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRecord {
    pub id: TerminalId,
    pub session: Option<SessionKey>,
    pub roster_key: RosterKey,
    pub argv: Vec<String>,
    pub pid: Option<u32>,
    pub size: TermSize,
    pub state: TerminalState,
    pub attached: u32,
    pub backing: Backing,
}

#[derive(Debug)]
pub struct AttachStream {
    pub record: TerminalRecord,
    pub replay: Vec<u8>,
    pub live: Option<mpsc::Receiver<Vec<u8>>>,
}

struct Inner {
    tasks: tokio_util::task::TaskTracker,
    shutdown: tokio_util::sync::CancellationToken,
    settings: TerminalSettings,
    plan: MuxPlan,
    state: Mutex<BTreeMap<TerminalId, Runtime>>,
    by_session: Mutex<BTreeMap<SessionKey, TerminalId>>,
    port_backends: Mutex<BTreeMap<TerminalId, TerminalId>>,
    owner: Mutex<Option<String>>,
    detach_tx: mpsc::UnboundedSender<TerminalId>,
    detach_rx: Mutex<Option<mpsc::UnboundedReceiver<TerminalId>>>,
}

#[derive(Clone)]
pub struct TerminalManager {
    inner: Arc<Inner>,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "Inner holds pty handles and locks and is not Debug, so the derive does not compile; the impl prints only the plan"
    )
)]
impl std::fmt::Debug for TerminalManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalManager")
            .field("plan", &self.inner.plan)
            .finish_non_exhaustive()
    }
}

impl TerminalManager {
    pub fn new(settings: TerminalSettings, plan: MuxPlan) -> Self {
        let (detach_tx, detach_rx) = mpsc::unbounded_channel();
        Self {
            inner: Arc::new(Inner {
                tasks: tokio_util::task::TaskTracker::new(),
                shutdown: tokio_util::sync::CancellationToken::new(),
                settings,
                plan,
                state: Mutex::new(BTreeMap::new()),
                by_session: Mutex::new(BTreeMap::new()),
                port_backends: Mutex::new(BTreeMap::new()),
                owner: Mutex::new(None),
                detach_tx,
                detach_rx: Mutex::new(Some(detach_rx)),
            }),
        }
    }

    pub async fn start(&self, spec: SpawnSpec) -> Result<TerminalId, Error> {
        if self.inner.shutdown.is_cancelled() {
            return Err(Error::ShuttingDown);
        }
        if let MuxPlan::Unavailable(unavailable) = &self.inner.plan {
            return Err(unavailable.reason.to_error());
        }
        if let Some(session) = &spec.session
            && let Some(id) = self.bound_terminal(session)
            && self.is_running(&id)
        {
            return Ok(id);
        }
        if let Some(victim) = self.enforce_cap(self.count_running())? {
            self.evict(&victim);
        }
        match &self.inner.plan {
            MuxPlan::Direct => {
                let manager = self.clone();
                self.inner
                    .tasks
                    .spawn_blocking(move || manager.start_pty(&spec))
                    .await
                    .map_err(Error::Worker)?
            }
            MuxPlan::Herdr(setup) => self.start_herdr(setup, spec).await,
            MuxPlan::Unavailable(unavailable) => Err(unavailable.reason.to_error()),
        }
    }
}

#[async_trait::async_trait]
impl kernel::terminal::TerminalMux for TerminalManager {
    async fn inspect(
        &self,
        id: &TerminalId,
    ) -> Result<kernel::terminal::TerminalDetails, kernel::terminal::TerminalMuxError> {
        let record = self
            .list()
            .into_iter()
            .find(|record| record.id == *id)
            .ok_or_else(|| kernel::terminal::TerminalMuxError::UnknownTerminal(id.clone()))?;
        port_details(&record)
    }

    async fn open(
        &self,
        id: &TerminalId,
    ) -> Result<kernel::terminal::TerminalAttachment, kernel::terminal::TerminalMuxError> {
        let stream = self.attach_stream(id)?;
        let output = match stream.live {
            Some(output) => output,
            None => mpsc::channel(1).1,
        };
        Ok(kernel::terminal::TerminalAttachment {
            replay: stream.replay,
            output,
        })
    }

    async fn wait(&self, id: &TerminalId) -> Result<i32, kernel::terminal::TerminalMuxError> {
        let record = self.wait_exited(id).await?;
        port_details(&record)?.exit_code.ok_or_else(|| {
            Error::Failed {
                terminal: id.clone(),
            }
            .into()
        })
    }
    async fn spawn(
        &self,
        session: &wire::session_id::SessionId,
        argv: Vec<String>,
        cwd: &Path,
        size: TermSize,
    ) -> Result<TerminalId, kernel::terminal::TerminalMuxError> {
        let session =
            SessionKey::new(session.as_str()).map_err(kernel::terminal::TerminalMuxError::from)?;
        let roster_key =
            roster_key_from_argv(&argv).map_err(kernel::terminal::TerminalMuxError::from)?;
        let spec = SpawnSpec {
            session: Some(session),
            roster_key,
            argv,
            cwd: cwd.to_path_buf(),
            size,
            entry_env: BTreeMap::new(),
        };
        self.port_spawn(spec)
            .await
            .map_err(kernel::terminal::TerminalMuxError::from)
    }

    async fn attach(
        &self,
        id: &TerminalId,
    ) -> Result<tokio::sync::mpsc::Receiver<Vec<u8>>, kernel::terminal::TerminalMuxError> {
        let stream = self
            .attach_stream(id)
            .map_err(kernel::terminal::TerminalMuxError::from)?;
        let (tx, rx) = mpsc::channel(CHUNK_QUEUE);
        let shutdown = self.inner.shutdown.clone();
        self.inner.tasks.spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => {},
                () = forward_port_attach(stream.replay, stream.live, tx) => {},
            }
        });
        Ok(rx)
    }

    async fn input(
        &self,
        id: &TerminalId,
        bytes: Vec<u8>,
    ) -> Result<(), kernel::terminal::TerminalMuxError> {
        Self::input(self, id, bytes)
            .await
            .map_err(kernel::terminal::TerminalMuxError::from)
    }

    async fn resize(
        &self,
        id: &TerminalId,
        size: TermSize,
    ) -> Result<(), kernel::terminal::TerminalMuxError> {
        Self::resize(self, id, size).map_err(kernel::terminal::TerminalMuxError::from)
    }

    async fn kill(&self, id: &TerminalId) -> Result<(), kernel::terminal::TerminalMuxError> {
        let backend = self.inner.port_backends.lock().get(id).cloned();
        if let Some(backend) = backend {
            Self::kill(self, &backend).await?;
        }
        Self::kill(self, id)
            .await
            .map_err(kernel::terminal::TerminalMuxError::from)
    }

    async fn list(
        &self,
    ) -> Result<Vec<kernel::terminal::PtyInfo>, kernel::terminal::TerminalMuxError> {
        self.list()
            .into_iter()
            .filter_map(|record| {
                let session = record.session?;
                Some(
                    wire::session_id::SessionId::from_str(session.as_str())
                        .map(|session| kernel::terminal::PtyInfo {
                            id: record.id,
                            session,
                            running: record.state == TerminalState::Running,
                        })
                        .map_err(|source| {
                            kernel::terminal::TerminalMuxError::Unavailable(Box::new(source))
                        }),
                )
            })
            .collect()
    }
}

fn port_details(
    record: &TerminalRecord,
) -> Result<kernel::terminal::TerminalDetails, kernel::terminal::TerminalMuxError> {
    let exit_code =
        match record.state {
            TerminalState::Running => None,
            TerminalState::Failed => {
                return Err(Error::Failed {
                    terminal: record.id.clone(),
                }
                .into());
            }
            TerminalState::Exited { code } => Some(i32::try_from(code).map_err(|source| {
                kernel::terminal::TerminalMuxError::Unavailable(Box::new(source))
            })?),
        };
    Ok(kernel::terminal::TerminalDetails {
        pid: record.pid,
        size: record.size,
        exit_code,
    })
}

async fn forward_port_attach(
    replay: Vec<u8>,
    live: Option<mpsc::Receiver<Vec<u8>>>,
    out: mpsc::Sender<Vec<u8>>,
) {
    if out.send(replay).await.is_err() {
        return;
    }
    if let Some(mut live) = live {
        while let Some(chunk) = live.recv().await {
            if out.send(chunk).await.is_err() {
                return;
            }
        }
    }
}

fn roster_key_from_argv(argv: &[String]) -> Result<RosterKey, Error> {
    let program = argv.first().ok_or(Error::EmptyArgv)?;
    let name = Path::new(program)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| Error::InvalidRosterKey(program.clone()))?;
    RosterKey::new(name.to_owned())
}

#[cfg(test)]
#[path = "../../tests/manager_ttl/mod.rs"]
mod ttl_tests;
