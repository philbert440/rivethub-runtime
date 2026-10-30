use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use kernel::terminal::{TermSize, TerminalId};
use kernel::topics::DeliveredVia;
use tokio::sync::{mpsc, oneshot, watch};
use wire::events::{AgentEvent, AgentEventKind, ProtocolVersion};
use wire::gateway::term::{PtyInfo, PtyState, TermSpawnResponse};

use super::{Launch, Spawned, TerminalService};
use crate::error::ApiError;
use crate::tenancy::UserId;

type Reply<T> = oneshot::Sender<Result<T, ApiError>>;

#[derive(Debug)]
pub(super) enum Command {
    Spawn {
        launch: Launch,
        reply: Reply<Spawned>,
    },
    Kill {
        reply: Reply<()>,
    },
    Join {
        client: uuid::Uuid,
        device: String,
        reply: Reply<()>,
    },
    Leave {
        client: uuid::Uuid,
    },
    Size {
        client: uuid::Uuid,
        size: Option<TermSize>,
        claim: bool,
        reply: Reply<()>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Owner {
    pub(super) client: uuid::Uuid,
    pub(super) device: String,
    pub(super) since: chrono::DateTime<Utc>,
}

#[derive(Debug)]
struct Viewer {
    size: TermSize,
    device: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Handle {
    pub(super) commands: mpsc::Sender<Command>,
    pub(super) state: watch::Receiver<Option<PtyInfo>>,
    pub(super) ownership: watch::Receiver<Option<Owner>>,
}

impl Handle {
    pub(super) async fn send(&self, command: Command) -> Result<(), ApiError> {
        self.commands
            .send(command)
            .await
            .map_err(|_| ApiError::Unavailable("terminal actor closed".into()))
    }
}

pub(super) struct TerminalActor {
    service: Arc<TerminalService>,
    user: UserId,
    session: String,
    status: watch::Sender<Option<PtyInfo>>,
    ownership: watch::Sender<Option<Owner>>,
    clients: BTreeMap<uuid::Uuid, Viewer>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl TerminalActor {
    pub(super) fn new(
        service: Arc<TerminalService>,
        user: UserId,
        session: String,
        status: watch::Sender<Option<PtyInfo>>,
        ownership: watch::Sender<Option<Owner>>,
    ) -> Self {
        Self {
            service,
            user,
            session,
            status,
            ownership,
            clients: BTreeMap::new(),
            permit: None,
        }
    }

    pub(super) async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
    ) -> Result<(), ApiError> {
        let mut exits = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                () = self.service.shutdown.cancelled() => break,
                command = commands.recv() => {
                    let Some(command) = command else { break; };
                    match command {
                        Command::Spawn { launch, reply } => {
                            let result = self.spawn(launch, &mut exits).await;
                            respond(reply, result);
                        }
                        Command::Kill { reply } => respond(reply, self.kill().await),
                        Command::Join { client, device, reply } => {
                            let state = self.status.borrow().clone().ok_or(ApiError::NotFound("unknown pty"))?;
                            let size = TermSize { cols: state.cols.ok_or(ApiError::NotFound("terminal size"))?, rows: state.rows.ok_or(ApiError::NotFound("terminal size"))? };
                            self.clients.insert(client, Viewer { size, device });
                            self.attached();
                            respond(reply, Ok(()));
                        }
                        Command::Leave { client } => {
                            self.clients.remove(&client);
                            self.attached();
                            if self.ownership.borrow().as_ref().is_some_and(|owner| owner.client == client) {
                                self.ownership.send_replace(None);
                                if self.clients.len() == 1
                                    && let Some((&client, viewer)) = self.clients.first_key_value()
                                { let size = viewer.size; self.resize(client, Some(size), true).await?; }
                            }
                        }
                        Command::Size { client, size, claim, reply } => respond(reply, self.resize(client, size, claim).await),
                    }
                }
                outcome = exits.join_next(), if !exits.is_empty() => {
                    let (id, code) = outcome.ok_or(ApiError::NotFound("terminal exit task"))?.map_err(ApiError::Task)??;
                    let mut current = false;
                    self.status.send_modify(|state| {
                        if let Some(state) = state && state.id == id.as_str() {
                            current = true;
                            state.state = PtyState::Exited;
                            state.exit_code = Some(code);
                        }
                    });
                    if current { self.permit.take(); self.service.forget_startup(&self.session).await; }
                }
            }
        }
        if self
            .status
            .borrow()
            .as_ref()
            .is_some_and(|state| state.state == PtyState::Running)
        {
            self.kill().await?;
        }
        exits.abort_all();
        self.service.forget_startup(&self.session).await;
        while let Some(outcome) = exits.join_next().await {
            match outcome {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => return Err(error),
                Err(error) if error.is_cancelled() => {}
                Err(error) => return Err(ApiError::Task(error)),
            }
        }
        Ok(())
    }

    async fn spawn(
        &mut self,
        launch: Launch,
        exits: &mut tokio::task::JoinSet<Result<(TerminalId, i32), ApiError>>,
    ) -> Result<Spawned, ApiError> {
        let port_session = TerminalService::port_session(&launch.command, &self.session)?;
        let previous = self.status.borrow().clone();
        if let Some(state) = previous
            && state.state == PtyState::Running
        {
            return Ok(Spawned {
                response: response(state)?,
                session: port_session,
                startup: None,
            });
        }
        let permit = self
            .service
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                ApiError::Conflict(format!(
                    "pty limit reached ({}); all active",
                    self.service.options.max_ptys
                ))
            })?;
        let (argv, startup) = self.argv(&launch).await?;
        self.service.trust_claude_directory(&launch.command).await?;
        let id = self
            .service
            .mux
            .spawn(
                &port_session,
                argv,
                &launch.command.cwd,
                &launch.command.env,
                launch.size,
            )
            .await?;
        let details = match self.service.mux.inspect(&id).await {
            Ok(details) => details,
            Err(error) => {
                self.service.mux.kill(&id).await?;
                return Err(error.into());
            }
        };
        let state = PtyInfo {
            id: id.to_string(),
            session: self.session.clone(),
            command: launch.command.id.clone(),
            state: PtyState::Running,
            pid: details.pid,
            attached: 0,
            exit_code: details.exit_code,
            created_at: Utc::now(),
            last_output_ts: None,
            cols: Some(details.size.cols),
            rows: Some(details.size.rows),
            mux: None,
            persisted: None,
            reattached: None,
            attach: None,
        };
        let response = response(state.clone())?;
        self.status.send_replace(Some(state));
        self.permit = Some(permit);
        self.register(&launch.command, &id)?;
        let mux = self.service.mux.clone();
        exits.spawn(async move {
            let code = mux.wait(&id).await?;
            Ok((id, code))
        });
        Ok(Spawned {
            response,
            session: port_session,
            startup,
        })
    }

    async fn argv(&self, launch: &Launch) -> Result<(Vec<String>, Option<DeliveredVia>), ApiError> {
        let harness = launch.command.harness;
        let resume = self
            .service
            .resume(harness, &self.session, launch.resume.clone())
            .await?;
        let fresh = resume.is_none();
        let mut argv = launch.command.argv.clone();
        argv.extend(session_args(harness, &self.session, resume));
        let startup = match (
            startup_args(harness, launch.startup.as_deref(), fresh),
            harness,
        ) {
            (Some(inline), Some(harness)) => Some(
                self.service
                    .startup_file(harness, &self.session, inline)
                    .await?,
            ),
            (startup, _) => startup,
        };
        let delivered = match &startup {
            Some(_) => DeliveredVia::CliFlag,
            None => DeliveredVia::None,
        };
        argv.extend(startup.into_iter().flatten());
        if let Some(model) = &launch.model {
            argv.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &launch.effort {
            argv.extend(["--effort".into(), effort.clone()]);
        }
        Ok((argv, fresh.then_some(delivered)))
    }

    fn register(&self, command: &super::LaunchCommand, id: &TerminalId) -> Result<(), ApiError> {
        if command.session {
            self.service.registry.ingest(
                &self.user,
                vec![AgentEvent {
                    v: ProtocolVersion::V1,
                    session: self.session.clone(),
                    kind: AgentEventKind::SessionStart {
                        title: None,
                        name: None,
                        harness: command.harness.map(Into::into),
                        ts: Some(Utc::now()),
                    },
                    extra: wire::Extra::default(),
                }],
            )?;
            self.service
                .registry
                .bind_terminal(&self.user, &self.session, id.as_str())?;
        }
        Ok(())
    }

    async fn kill(&mut self) -> Result<(), ApiError> {
        let state = self
            .status
            .borrow()
            .clone()
            .ok_or(ApiError::NotFound("unknown pty"))?;
        let id = TerminalId::new(&state.id);
        self.service.mux.kill(&id).await?;
        if state.state == PtyState::Running {
            let code = self.service.mux.wait(&id).await?;
            self.status.send_modify(|state| {
                if let Some(state) = state {
                    state.state = PtyState::Exited;
                    state.exit_code = Some(code);
                }
            });
        } else {
            self.status.send_replace(None);
        }
        self.permit.take();
        Ok(())
    }

    fn attached(&self) {
        self.status.send_modify(|state| {
            if let Some(state) = state {
                state.attached = self.clients.len() as u64;
            }
        });
    }

    async fn resize(
        &mut self,
        client: uuid::Uuid,
        size: Option<TermSize>,
        claim: bool,
    ) -> Result<(), ApiError> {
        let viewer = self
            .clients
            .get_mut(&client)
            .ok_or(ApiError::Unauthorized)?;
        if let Some(size) = size {
            viewer.size = TermSize {
                cols: size.cols.clamp(20, 400),
                rows: size.rows.clamp(10, 200),
            };
        }
        let size = viewer.size;
        let device = viewer.device.clone();
        let owner = self.ownership.borrow().clone();
        if claim || owner.is_none() {
            if owner.as_ref().is_none_or(|owner| owner.client != client) {
                self.ownership.send_replace(Some(Owner {
                    client,
                    device,
                    since: Utc::now(),
                }));
            }
        } else if owner.as_ref().is_some_and(|owner| owner.client != client) {
            return Ok(());
        }
        let state = self
            .status
            .borrow()
            .clone()
            .ok_or(ApiError::NotFound("unknown pty"))?;
        if state.cols != Some(size.cols) || state.rows != Some(size.rows) {
            self.service
                .mux
                .resize(&TerminalId::new(state.id), size)
                .await?;
            self.status.send_modify(|state| {
                if let Some(state) = state {
                    state.cols = Some(size.cols);
                    state.rows = Some(size.rows);
                }
            });
        }
        Ok(())
    }
}

