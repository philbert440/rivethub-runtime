use std::collections::VecDeque;
use std::sync::Arc;

use kernel::harness::{HarnessAdapter, SessionEvent, TurnAttachment, TurnInput};
use tokio::sync::{mpsc, oneshot};
use wire::events::{AgentEvent, AgentEventKind, ProtocolVersion};
use wire::gateway::term::TermSpawnResponse;
use wire::harness::HarnessSessionStatus;
use wire::session_id::SessionId;

use super::fanout::ToolNames;
use crate::error::ApiError;
use crate::tenancy::UserId;
use crate::term::{Launch, TerminalService};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    Chat,
    Terminal,
    Idle,
}

#[derive(Debug)]
enum Request {
    Close {
        reply: oneshot::Sender<Result<(), ApiError>>,
    },
    Prompt {
        turn: Queued,
        reply: oneshot::Sender<Result<(), ApiError>>,
    },
    Terminal {
        launch: Launch,
        reply: oneshot::Sender<Result<TermSpawnResponse, ApiError>>,
    },
    TurnEnd,
    Stop {
        reply: oneshot::Sender<Result<(), ApiError>>,
    },
}

#[derive(Debug, Clone)]
pub struct ChatHandle {
    commands: mpsc::Sender<Request>,
}

impl ChatHandle {
    pub async fn close(&self) -> Result<(), ApiError> {
        let (reply, result) = oneshot::channel();
        self.send(Request::Close { reply }).await?;
        result.await.map_err(|_| closed())?
    }
    pub(crate) async fn terminal(&self, launch: Launch) -> Result<TermSpawnResponse, ApiError> {
        let (reply, result) = oneshot::channel();
        self.send(Request::Terminal { launch, reply }).await?;
        result.await.map_err(|_| closed())?
    }

    pub async fn prompt(
        &self,
        text: String,
        system_prompt: Option<String>,
        attachments: Vec<TurnAttachment>,
    ) -> Result<(), ApiError> {
        let (reply, result) = oneshot::channel();
        let turn = Queued {
            text,
            system_prompt,
            attachments,
        };
        self.send(Request::Prompt { turn, reply }).await?;
        result.await.map_err(|_| closed())?
    }

    pub async fn stop(&self) -> Result<(), ApiError> {
        let (reply, result) = oneshot::channel();
        self.send(Request::Stop { reply }).await?;
        result.await.map_err(|_| closed())?
    }

    pub async fn turn_end(&self) -> Result<(), ApiError> {
        self.send(Request::TurnEnd).await
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.commands.is_closed()
    }

    async fn send(&self, request: Request) -> Result<(), ApiError> {
        self.commands.send(request).await.map_err(|_| closed())
    }
}

#[derive(Debug)]
struct Queued {
    text: String,
    system_prompt: Option<String>,
    attachments: Vec<TurnAttachment>,
}

impl Queued {
    fn input(self) -> TurnInput {
        let text = with_system_prompt(self.system_prompt.as_deref(), self.text);
        if self.attachments.is_empty() {
            return TurnInput::Text(text);
        }
        TurnInput::Attached {
            text,
            attachments: self.attachments,
        }
    }
}

const SYSTEM_PROMPT_HEADING: &str = "[System instructions]";
const SYSTEM_PROMPT_MAX_CHARS: usize = 16_384;

pub(crate) fn with_system_prompt(system_prompt: Option<&str>, text: String) -> String {
    let prompt: String = system_prompt
        .unwrap_or_default()
        .trim()
        .chars()
        .take(SYSTEM_PROMPT_MAX_CHARS)
        .collect();
    if prompt.is_empty() {
        return text;
    }
    format!("{SYSTEM_PROMPT_HEADING}\n{prompt}\n\n{text}")
}

fn closed() -> ApiError {
    ApiError::Unavailable("session actor closed".into())
}

type PendingTerminal = (Launch, oneshot::Sender<Result<TermSpawnResponse, ApiError>>);

pub(crate) struct ChatActor {
    service: Arc<TerminalService>,
    harness: Arc<dyn HarnessAdapter>,
    user: UserId,
    session: SessionId,
    key: String,
    surface: Surface,
    terminal_busy: bool,
    turn_ended: bool,
    pending: Option<PendingTerminal>,
    queued: VecDeque<Queued>,
    events: Option<mpsc::Receiver<SessionEvent>>,
    closing: Option<oneshot::Sender<Result<(), ApiError>>>,
    tools: ToolNames,
}

impl ChatActor {
    pub(crate) fn start(
        service: &Arc<TerminalService>,
        harness: Arc<dyn HarnessAdapter>,
        user: UserId,
        session: SessionId,
        events: Option<mpsc::Receiver<SessionEvent>>,
    ) -> Result<ChatHandle, ApiError> {
        let surface = if events.is_some() {
            Surface::Chat
        } else {
            Surface::Terminal
        };
        Self::spawn(service, harness, user, session, events, surface)
    }

