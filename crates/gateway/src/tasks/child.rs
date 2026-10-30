use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use kernel::queue::ClaimedJob;
use kernel::tasks::Finish;
use kernel::tasks::Outcome;
use kernel::tasks::Task;
use kernel::tasks::TaskError;
use kernel::tasks::TaskJobPayload;
use kernel::tasks::TaskStatus;
use kernel::tasks::Usage;
use kernel::tasks::Verdict;
use kernel::tasks::first_turn;
use kernel::topics::TopicKey;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::harness::{HarnessEvent, HarnessSessionStatus};
use wire::session_id::SessionId;
use wire::stream::{LlmUsage, ThinkingLevel};

use super::Tasks;
use super::paths;
use crate::Gateway;
use crate::error::ApiError;
use crate::routes::agents::Failure;
use crate::sessions::control::ChatHandle;
use crate::tenancy::UserId;
use crate::term::{AgentLaunch, BYPASS_PERMISSIONS, ChatStart, Lineage, TerminalService};

const TOPIC_DEPTH: usize = 8;

pub(super) async fn run(
    gateway: Arc<Gateway>,
    tasks: Arc<Tasks>,
    claimed: ClaimedJob,
    stop: CancellationToken,
) {
    let ran = match payload(&claimed) {
        Ok(id) => execute(&gateway, &tasks, id, &stop).await,
        Err(error) => Err(error),
    };
    let settled = match ran {
        Ok(()) => tasks.jobs.complete(claimed.id).await,
        Err(error) => tasks.jobs.fail(claimed.id, &error.to_string()).await,
    };
    if let Err(error) = settled {
        tracing::warn!(%error, "task job was not settled");
    }
}

fn payload(claimed: &ClaimedJob) -> Result<Uuid, TaskError> {
    let payload: TaskJobPayload =
        serde_json::from_value(serde_json::Value::Object(claimed.payload.as_map().clone()))
            .map_err(|source| TaskError::Payload(Box::new(source)))?;
    Uuid::parse_str(&payload.task_id).map_err(|_| TaskError::Stored("task id".to_owned()))
}

async fn execute(
    gateway: &Arc<Gateway>,
    tasks: &Arc<Tasks>,
    id: Uuid,
    stop: &CancellationToken,
) -> Result<(), TaskError> {
    let task = tasks.store.load(id).await?;
    if task.status.terminal() || !tasks.store.begin(id, &tasks.node, Utc::now()).await? {
        return Ok(());
    }
    tasks.track(id, stop);
    let started = Utc::now();
    let finish = match drive(gateway, tasks, &task, stop, started).await {
        Ok(finish) => finish,
        Err(failure) => Some(Finish::failed(failure.to_string(), Utc::now())),
    };
    tasks.untrack(id);
    if let Some(finish) = finish {
        tasks.store.finish(id, &finish).await?;
    }
    Ok(())
}

#[derive(Debug, Default)]
struct Progress {
    text: String,
    steps: u32,
}

struct Run<'a> {
    terminal: &'a Arc<TerminalService>,
    tasks: &'a Tasks,
    task: &'a Task,
    user: UserId,
    session: SessionId,
    chat: ChatHandle,
    bypass: bool,
    started: DateTime<Utc>,
}

async fn drive(
    gateway: &Gateway,
    tasks: &Tasks,
    task: &Task,
    stop: &CancellationToken,
    started: DateTime<Utc>,
) -> Result<Option<Finish>, Failure> {
    let terminal = gateway.terminal().ok_or_else(|| {
        ApiError::Unavailable(format!("node {} runs no harness sessions", tasks.node))
    })?;
    let user = UserId::new(task.owner.clone())?;
    let (start, own_mode) = launch(gateway, tasks, terminal, task).await?;
    let summary = terminal.start_chat(&user, start).await?;
    let session = summary.session_id.clone();
    tasks
        .store
        .attach(
            task.id,
            session.as_str(),
            super::harness_in(summary.harness_id),
        )
        .await
        .map_err(|error| ApiError::Unavailable(error.to_string()))?;
    let key = terminal.registry.authorize(&user, session.as_str())?;
    let mut events = terminal.subscribe_harness(&key);
    let mode = task.target.permission_mode.clone().or(own_mode);
    if let Some(mode) = mode.clone() {
        terminal.choose_mode(&session, mode).await?;
    }
    let chat = terminal.chat(&user, session.as_str())?;
    chat.prompt(first_turn(task), None, Vec::new(), Vec::new())
        .await?;
    let run = Run {
        terminal,
        tasks,
        task,
        user,
        session,
        chat,
        bypass: mode.as_deref() == Some(BYPASS_PERMISSIONS),
        started,
    };
    let finish = run.follow(&mut events, stop).await;
    drop(events);
    terminal.release_harness(&key);
    Ok(finish)
}

