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

mod attach;
mod reaper;
mod registry;
mod spawn;

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
    settings: TerminalSettings,
    plan: MuxPlan,
    state: Mutex<BTreeMap<TerminalId, Runtime>>,
    by_session: Mutex<BTreeMap<SessionKey, TerminalId>>,
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
                settings,
                plan,
                state: Mutex::new(BTreeMap::new()),
                by_session: Mutex::new(BTreeMap::new()),
                detach_tx,
                detach_rx: Mutex::new(Some(detach_rx)),
            }),
        }
    }

    #[must_use]
    pub fn plan(&self) -> &MuxPlan {
        &self.inner.plan
    }

    #[must_use]
    pub fn settings(&self) -> &TerminalSettings {
        &self.inner.settings
    }

    pub async fn start(&self, spec: SpawnSpec) -> Result<TerminalId, Error> {
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
            MuxPlan::Direct => self.start_pty(&spec),
            MuxPlan::Herdr(setup) => self.start_herdr(setup, spec).await,
            MuxPlan::Unavailable(_) => Err(Error::MuxUnavailable(
                "terminal mux is unavailable".to_owned(),
            )),
        }
    }
}

#[async_trait::async_trait]
impl kernel::terminal::TerminalMux for TerminalManager {
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
        self.start(spec)
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
        tokio::spawn(forward_port_attach(stream.replay, stream.live, tx));
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
        Self::kill(self, id)
            .await
            .map_err(kernel::terminal::TerminalMuxError::from)
    }

    async fn list(
        &self,
    ) -> Result<Vec<kernel::terminal::PtyInfo>, kernel::terminal::TerminalMuxError> {
        Ok(self
            .list()
            .into_iter()
            .filter_map(|record| {
                let session = match record.session {
                    Some(session) => {
                        match wire::session_id::SessionId::from_str(session.as_str()) {
                            Ok(session) => session,
                            Err(_) => return None,
                        }
                    }
                    None => return None,
                };
                Some(kernel::terminal::PtyInfo {
                    id: record.id,
                    session,
                    running: record.state == TerminalState::Running,
                })
            })
            .collect())
    }
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
    let program = argv
        .first()
        .ok_or_else(|| Error::Spawn("argv is empty".to_owned()))?;
    let name = Path::new(program)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(program);
    RosterKey::new(name.to_owned())
}
