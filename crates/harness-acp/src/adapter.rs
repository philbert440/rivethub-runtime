use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use agent_client_protocol as acp;
use async_trait::async_trait;
use config::Config;
use kernel::harness::HarnessAdapter;
use kernel::harness::HarnessAdapterError;
use kernel::harness::SessionEvent;
use kernel::harness::SessionOption;
use kernel::harness::SessionSpec;
use kernel::harness::TurnInput;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use wire::harness::ApprovalDecision;
use wire::harness::HarnessId;
use wire::session_id::SessionId;

use crate::connection::AgentConnection;
use crate::connection::SessionCommand;
use crate::connection::StreamBarrier;
use crate::connection::port_failure;
use crate::connection::request;
use crate::error::AcpError;
use crate::error::port;
use crate::identity;
use crate::launch::AcpCommand;
use crate::launch::LaunchTable;
use crate::permissions::PermissionRegistry;

#[derive(Debug)]
struct SessionEntry {
    reaped: Option<tokio::sync::oneshot::Receiver<Result<(), std::io::Error>>>,
    commands: mpsc::Sender<SessionCommand>,
    acp_session: acp::SessionId,
    permissions: Arc<PermissionRegistry>,
    barrier: Arc<StreamBarrier>,
    turn_end: mpsc::UnboundedSender<()>,
    events: Option<Receiver<SessionEvent>>,
    exited: Arc<AtomicBool>,
    shutdown: CancellationToken,
}

impl Drop for SessionEntry {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[derive(Clone, Debug)]
struct SessionSnapshot {
    commands: mpsc::Sender<SessionCommand>,
    acp_session: acp::SessionId,
    permissions: Arc<PermissionRegistry>,
    barrier: Arc<StreamBarrier>,
    turn_end: mpsc::UnboundedSender<()>,
    exited: Arc<AtomicBool>,
}

#[derive(Debug)]
pub struct AcpHarnessAdapter {
    launch: LaunchTable,
    default_working_dir: Option<PathBuf>,
    sessions: Mutex<BTreeMap<SessionId, SessionEntry>>,
}

impl AcpHarnessAdapter {
    #[must_use]
    pub fn new(config: &Config) -> Self {
        Self {
            launch: LaunchTable::from_config(config),
            default_working_dir: config
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.workspace.clone()),
            sessions: Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn with_launch_table(launch: LaunchTable) -> Self {
        Self {
            launch,
            default_working_dir: None,
            sessions: Mutex::new(BTreeMap::new()),
        }
    }

    fn snapshot(&self, session: &SessionId) -> Result<SessionSnapshot, HarnessAdapterError> {
        let sessions = self.sessions.lock();
        let snapshot = sessions.get(session).map(|entry| SessionSnapshot {
            commands: entry.commands.clone(),
            acp_session: entry.acp_session.clone(),
            permissions: entry.permissions.clone(),
            barrier: entry.barrier.clone(),
            turn_end: entry.turn_end.clone(),
            exited: entry.exited.clone(),
        });
        drop(sessions);
        snapshot.ok_or_else(|| HarnessAdapterError::UnknownSession(session.clone()))
    }

    fn insert(&self, session: SessionId, agent: AgentConnection) {
        let mut sessions = self.sessions.lock();
        sessions.insert(
            session,
            SessionEntry {
                reaped: Some(agent.reaped),
                commands: agent.commands,
                acp_session: agent.acp_session,
                permissions: agent.permissions,
                barrier: agent.barrier,
                turn_end: agent.turn_end,
                events: Some(agent.events_rx),
                exited: agent.exited,
                shutdown: agent.shutdown,
            },
        );
    }

    fn remove(&self, session: &SessionId) {
        let mut sessions = self.sessions.lock();
        sessions.remove(session);
    }

    fn working_dir(
        &self,
        spec: &SessionSpec,
        command: &AcpCommand,
    ) -> Result<PathBuf, HarnessAdapterError> {
        spec.working_dir
            .clone()
            .or_else(|| command.working_dir.clone())
            .or_else(|| self.default_working_dir.clone())
            .ok_or_else(|| port(AcpError::WorkingDir))
    }

    fn launch_command(&self, harness: HarnessId) -> Result<AcpCommand, HarnessAdapterError> {
        self.launch.verified_command(harness).map_err(port)
    }

    fn resume_dir(&self, command: &AcpCommand) -> Result<PathBuf, HarnessAdapterError> {
        command
            .working_dir
            .clone()
            .or_else(|| self.default_working_dir.clone())
            .ok_or_else(|| port(AcpError::WorkingDir))
    }

    fn requested_session(spec: &SessionSpec) -> Result<(), HarnessAdapterError> {
        let Some((prefix, _native)) = spec.session.as_str().split_once(':') else {
            return Err(HarnessAdapterError::InvalidSession(spec.session.clone()));
        };
        match HarnessId::from_str(prefix) {
            Ok(harness) if harness == spec.harness => Ok(()),
            _ => Err(HarnessAdapterError::InvalidSession(spec.session.clone())),
        }
    }
}

#[async_trait]
impl HarnessAdapter for AcpHarnessAdapter {
    async fn release(&self, session: &SessionId) -> Result<(), HarnessAdapterError> {
        let mut entry = self
            .sessions
            .lock()
            .remove(session)
            .ok_or_else(|| HarnessAdapterError::UnknownSession(session.clone()))?;
        entry.shutdown.cancel();
        let reaped = entry
            .reaped
            .take()
            .ok_or_else(|| HarnessAdapterError::UnknownSession(session.clone()))?;
        reaped
            .await
            .map_err(|source| HarnessAdapterError::Failed {
                message: "child reaper closed".into(),
                source: Box::new(source),
            })?
            .map_err(|source| HarnessAdapterError::Failed {
                message: "child release failed".into(),
                source: Box::new(source),
            })
    }
    async fn new_session(&self, spec: SessionSpec) -> Result<SessionId, HarnessAdapterError> {
        Self::requested_session(&spec)?;
        let command = self.launch_command(spec.harness)?;
        let working_dir = self.working_dir(&spec, &command)?;
        let agent = crate::connection::start_new(&command, &working_dir)
            .await
            .map_err(port)?;
        let native = match identity::NativeSession::parse(spec.harness, &agent.acp_session.0) {
            Ok(native) => native,
            Err(error) => {
                agent.shutdown.cancel();
                return Err(port(error));
            }
        };
        let canonical = native.canonical();
        {
            let sessions = self.sessions.lock();
            if sessions.contains_key(&canonical) {
                agent.shutdown.cancel();
                return Err(port(AcpError::SessionActive { session: canonical }));
            }
        }
        self.insert(canonical.clone(), agent);
        if let Some(model) = spec.model
            && let Err(error) = self
                .set_option(&canonical, SessionOption::Model(model))
                .await
        {
            self.remove(&canonical);
            return Err(error);
        }
        if let Some(effort) = spec.effort
            && let Err(error) = self
                .set_option(&canonical, SessionOption::Effort(effort))
                .await
        {
            self.remove(&canonical);
            return Err(error);
        }
        Ok(canonical)
    }

