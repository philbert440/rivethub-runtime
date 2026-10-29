use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use agent_client_protocol::schema::v1 as acp;
use async_trait::async_trait;
use config::Config;
use kernel::harness::Command;
use kernel::harness::HarnessAdapter;
use kernel::harness::HarnessAdapterError;
use kernel::harness::HarnessCommands;
use kernel::harness::SessionEvent;
use kernel::harness::SessionOption;
use kernel::harness::SessionSpec;
use kernel::harness::SystemPromptSupport;
use kernel::harness::TurnInput;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use wire::gateway::sessions::SessionSwitches;
use wire::harness::ApprovalDecision;
use wire::harness::HarnessId;
use wire::harness::SteerOutcome;
use wire::session_id::SessionId;
use wire::stream::LlmUsage;

use crate::connection::AgentConnection;
use crate::connection::SessionCommand;
use crate::connection::port_failure;
use crate::connection::request;
use crate::error::AcpError;
use crate::error::port;
use crate::identity;
use crate::launch::AcpCommand;
use crate::launch::LaunchTable;
use crate::launch::probe;
use crate::launch::resolve_program;
use crate::options::SessionOptions;
use crate::permissions::PermissionRegistry;
use crate::sheets::SheetInput;
use crate::sheets::sheet_inputs;

#[derive(Debug)]
struct SessionEntry {
    tasks: tokio_util::task::TaskTracker,
    reaped: Option<tokio::sync::oneshot::Receiver<Result<(), std::io::Error>>>,
    commands: mpsc::Sender<SessionCommand>,
    acp_session: acp::SessionId,
    permissions: Arc<PermissionRegistry>,
    options: Arc<SessionOptions>,
    turn_end: mpsc::UnboundedSender<Option<LlmUsage>>,
    events: Option<Receiver<SessionEvent>>,
    exited: Arc<AtomicBool>,
    shutdown: CancellationToken,
    images: bool,
    steering: bool,
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
    options: Arc<SessionOptions>,
    turn_end: mpsc::UnboundedSender<Option<LlmUsage>>,
    exited: Arc<AtomicBool>,
    images: bool,
    steering: bool,
}

#[derive(Debug)]
pub struct AcpHarnessAdapter {
    launch: LaunchTable,
    default_working_dir: Option<PathBuf>,
    sessions: Mutex<BTreeMap<SessionId, SessionEntry>>,
    sheets: BTreeMap<HarnessId, SheetInput>,
    search_paths: Vec<PathBuf>,
    advertised: Mutex<BTreeMap<HarnessId, crate::discovery::Advertisement>>,
    discovered_commands: Mutex<BTreeMap<HarnessId, Vec<Command>>>,
    discovery_dir: Option<PathBuf>,
    failed_discoveries: tokio::sync::Mutex<BTreeMap<HarnessId, Instant>>,
}

const DISCOVERY_RETRY: Duration = Duration::from_secs(60);

impl AcpHarnessAdapter {
    pub(crate) fn headless(
        config: &Config,
        harness: HarnessId,
        command: AcpCommand,
        search_paths: Vec<PathBuf>,
    ) -> Self {
        let mut adapter = Self::new(config).with_search_paths(search_paths);
        adapter.launch = LaunchTable::from_entries([(harness, crate::AcpLaunch::Native(command))]);
        adapter
    }

    pub(crate) fn advertise(
        mut self,
        harness: HarnessId,
        advertisement: crate::discovery::Advertisement,
    ) -> Self {
        self.discovered_commands
            .get_mut()
            .insert(harness, advertisement.commands().to_vec());
        self.advertised.get_mut().insert(harness, advertisement);
        self
    }