impl Run<'_> {
    async fn follow(
        &self,
        events: &mut broadcast::Receiver<HarnessEvent>,
        stop: &CancellationToken,
    ) -> Option<Finish> {
        let deadline = self
            .task
            .budget
            .wall_clock
            .map(|wall| tokio::time::Instant::now() + wall);
        let mut progress = Progress::default();
        loop {
            let event = tokio::select! {
                () = stop.cancelled() => return self.stopped().await,
                () = until(deadline) => {
                    return self.halt(TaskStatus::Timeout, Verdict::Timeout, progress).await;
                }
                event = events.recv() => event,
            };
            match event {
                Ok(event) => {
                    if let Some(finish) = self.observe(&mut progress, event).await {
                        return Some(finish);
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(task = %self.task.id, missed, "task child events lagged");
                }
                Err(RecvError::Closed) => {
                    return Some(Finish::failed(
                        "the child's event stream closed",
                        Utc::now(),
                    ));
                }
            }
        }
    }

    async fn observe(&self, progress: &mut Progress, event: HarnessEvent) -> Option<Finish> {
        match event {
            HarnessEvent::AssistantDelta { text, .. } => progress.text.push_str(&text),
            HarnessEvent::ToolUse { .. } => {
                progress.steps = progress.steps.saturating_add(1);
                if self.task.budget.over_turns(progress.steps) {
                    let spent = std::mem::take(progress);
                    return self
                        .halt(TaskStatus::Killed, Verdict::BudgetExceeded, spent)
                        .await;
                }
            }
            HarnessEvent::ApprovalRequest { request_id, .. } => self.approval(&request_id).await,
            HarnessEvent::ApprovalResolved { .. } => self.flip(TaskStatus::Running).await,
            HarnessEvent::TurnComplete { usage, .. } => {
                let done = std::mem::take(progress);
                return Some(self.ended(
                    TaskStatus::Completed,
                    Verdict::Completed,
                    done,
                    usage.as_ref(),
                ));
            }
            HarnessEvent::SessionUpdated {
                status: HarnessSessionStatus::Ended | HarnessSessionStatus::Error,
                ..
            } => {
                return Some(Finish::failed(
                    "the child session ended before its turn completed",
                    Utc::now(),
                ));
            }
            HarnessEvent::Error { message, .. } => {
                return Some(Finish::failed(message, Utc::now()));
            }
            HarnessEvent::ReasoningDelta { .. }
            | HarnessEvent::ToolUpdate { .. }
            | HarnessEvent::ToolResult { .. }
            | HarnessEvent::SessionCreated { .. }
            | HarnessEvent::Plan { .. }
            | HarnessEvent::SessionUpdated { .. }
            | HarnessEvent::Context { .. }
            | HarnessEvent::Transcript { .. }
            | HarnessEvent::Status(_)
            | HarnessEvent::Driver { .. }
            | HarnessEvent::Prompt { .. } => {}
        }
        None
    }

    async fn approval(&self, request: &str) {
        if !self.bypass {
            self.flip(TaskStatus::AwaitingInput).await;
            return;
        }
        if let Err(error) = self
            .terminal
            .allow(&self.user, &self.session, request)
            .await
        {
            tracing::warn!(%error, task = %self.task.id, "the child's approval was not answered");
        }
    }

    async fn flip(&self, status: TaskStatus) {
        if let Err(error) = self.tasks.store.flip(self.task.id, status).await {
            tracing::warn!(%error, task = %self.task.id, "task status did not change");
        }
    }

    async fn stopped(&self) -> Option<Finish> {
        self.interrupt().await;
        match self.tasks.store.load(self.task.id).await {
            Ok(task) if task.status == TaskStatus::Killed => None,
            Ok(_) => Some(Finish::failed("its gateway stopped", Utc::now())),
            Err(error) => Some(Finish::failed(
                format!("its gateway stopped, and its status could not be read: {error}"),
                Utc::now(),
            )),
        }
    }

    async fn halt(
        &self,
        status: TaskStatus,
        verdict: Verdict,
        progress: Progress,
    ) -> Option<Finish> {
        self.interrupt().await;
        Some(self.ended(status, verdict, progress, None))
    }

    async fn interrupt(&self) {
        if let Err(error) = self.chat.stop().await {
            tracing::warn!(%error, task = %self.task.id, "the child's turn was not stopped");
        }
    }

    fn ended(
        &self,
        status: TaskStatus,
        verdict: Verdict,
        progress: Progress,
        usage: Option<&LlmUsage>,
    ) -> Finish {
        let Progress {
            text: summary,
            steps,
        } = progress;
        let at = Utc::now();
        let wall = at.signed_duration_since(self.started).num_milliseconds();
        Finish {
            status,
            result: Some(Outcome { verdict, summary }),
            error: (verdict == Verdict::BudgetExceeded)
                .then(|| format!("maxTurns ({steps}) exceeded")),
            usage: Some(Usage {
                input_tokens: usage.map(|usage| usage.prompt_tokens),
                output_tokens: usage.map(|usage| usage.completion_tokens),
                turns: steps,
                wall_clock_ms: wall.max(0).unsigned_abs(),
            }),
            at,
        }
    }
}