    async fn resume(&self, session: &SessionId) -> Result<(), HarnessAdapterError> {
        let native = identity::from_canonical(session).map_err(port)?;
        let command = self.launch_command(native.harness())?;
        {
            let sessions = self.sessions.lock();
            if sessions.contains_key(session) {
                return Err(port(AcpError::SessionActive {
                    session: session.clone(),
                }));
            }
        }
        let working_dir = self.resume_dir(&command)?;
        let protocol_session = acp::SessionId(native.native().into());
        let agent = crate::connection::start_loaded(&command, &working_dir, &protocol_session)
            .await
            .map_err(port)?;
        self.insert(session.clone(), agent);
        Ok(())
    }

    async fn prompt(
        &self,
        session: &SessionId,
        input: TurnInput,
    ) -> Result<(), HarnessAdapterError> {
        let snapshot = self.snapshot(session)?;
        if snapshot.exited.load(Ordering::Acquire) {
            return Err(port(AcpError::Closed {
                method: "session/prompt",
            }));
        }
        let TurnInput::Text(text) = input;
        let responded = request(&snapshot.commands, |reply| SessionCommand::Prompt {
            acp_session: snapshot.acp_session.clone(),
            text,
            reply,
        })
        .await;
        match responded {
            Ok(response) => {
                tracing::debug!(stop = ?response.stop_reason, "turn ended");
            }
            Err(failure) => {
                let error = if snapshot.exited.load(Ordering::Acquire) {
                    AcpError::Closed {
                        method: "session/prompt",
                    }
                } else {
                    port_failure(failure, "session/prompt")
                };
                return Err(port(error));
            }
        }
        snapshot.barrier.wait().await;
        if snapshot.turn_end.send(()).is_err() {
            tracing::debug!("turn end had no listener");
        }
        Ok(())
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), HarnessAdapterError> {
        let snapshot = self.snapshot(session)?;
        snapshot.permissions.cancel_all();
        snapshot
            .commands
            .send(SessionCommand::Cancel {
                acp_session: snapshot.acp_session,
            })
            .await
            .map_err(|_| {
                port(AcpError::Closed {
                    method: "session/cancel",
                })
            })
    }

    async fn answer_permission(
        &self,
        session: &SessionId,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), HarnessAdapterError> {
        let snapshot = self.snapshot(session)?;
        snapshot
            .permissions
            .answer(request_id, decision)
            .map_err(port)
    }

    async fn set_option(
        &self,
        session: &SessionId,
        option: SessionOption,
    ) -> Result<(), HarnessAdapterError> {
        let snapshot = self.snapshot(session)?;
        let mode = match option {
            SessionOption::Model(model) => model,
            SessionOption::Effort(level) => level.to_string(),
        };
        request(&snapshot.commands, |reply| SessionCommand::SetMode {
            acp_session: snapshot.acp_session.clone(),
            mode,
            reply,
        })
        .await
        .map_err(|failure| port(port_failure(failure, "session/set_mode")))?;
        Ok(())
    }

    async fn events(
        &self,
        session: &SessionId,
    ) -> Result<Receiver<SessionEvent>, HarnessAdapterError> {
        let mut sessions = self.sessions.lock();
        let taken = sessions.get_mut(session).map(|entry| entry.events.take());
        drop(sessions);
        let receiver = taken.ok_or_else(|| HarnessAdapterError::UnknownSession(session.clone()))?;
        receiver.ok_or_else(|| HarnessAdapterError::EventStreamTaken(session.clone()))
    }
}