fn session_args(
    harness: Option<wire::harness::HarnessId>,
    session: &str,
    resume: Option<String>,
) -> Vec<String> {
    let flags = harness.and_then(wire::gateway::roster::session_flags);
    match (resume, flags) {
        (Some(resume), Some(flags)) => vec![flags.resume.into(), resume],
        (Some(resume), None) => vec!["--resume".into(), resume],
        (None, Some(flags)) => match flags.pin {
            Some(pin) if session.len() == 36 && uuid::Uuid::try_parse(session).is_ok() => {
                vec![pin.into(), session.into()]
            }
            _ => Vec::new(),
        },
        (None, None) => Vec::new(),
    }
}

fn startup_args(
    harness: Option<wire::harness::HarnessId>,
    startup: Option<&str>,
    fresh: bool,
) -> Option<[String; 2]> {
    let (harness, text) = harness.zip(startup).filter(|_| fresh)?;
    kernel::harness::cli_system_prompt(harness).cli_args(text)
}

fn respond<T>(reply: Reply<T>, result: Result<T, ApiError>) {
    if reply.send(result).is_err() {
        tracing::debug!("terminal request caller disconnected");
    }
}

fn response(state: PtyInfo) -> Result<TermSpawnResponse, ApiError> {
    Ok(TermSpawnResponse {
        harness_session_id: None,
        id: state.id,
        session: state.session,
        command: state.command,
        cwd: None,
        pid: state
            .pid
            .ok_or_else(|| ApiError::Unavailable("terminal process id unavailable".into()))?,
        created_at: state.created_at,
        mux: state.mux,
        reattached: None,
        attach: None,
    })
}