async fn until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn launch(
    gateway: &Gateway,
    tasks: &Tasks,
    terminal: &TerminalService,
    task: &Task,
) -> Result<(ChatStart, Option<String>), Failure> {
    let requested = match &task.target.cwd {
        Some(cwd) => Some(paths::requested(tasks.files_root.as_deref(), cwd).await?),
        None => None,
    };
    let Runtime {
        mut agent,
        model,
        mode: own_mode,
    } = if let Some(preset) = &task.target.preset {
        Runtime {
            agent: crate::routes::agents::launch_preset(gateway, preset.clone()).await?,
            model: None,
            mode: None,
        }
    } else {
        runtime_agent(tasks, terminal, task, requested.as_ref()).await?
    };
    if let Some(directory) = requested {
        agent.directory = directory;
    }
    let harness = agent
        .harness
        .ok_or_else(|| ApiError::Conflict(format!("agent \"{}\" has no harness", agent.name)))?;
    let (topic, startup_context) = inherited_topic(gateway, tasks, task).await;
    let start = ChatStart {
        harness,
        model: task.target.model.clone().or(model),
        effort: effort(task.target.effort.as_deref()),
        cwd: None,
        inherited_cwd: None,
        agent: Some(agent),
        startup_context,
        topic,
        lineage: Some(Lineage {
            parent: canonical(task.parent.session.as_deref()),
            task: task.id.to_string(),
        }),
    };
    Ok((start, own_mode))
}

fn effort(stored: Option<&str>) -> Option<ThinkingLevel> {
    match stored.map(str::parse::<ThinkingLevel>) {
        Some(Ok(level)) => Some(level),
        Some(Err(error)) => {
            tracing::warn!(%error, "a task's effort is not a thinking level; the harness keeps its own");
            None
        }
        None => None,
    }
}

fn canonical(session: Option<&str>) -> Option<SessionId> {
    match session.map(str::parse::<SessionId>) {
        Some(Ok(session)) => Some(session),
        Some(Err(_)) | None => None,
    }
}

struct Runtime {
    agent: AgentLaunch,
    model: Option<String>,
    mode: Option<String>,
}

async fn runtime_agent(
    tasks: &Tasks,
    terminal: &TerminalService,
    task: &Task,
    requested: Option<&std::path::PathBuf>,
) -> Result<Runtime, Failure> {
    let delegate = terminal
        .delegate(&task.target.agent)
        .cloned()
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "agent \"{}\" has no harness on node {}",
                task.target.agent, tasks.node
            ))
        })?;
    let directory = match requested {
        Some(directory) => directory.clone(),
        None => paths::configured(tasks.files_root.as_deref(), delegate.cwd.as_deref()).await?,
    };
    let agent = AgentLaunch {
        name: task.target.agent.clone(),
        directory,
        harness: Some(delegate.harness),
        model: String::new(),
        effort: String::new(),
    };
    Ok(Runtime {
        agent,
        model: delegate.model,
        mode: delegate.permission_mode,
    })
}

async fn inherited_topic(
    gateway: &Gateway,
    tasks: &Tasks,
    task: &Task,
) -> (Option<String>, Option<String>) {
    let topics = match gateway.topics() {
        Ok(topics) => topics,
        Err(ApiError::Unavailable(_)) => return (None, None),
        Err(error) => {
            tracing::warn!(%error, task = %task.id, "the parent's topic was not read");
            return (None, None);
        }
    };
    let mut session = task.parent.session.clone();
    for _ in 0..TOPIC_DEPTH {
        let Some(current) = session.take() else {
            break;
        };
        let Ok(canonical) = current.parse::<SessionId>() else {
            break;
        };
        if let Ok(Some(placement)) = topics.placement(&canonical).await {
            let Ok(Some(topic)) = topics.find(&TopicKey::Id(placement.topic)).await else {
                break;
            };
            let context = crate::routes::topics::prepare(gateway, &topic, Some(&task.goal)).await;
            let text = context
                .as_ref()
                .and_then(|context| kernel::topics::render(context, topic.name.as_str()));
            return (Some(topic.id.to_string()), text);
        }
        session = match tasks.store.by_child(&current).await {
            Ok(Some(above)) => above.parent.session,
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, task = %task.id, "the parent's own parent was not read");
                None
            }
        };
    }
    (None, None)
}
