use std::collections::BTreeMap;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

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
use crate::connection::port_failure;
use crate::connection::request;
use crate::error::AcpError;
use crate::error::port;
use crate::identity;
use crate::launch::AcpCommand;
use crate::launch::LaunchFact;
use crate::launch::LaunchTable;
use crate::options::SessionOptions;
use crate::permissions::PermissionRegistry;
use crate::sheets::SheetInput;

#[derive(Debug)]
struct SessionEntry {
    tasks: tokio_util::task::TaskTracker,
    reaped: Option<tokio::sync::oneshot::Receiver<Result<(), std::io::Error>>>,
    commands: mpsc::Sender<SessionCommand>,
    acp_session: acp::SessionId,
    permissions: Arc<PermissionRegistry>,
    options: Arc<SessionOptions>,
    turn_end: mpsc::UnboundedSender<()>,
    events: Option<Receiver<SessionEvent>>,
    exited: Arc<AtomicBool>,
    shutdown: CancellationToken,
    images: bool,
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
    turn_end: mpsc::UnboundedSender<()>,
    exited: Arc<AtomicBool>,
    images: bool,
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
}

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
        }
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
    async fn new_session(&self, spec: SessionSpec) -> Result<SessionId, HarnessAdapterError> {
        Self::requested_session(&spec)?;
        let command = self.launch_command(spec.harness)?;
        let working_dir = self.working_dir(&spec, &command)?;
        let agent = crate::connection::start_new(&command, &working_dir)
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
                    if let Some(cwd) = command
                        .working_dir
                        .as_ref()
                        .or(self.default_working_dir.as_ref())
                    {
                        let discovered = crate::discovery::discover(
                            command,
                            cwd,
                            crate::discovery::DEFAULT_DISCOVERY_TIMEOUT,
                        )
                        .await;
                        let advertised = match discovered {
                            Ok(advertised) => advertised,
                            Err(error) => {
                                tracing::warn!(harness = %fact.harness, %error, "harness discovery failed; offering it without chat");
                                offering.chat = false;
                                offerings.push(offering);
                                continue;
                            }
                        };
                        advertised
                            .apply(&mut offering, self.sheets.get(&fact.harness))
                            .map_err(port)?;
                        self.discovered_commands
                            .lock()
                            .insert(fact.harness, advertised.commands().to_vec());
                        self.advertised.lock().insert(fact.harness, advertised);
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

fn sheet_inputs(config: &Config) -> BTreeMap<HarnessId, SheetInput> {
    let Some(tasks) = &config.tasks else {
        return BTreeMap::new();
    };
    let Some(harnesses) = &tasks.harnesses else {
        return BTreeMap::new();
    };
    harnesses
        .iter()
        .filter_map(|(slot, entry)| match slot {
            config::HarnessSlot::Known(harness) => Some((
                *harness,
                SheetInput {
                    models: entry.models.clone(),
                    efforts: entry.efforts.clone(),
                    mode: entry.models_mode,
                },
            )),
            config::HarnessSlot::Unknown(_) => None,
        })
        .collect()
}

fn probe(
    facts: &[LaunchFact],
    search: &[PathBuf],
) -> Result<Vec<(LaunchFact, bool)>, std::io::Error> {
    let mut installed = Vec::with_capacity(facts.len());
    for fact in facts {
        let present = match &fact.program {
            Some(program) => program_installed(program, search)?,
            None => false,
        };
        installed.push((fact.clone(), present));
    }
    Ok(installed)
}

fn program_installed(program: &Path, search: &[PathBuf]) -> Result<bool, std::io::Error> {
    Ok(resolve_program(program, search)?.is_some())
}

pub(crate) fn resolve_program(
    program: &Path,
    search: &[PathBuf],
) -> Result<Option<PathBuf>, std::io::Error> {
    if program.as_os_str().is_empty()
        || program
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Ok(None);
    }
    if program.is_absolute() {
        return Ok(is_file(program)?.then(|| program.to_owned()));
    }
    for directory in search {
        let candidate = directory.join(program);
        if is_file(&candidate)? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn is_file(path: &Path) -> Result<bool, std::io::Error> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