#[cfg(test)]
mod tests {
    use wire::harness::HarnessId;

    use super::session_args;
    use super::startup_args;

    const UUID: &str = "8922b6dd-d60f-42d9-9690-51b0d6e63176";

    fn args(harness: HarnessId, session: &str, resume: Option<&str>) -> Vec<String> {
        session_args(Some(harness), session, resume.map(str::to_owned))
    }

    #[test]
    fn a_fresh_claude_launch_pins_the_registered_session() {
        assert_eq!(
            args(HarnessId::ClaudeCode, UUID, None),
            ["--session-id", UUID]
        );
    }

    #[test]
    fn a_claude_resume_resumes_without_pinning() {
        assert_eq!(
            args(HarnessId::ClaudeCode, UUID, Some(UUID)),
            ["--resume", UUID]
        );
    }

    #[test]
    fn each_harness_resumes_with_its_own_flag() {
        assert_eq!(args(HarnessId::Codex, UUID, Some(UUID)), ["resume", UUID]);
        assert_eq!(
            args(HarnessId::KimiCode, UUID, Some(UUID)),
            ["--session", UUID]
        );
        assert_eq!(args(HarnessId::Omp, UUID, Some(UUID)), ["--resume", UUID]);
    }

    #[test]
    fn a_harness_without_a_pin_flag_starts_bare() {
        for harness in [
            HarnessId::Codex,
            HarnessId::KimiCode,
            HarnessId::Hermes,
            HarnessId::Opencode,
            HarnessId::Omp,
            HarnessId::Cursor,
        ] {
            assert_eq!(args(harness, UUID, None), Vec::<String>::new());
        }
    }

    #[test]
    fn a_non_uuid_session_is_not_pinned() {
        let generated = format!("rivethub-{UUID}");
        assert_eq!(
            args(HarnessId::ClaudeCode, &generated, None),
            Vec::<String>::new()
        );
        assert_eq!(session_args(None, UUID, None), Vec::<String>::new());
    }

    #[test]
    fn a_fresh_claude_launch_appends_the_startup_memory_and_a_resume_does_not() {
        let memory = "## Memory for this session (topic: Mesh)";
        assert_eq!(
            startup_args(Some(HarnessId::ClaudeCode), Some(memory), true),
            Some(["--append-system-prompt".to_owned(), memory.to_owned()])
        );
        assert_eq!(
            startup_args(Some(HarnessId::ClaudeCode), Some(memory), false),
            None
        );
    }
}