    #[must_use]
    pub fn new(config: &Config) -> Self {
        Self {
            launch: LaunchTable::from_config(config),
            default_working_dir: config
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.workspace.clone()),
            sessions: Mutex::new(BTreeMap::new()),
            sheets: sheet_inputs(config),
            search_paths: Vec::new(),
            advertised: Mutex::new(BTreeMap::new()),
            discovered_commands: Mutex::new(BTreeMap::new()),
            discovery_dir: None,
            failed_discoveries: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn with_launch_table(launch: LaunchTable) -> Self {
        Self {
            launch,
            default_working_dir: None,
            sessions: Mutex::new(BTreeMap::new()),
            sheets: BTreeMap::new(),
            search_paths: Vec::new(),
            advertised: Mutex::new(BTreeMap::new()),
            discovered_commands: Mutex::new(BTreeMap::new()),
            discovery_dir: None,
            failed_discoveries: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn with_discovery_dir(mut self, directory: PathBuf) -> Self {
        self.discovery_dir = Some(directory);
        self
    }

    #[must_use]
    pub fn with_search_paths(mut self, search_paths: Vec<PathBuf>) -> Self {
        self.search_paths = search_paths;
        self
    }

    fn snapshot(&self, session: &SessionId) -> Result<SessionSnapshot, HarnessAdapterError> {
        let sessions = self.sessions.lock();
        let snapshot = sessions.get(session).map(|entry| SessionSnapshot {
            commands: entry.commands.clone(),
            acp_session: entry.acp_session.clone(),
            permissions: entry.permissions.clone(),
            options: Arc::clone(&entry.options),
            turn_end: entry.turn_end.clone(),
            exited: entry.exited.clone(),
            images: entry.images,
            steering: entry.steering,
        });
        drop(sessions);
        snapshot.ok_or_else(|| HarnessAdapterError::UnknownSession(session.clone()))
    }

    fn insert(&self, session: SessionId, agent: AgentConnection) {
        let mut sessions = self.sessions.lock();
        sessions.insert(
            session,
            SessionEntry {
                tasks: agent.tasks,
                reaped: Some(agent.reaped),
                commands: agent.commands,
                acp_session: agent.acp_session,
                permissions: agent.permissions,
                options: agent.options,
                turn_end: agent.turn_end,
                events: Some(agent.events_rx),
                exited: agent.exited,
                shutdown: agent.shutdown,
                images: agent.capabilities.prompt_capabilities.image,
                steering: agent.steering,
            },
        );
    }

    fn session_options(&self, session: &SessionId) -> Arc<SessionOptions> {
        let live = self
            .sessions
            .lock()
            .get(session)
            .map(|entry| Arc::clone(&entry.options));
        live.or_else(|| {
            self.advertised
                .lock()
                .get(&session.harness())
                .map(crate::discovery::Advertisement::options)
        })
        .unwrap_or_default()
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
        let command = self.launch.verified_command(harness).map_err(port)?;
        self.located(command)
    }

    fn located(&self, mut command: AcpCommand) -> Result<AcpCommand, HarnessAdapterError> {
        if self.search_paths.is_empty() {
            return Ok(command);
        }
        let found = resolve_program(&command.program, &self.search_paths).map_err(|error| {
            HarnessAdapterError::Failed {
                message: "install probe failed".into(),
                source: Box::new(error),
            }
        })?;
        if let Some(path) = found {
            command.program = path;
        }
        Ok(command)
    }

    fn resume_dir(&self, command: &AcpCommand) -> Result<PathBuf, HarnessAdapterError> {
        command
            .working_dir
            .clone()
            .or_else(|| self.default_working_dir.clone())
            .ok_or_else(|| port(AcpError::WorkingDir))
    }

    async fn discovered(
        &self,
        harness: HarnessId,
        command: &AcpCommand,
        cwd: &Path,
    ) -> Option<crate::discovery::Advertisement> {
        let mut failed = self.failed_discoveries.lock().await;
        let known = self.advertised.lock().get(&harness).cloned();
        let due = failed
            .get(&harness)
            .is_none_or(|at| at.elapsed() >= DISCOVERY_RETRY);
        if known.is_some() || !due {
            return known;
        }
        match crate::discovery::discover(command, cwd, crate::discovery::DEFAULT_DISCOVERY_TIMEOUT)
            .await
        {
            Ok(advertised) => {
                failed.remove(&harness);
                self.discovered_commands
                    .lock()
                    .insert(harness, advertised.commands().to_vec());
                self.advertised.lock().insert(harness, advertised.clone());
                Some(advertised)
            }
            Err(error) => {
                tracing::warn!(%harness, %error, "harness discovery failed; offering it without chat");
                failed.insert(harness, Instant::now());
                None
            }
        }
    }

    fn requested_session(spec: &SessionSpec) -> Result<(), HarnessAdapterError> {
        if spec.session.harness() == spec.harness {
            Ok(())
        } else {
            Err(HarnessAdapterError::InvalidSession(spec.session.clone()))
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
        let outcome = reaped
            .await
            .map_err(|source| HarnessAdapterError::Failed {
                message: "child reaper closed".into(),
                source: Box::new(source),
            })?
            .map_err(|source| HarnessAdapterError::Failed {
                message: "child release failed".into(),
                source: Box::new(source),
            });
        entry.tasks.wait().await;
        outcome
    }
    fn system_prompt(&self, harness: HarnessId) -> SystemPromptSupport {
        crate::launch::system_prompt(harness)
    }

    async fn new_session(&self, spec: SessionSpec) -> Result<SessionId, HarnessAdapterError> {
        Self::requested_session(&spec)?;
        let command = self.launch_command(spec.harness)?;
        let working_dir = self.working_dir(&spec, &command)?;
        let meta = match spec.startup_context.as_deref() {
            Some(text) => crate::launch::startup_meta(spec.harness, text).map_err(port)?,
            None => None,
        };
        let agent = crate::connection::start_new(&command, &working_dir, meta)
            .await
            .map_err(port)?;
        let native =
            match identity::NativeSession::parse(spec.harness, agent.acp_session.0.as_ref()) {
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
        self.advertised.lock().insert(
            spec.harness,
            crate::discovery::Advertisement::from_agent(&agent),
        );
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
        let protocol_session = acp::SessionId::new(native.native());
        let agent = crate::connection::start_loaded(&command, &working_dir, &protocol_session)
            .await
            .map_err(port)?;
        self.advertised.lock().insert(
            native.harness(),
            crate::discovery::Advertisement::from_agent(&agent),
        );
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
        let content = crate::prompt::content(input, snapshot.images)
            .await
            .map_err(port)?;
        let responded = request(&snapshot.commands, |reply| SessionCommand::Prompt {
            acp_session: snapshot.acp_session.clone(),
            content,
            reply,
        })
        .await;
        let usage = match responded {
            Ok(response) => {
                tracing::debug!(stop = ?response.stop_reason, "turn ended");
                crate::events::turn_usage(&response).map_err(port)?
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
        };
        if snapshot.turn_end.send(usage).is_err() {
            tracing::debug!("turn end had no listener");
        }
        Ok(())
    }

    async fn steer(
        &self,
        session: &SessionId,
        input: TurnInput,
    ) -> Result<SteerOutcome, HarnessAdapterError> {
        let snapshot = self.snapshot(session)?;
        if !snapshot.steering {
            return Err(HarnessAdapterError::SteeringUnsupported);
        }
        let content = crate::prompt::content(input, snapshot.images)
            .await
            .map_err(port)?;
        request(&snapshot.commands, |reply| SessionCommand::Steer {
            acp_session: snapshot.acp_session.clone(),
            content,
            reply,
        })
        .await
        .map_err(|failure| port(port_failure(failure, crate::steering::METHOD)))?
        .outcome()
        .map_err(port)
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
        let selected = snapshot.options.select(&option).map_err(port)?;
        let response = request(&snapshot.commands, |reply| SessionCommand::SetConfig {
            acp_session: snapshot.acp_session.clone(),
            config_id: selected.config_id,
            value: selected.value,
            reply,
        })
        .await
        .map_err(|failure| port(port_failure(failure, "session/set_config_option")))?;
        snapshot.options.replace(response.config_options);
        Ok(())
    }

    fn switches(&self, session: &SessionId) -> SessionSwitches {
        self.session_options(session).switches()
    }

    fn admit(
        &self,
        session: &SessionId,
        option: &SessionOption,
    ) -> Result<(), HarnessAdapterError> {
        self.session_options(session)
            .select(option)
            .map(drop)
            .map_err(port)
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

    async fn offerings(&self) -> Result<Vec<wire::harness::HarnessOffering>, HarnessAdapterError> {
        let facts = self.launch.facts();
        let search = self.search_paths.clone();
        let installed = tokio::task::spawn_blocking(move || probe(&facts, &search))
            .await
            .map_err(|error| HarnessAdapterError::Failed {
                message: "install probe failed".into(),
                source: Box::new(error),
            })?
            .map_err(|error| HarnessAdapterError::Failed {
                message: "install probe failed".into(),
                source: Box::new(error),
            })?;
        let mut offerings = Vec::new();
        for (fact, present) in installed {
            if !present {
                continue;
            }
            let mut offering = crate::sheets::offering(
                fact.harness,
                fact.verified_chat,
                self.sheets.get(&fact.harness),
            )
            .map_err(|error| HarnessAdapterError::Failed {
                message: error.to_string(),
                source: Box::new(error),
            })?;
            let known = self.advertised.lock().get(&fact.harness).cloned();
            if let Some(advertised) = known {
                advertised
                    .apply(&mut offering, self.sheets.get(&fact.harness))
                    .map_err(port)?;
            } else {
                let launch = self.launch.launch(fact.harness).map_err(port)?;
                if launch.is_verified() {
                    let command = &self.located(launch.command().clone())?;
                    if let Some(cwd) = self
                        .discovery_dir
                        .as_ref()
                        .or(command.working_dir.as_ref())
                        .or(self.default_working_dir.as_ref())
                    {
                        match self.discovered(fact.harness, command, cwd).await {
                            Some(advertised) => advertised
                                .apply(&mut offering, self.sheets.get(&fact.harness))
                                .map_err(port)?,
                            None => offering.chat = false,
                        }
                    }
                }
            }
            offerings.push(offering);
        }
        Ok(offerings)
    }

    async fn commands(&self) -> Result<Vec<HarnessCommands>, HarnessAdapterError> {
        let offered = self.offerings().await?;
        let discovered = self.discovered_commands.lock();
        Ok(offered
            .iter()
            .filter_map(|offering| {
                discovered
                    .get(&offering.harness_id)
                    .map(|commands| HarnessCommands {
                        harness: offering.harness_id,
                        commands: commands.clone(),
                    })
            })
            .collect())
    }
}