    pub(crate) fn idle(
        service: &Arc<TerminalService>,
        harness: Arc<dyn HarnessAdapter>,
        user: UserId,
        session: SessionId,
    ) -> Result<ChatHandle, ApiError> {
        Self::spawn(service, harness, user, session, None, Surface::Idle)
    }

    fn spawn(
        service: &Arc<TerminalService>,
        harness: Arc<dyn HarnessAdapter>,
        user: UserId,
        session: SessionId,
        events: Option<mpsc::Receiver<SessionEvent>>,
        surface: Surface,
    ) -> Result<ChatHandle, ApiError> {
        let key = service.registry.authorize(&user, session.as_str())?;
        let (commands, requests) = mpsc::channel(64);
        let actor = Self {
            service: service.clone(),
            harness,
            user,
            session,
            key,
            surface,
            terminal_busy: surface == Surface::Terminal,
            turn_ended: true,
            pending: None,
            queued: VecDeque::new(),
            events,
            closing: None,
            tools: ToolNames::default(),
        };
        service.track(async move {
            if let Err(error) = actor.run(requests).await {
                tracing::error!(%error, "session actor failed");
            }
        });
        Ok(ChatHandle { commands })
    }

    async fn run(mut self, mut requests: mpsc::Receiver<Request>) -> Result<(), ApiError> {
        let mut turn = tokio::task::JoinSet::new();
        let outcome = self.drive(&mut requests, &mut turn).await;
        if outcome.is_err() {
            self.service
                .set_live_status(&self.key, &self.session, HarnessSessionStatus::Error);
        }
        turn.abort_all();
        while let Some(result) = turn.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::error!(%error, "session prompt failed during shutdown"),
                Err(error) if error.is_cancelled() => {}
                Err(error) => tracing::error!(%error, "session prompt task failed"),
            }
        }
        let cleanup = match self.surface {
            Surface::Chat => self
                .harness
                .release(&self.session)
                .await
                .map_err(ApiError::from),
            Surface::Terminal => self.service.remove_terminal(&self.user, &self.key).await,
            Surface::Idle => Ok(()),
        };
        let result = outcome.and(cleanup);
        if let Some(reply) = self.closing {
            respond(reply, result);
            Ok(())
        } else {
            result
        }
    }

    async fn drive(
        &mut self,
        requests: &mut mpsc::Receiver<Request>,
        turn: &mut tokio::task::JoinSet<Result<(), ApiError>>,
    ) -> Result<(), ApiError> {
        loop {
            self.advance(turn).await?;
            tokio::select! {
                () = self.service.shutdown.cancelled() => return Ok(()),
                request = requests.recv() => {
                    let Some(request) = request else { return Ok(()); };
                    match request {
                        Request::Close { reply } => { self.closing = Some(reply); return Ok(()); }
                        Request::Prompt { turn, reply } => {
                            let result = if self.queued.len() >= 64 { Err(ApiError::Conflict("session message queue full".into())) }
                            else if turn.text.trim().is_empty() && turn.attachments.is_empty() { Err(ApiError::BadRequest("text required".into())) }
                            else { self.queued.push_back(turn); Ok(()) };
                            respond(reply, result);
                        }
                        Request::Terminal { launch, reply } => {
                            if launch.command.harness.is_none_or(|harness| !self.session.as_str().starts_with(&format!("{harness}:"))) { respond(reply, Err(ApiError::Conflict("surface switch must retain the session harness".into()))); }
                            else if self.pending.is_some() { respond(reply, Err(ApiError::Conflict("surface switch already pending".into()))); }
                            else { self.pending = Some((launch, reply)); }
                        }
                        Request::TurnEnd => self.terminal_busy = false,
                        Request::Stop { reply } => {
                            let result = match self.surface {
                                Surface::Chat => self.harness.cancel(&self.session).await.map_err(ApiError::from),
                                Surface::Terminal => {
                                    let result = self.service.remove_terminal(&self.user, &self.key).await;
                                    if result.is_ok() { self.terminal_busy = false; self.surface = Surface::Idle; }
                                    result
                                }
                                Surface::Idle => Ok(()),
                            };
                            respond(reply, result);
                        }
                    }
                }
                event = next_event(&mut self.events), if self.events.is_some() => {
                    if let Some(event) = event {
                        if matches!(event, SessionEvent::TurnEnded) { self.turn_ended = true; }
                        self.publish(event)?;
                    }
                    else { return Err(ApiError::Unavailable("harness event stream closed".into())); }
                }
                completed = turn.join_next(), if !turn.is_empty() => {
                    completed.ok_or_else(closed)?.map_err(ApiError::Task)??;
                }
            }
        }
    }

    async fn advance(
        &mut self,
        turn: &mut tokio::task::JoinSet<Result<(), ApiError>>,
    ) -> Result<(), ApiError> {
        if !turn.is_empty() || !self.turn_ended {
            return Ok(());
        }
        if let Some((mut launch, reply)) = self.pending.take() {
            if self.surface == Surface::Chat {
                self.harness.release(&self.session).await?;
                self.events = None;
                self.surface = Surface::Idle;
            }
            let native = self
                .session
                .as_str()
                .split_once(':')
                .ok_or_else(|| ApiError::BadRequest("canonical session required".into()))?
                .1;
            launch.resume = Some(native.into());
            let result = self.service.spawn_raw(&self.user, &self.key, launch).await;
            if result.is_ok() {
                self.surface = Surface::Terminal;
                self.terminal_busy = true;
            }
            respond(reply, result);
            return Ok(());
        }
        if self.queued.is_empty() || (self.surface == Surface::Terminal && self.terminal_busy) {
            return Ok(());
        }
        if self.surface != Surface::Chat {
            if self.surface == Surface::Terminal {
                self.service.remove_terminal(&self.user, &self.key).await?;
            }
            self.surface = Surface::Idle;
            self.harness.resume(&self.session).await?;
            self.surface = Surface::Chat;
            self.events = Some(self.harness.events(&self.session).await?);
        }
        if let Some(queued) = self.queued.pop_front() {
            self.turn_ended = false;
            self.service
                .set_live_status(&self.key, &self.session, HarnessSessionStatus::Active);
            self.publish(SessionEvent::UserMessageChunk {
                text: queued.text.clone(),
            })?;
            let input = queued.input();
            let harness = self.harness.clone();
            let session = self.session.clone();
            turn.spawn(async move {
                harness
                    .prompt(&session, input)
                    .await
                    .map_err(ApiError::from)
            });
        }
        Ok(())
    }

    fn fan_out(&mut self, event: &SessionEvent) {
        if let Some(harness_event) = self.tools.harness_event(&self.session, event) {
            self.service.publish_harness(&self.key, harness_event);
        }
        if matches!(event, SessionEvent::Permission(_)) {
            self.service.set_blocked(&self.key, &self.session, true);
            return;
        }
        let status = if matches!(event, SessionEvent::TurnEnded) {
            HarnessSessionStatus::Idle
        } else if matches!(event, SessionEvent::Exited { .. }) {
            HarnessSessionStatus::Ended
        } else {
            return;
        };
        self.service
            .set_live_status(&self.key, &self.session, status);
    }

    fn publish(&mut self, event: SessionEvent) -> Result<(), ApiError> {
        self.fan_out(&event);
        let ts = Some(chrono::Utc::now());
        let kind = match event {
            SessionEvent::UserMessageChunk { text } => AgentEventKind::MessageUser { text, ts },
            SessionEvent::AgentMessageChunk { text } => AgentEventKind::MessageAgent {
                text,
                model: None,
                duration_ms: None,
                usage: None,
                ts,
            },
            SessionEvent::AgentThoughtChunk { text } => AgentEventKind::ThinkingDelta { text, ts },
            SessionEvent::ToolCall(call) => AgentEventKind::ToolStart {
                tool: call.name,
                args: Some(call.input),
                ts,
            },
            SessionEvent::Plan(plan) => AgentEventKind::TaskPlan {
                tasks: plan.into_iter().map(|entry| entry.content).collect(),
                ts,
            },
            SessionEvent::TurnEnded => AgentEventKind::TurnEnd { ts },
            SessionEvent::Exited { .. } => AgentEventKind::SessionEnd { ts },
            SessionEvent::Permission(request) => AgentEventKind::ToolStart {
                tool: request.name,
                args: Some(request.input),
                ts,
            },
            SessionEvent::ToolCallUpdate(_)
            | SessionEvent::Usage { .. }
            | SessionEvent::AvailableCommands(_)
            | SessionEvent::CurrentMode { .. }
            | SessionEvent::TerminalBytes { .. }
            | SessionEvent::TerminalResize { .. } => {
                tracing::trace!("harness metadata has no v1 session projection");
                return Ok(());
            }
        };
        self.service.registry.ingest(
            &self.user,
            vec![AgentEvent {
                v: ProtocolVersion::V1,
                session: self.key.clone(),
                kind,
                extra: wire::Extra::default(),
            }],
        )
    }
}

async fn next_event(events: &mut Option<mpsc::Receiver<SessionEvent>>) -> Option<SessionEvent> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

fn respond<T>(reply: oneshot::Sender<Result<T, ApiError>>, result: Result<T, ApiError>) {
    if reply.send(result).is_err() {
        tracing::debug!("session request caller disconnected");
    }
}
