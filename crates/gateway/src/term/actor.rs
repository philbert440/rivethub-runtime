use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use kernel::terminal::{TermSize, TerminalId};
use tokio::sync::{mpsc, oneshot, watch};
use wire::events::{AgentEvent, AgentEventKind, ProtocolVersion};
use wire::gateway::term::{PtyInfo, PtyState, TermSpawnResponse};

use super::{Launch, TerminalService};
use crate::error::ApiError;
use crate::tenancy::UserId;

type Reply<T> = oneshot::Sender<Result<T, ApiError>>;

#[derive(Debug)]
pub(super) enum Command {
    Spawn {
        launch: Launch,
        reply: Reply<TermSpawnResponse>,
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
                    if current { self.permit.take(); }
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
    ) -> Result<TermSpawnResponse, ApiError> {
        let previous = self.status.borrow().clone();
        if let Some(state) = previous
            && state.state == PtyState::Running
        {
            return response(state);
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
        let mut argv = launch.command.argv.clone();
        if let Some(resume) = launch.resume {
            argv.extend(["--resume".into(), resume]);
        }
        if let Some(model) = launch.model {
            argv.extend(["--model".into(), model]);
        }
        if let Some(effort) = launch.effort {
            argv.extend(["--effort".into(), effort]);
        }
        self.service.trust_claude_directory(&launch.command).await?;
        let port_session = TerminalService::port_session(&launch.command, &self.session)?;
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
        Ok(response)
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
