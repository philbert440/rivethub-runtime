use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use kernel::queue::JobId;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::sessions::SessionMessage;
use wire::gateway::sessions::SessionPostReply;
use wire::gateway::sessions::SessionPostRequest;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskResult;
use wire::task::TaskStatus;
use wire::task::TaskUsage;
use wire::task::TaskVerdict;

use kernel::queue::ClaimSelector;
use kernel::queue::ClaimedJob;
use kernel::tasks::Finish;
use kernel::tasks::TaskJobPayload;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use kernel::tasks::terminal;

use crate::client::GatewayClient;
use crate::client::check_segment;
use crate::directory::ResolvedTarget;
use crate::directory::TargetInput;
use crate::directory::apply_spec;
use crate::engine::Engine;
use crate::error::Error;
use crate::paths::contained;
use crate::price::cost;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Idle,
    Completed(Uuid),
    AlreadyFinished(Uuid),
}

impl<S, J> Engine<S, J>
where
    S: TaskStore,
    J: JobQueue,
{
    pub async fn run_one(&self) -> Result<RunOutcome, Error> {
        self.store.sweep(&self.settings.worker, Utc::now()).await?;
        let Some(claimed) = self.jobs.claim(&self.selector()).await? else {
            return Ok(RunOutcome::Idle);
        };
        self.execute(claimed, &CancellationToken::new()).await
    }

    pub(crate) fn selector(&self) -> ClaimSelector {
        ClaimSelector {
            names: vec![
                JobName::Fixed(kernel::queue::FixedJob::RunTask),
                JobName::RunTaskOn {
                    node: self.settings.node.clone(),
                },
            ],
            only_id: None,
        }
    }

    pub(crate) async fn execute(
        &self,
        claimed: ClaimedJob,
        parent: &CancellationToken,
    ) -> Result<RunOutcome, Error> {
        let parsed = match parse_claimed(&claimed) {
            Ok(parsed) => parsed,
            Err(error) => return Err(self.fail_job(claimed.id, error).await),
        };
        let task = match self.store.load(parsed.task_id).await {
            Ok(task) => task,
            Err(error) => return Err(self.fail_job(claimed.id, error.into()).await),
        };
        if terminal(task.status) {
            self.jobs.complete(claimed.id).await?;
            return Ok(RunOutcome::AlreadyFinished(task.id));
        }
        if task.budget.max_turns == Some(0) {
            return self
                .settle_budget(
                    &task,
                    claimed.id,
                    None,
                    "maxTurns (0) reached".to_string(),
                    Utc::now(),
                )
                .await;
        }
        let now = Utc::now();
        let running = match self
            .store
            .mark_running(task.id, &self.settings.worker, &self.settings.node, now)
            .await
        {
            Ok(running) => running,
            Err(error) => return Err(self.fail_job(claimed.id, error.into()).await),
        };
        let child = self.begin_run(running.id, parent);
        let driven = self.drive(&running, &child).await;
        self.end_run(running.id);
        match driven {
            Ok(reply) => self.settle_reply(&running, claimed.id, reply, now).await,
            Err(Error::Cancelled) => self.settle_cancelled(running.id, claimed.id).await,
            Err(error) => {
                self.settle_call_error(&running, claimed.id, error, now)
                    .await
            }
        }
    }

    async fn drive(
        &self,
        task: &TaskRecord,
        cancel: &CancellationToken,
    ) -> Result<SessionPostReply, Error> {
        let mut target = self.directory.resolve(&target_input(task))?;
        apply_spec(&mut target, &task.spec)?;
        drop(self.working_dir(&target).await?);
        let client = match target.mesh.as_ref() {
            Some(identity) => GatewayClient::with_mesh(target.base.clone(), identity)?,
            None => GatewayClient::new(target.base.clone())?,
        };
        client.healthz_at(cancel).await?;
        check_segment(&task.spec.session)?;
        let body = SessionPostRequest {
            exclude_tools: task.spec.exclude_tools.clone(),
            text: prompt_text(&task.goal, task.pending_message.as_deref()),
            user_id: None,
            agent: Some(task.agent_id.clone()),
            thinking: None,
            system_prompt: target.system_prompt.clone(),
        };
        client
            .post_message(
                &task.spec.session,
                &body,
                request_timeout(&task.budget),
                cancel,
            )
            .await
    }

    async fn working_dir(&self, target: &ResolvedTarget) -> Result<Option<String>, Error> {
        if let Some(working) = target.working_dir.as_deref() {
            let Some(root) = self.settings.files_root.as_deref() else {
                return Err(Error::FilesRootRequired);
            };
            return contained_string(root, std::path::Path::new(working))
                .await
                .map(Some);
        }
        if target.harness.is_some() || target.executor == TaskExecutorKind::HarnessSession {
            return self.files_root_cwd().await;
        }
        Ok(None)
    }

    async fn files_root_cwd(&self) -> Result<Option<String>, Error> {
        let Some(root) = self.settings.files_root.as_deref() else {
            return Err(Error::FilesRootRequired);
        };
        let path = contained(root, root).await?;
        path.into_os_string()
            .into_string()
            .map(Some)
            .map_err(|_| Error::PathEscape)
    }

    async fn settle_reply(
        &self,
        task: &TaskRecord,
        job_id: JobId,
        reply: SessionPostReply,
        started: DateTime<Utc>,
    ) -> Result<RunOutcome, Error> {
        if self.release_if_killed(task.id, job_id).await? {
            return Ok(RunOutcome::AlreadyFinished(task.id));
        }
        let ended = Utc::now();
        let wall = match wall_ms(started, ended) {
            Ok(wall) => wall,
            Err(error) => return self.fail_clock(task.id, job_id, error, ended).await,
        };
        let usage = match usage_of(&reply.message, &self.settings.pricing, wall) {
            Ok(usage) => usage,
            Err(error) => return self.settle_call_error(task, job_id, error, started).await,
        };
        if let Some(reason) = budget_reason(&task.budget, &usage) {
            return self
                .settle_budget(task, job_id, Some(usage), reason, ended)
                .await;
        }
        let result = TaskResult {
            verdict: TaskVerdict::Completed,
            summary: reply.message.text.clone(),
            output: None,
            artifacts: Vec::new(),
            criteria_self_report: None,
            usage,
            error: None,
        };
        self.finish_ok(task.id, job_id, result, ended, wall).await
    }

    async fn settle_budget(
        &self,
        task: &TaskRecord,
        job_id: JobId,
        usage: Option<TaskUsage>,
        reason: String,
        ended: DateTime<Utc>,
    ) -> Result<RunOutcome, Error> {
        let result = TaskResult {
            verdict: TaskVerdict::BudgetExceeded,
            summary: String::new(),
            output: None,
            artifacts: Vec::new(),
            criteria_self_report: None,
            usage: match usage {
                Some(usage) => usage,
                None => empty_usage(0),
            },
            error: Some(reason),
        };
        self.record_failure(RecordedFailure {
            id: task.id,
            job_id,
            status: TaskStatus::Killed,
            result,
            error: Error::Budget,
            ended,
        })
        .await
    }

    async fn settle_call_error(
        &self,
        task: &TaskRecord,
        job_id: JobId,
        error: Error,
        started: DateTime<Utc>,
    ) -> Result<RunOutcome, Error> {
        if self.release_if_killed(task.id, job_id).await? {
            return Ok(RunOutcome::AlreadyFinished(task.id));
        }
        let ended = Utc::now();
        let wall = match wall_ms(started, ended) {
            Ok(wall) => wall,
            Err(clock) => return self.fail_clock(task.id, job_id, clock, ended).await,
        };
        let status = if matches!(error, Error::Timeout) {
            TaskStatus::Timeout
        } else {
            TaskStatus::Failed
        };
        let verdict = if status == TaskStatus::Timeout {
            TaskVerdict::Timeout
        } else {
            TaskVerdict::Failed
        };
        let result = TaskResult {
            verdict,
            summary: String::new(),
            output: None,
            artifacts: Vec::new(),
            criteria_self_report: None,
            usage: empty_usage(wall),
            error: Some(error.to_string()),
        };
        self.record_failure(RecordedFailure {
            id: task.id,
            job_id,
            status,
            result,
            error,
            ended,
        })
        .await
    }

    async fn settle_cancelled(&self, id: Uuid, job_id: JobId) -> Result<RunOutcome, Error> {
        if self.release_if_killed(id, job_id).await? {
            return Ok(RunOutcome::AlreadyFinished(id));
        }
        Err(self.fail_job(job_id, Error::Cancelled).await)
    }

    async fn fail_clock(
        &self,
        id: Uuid,
        job_id: JobId,
        error: Error,
        ended: DateTime<Utc>,
    ) -> Result<RunOutcome, Error> {
        let result = TaskResult {
            verdict: TaskVerdict::Failed,
            summary: String::new(),
            output: None,
            artifacts: Vec::new(),
            criteria_self_report: None,
            usage: empty_usage(0),
            error: Some(error.to_string()),
        };
        self.record_failure(RecordedFailure {
            id,
            job_id,
            status: TaskStatus::Failed,
            result,
            error,
            ended,
        })
        .await
    }

    async fn release_if_killed(&self, id: Uuid, job_id: JobId) -> Result<bool, Error> {
        let current = self.store.load(id).await?;
        if current.status != TaskStatus::Killed {
            return Ok(false);
        }
        self.jobs.complete(job_id).await?;
        Ok(true)
    }

    async fn finish_ok(
        &self,
        id: Uuid,
        job_id: JobId,
        result: TaskResult,
        ended: DateTime<Utc>,
        wall: u64,
    ) -> Result<RunOutcome, Error> {
        let duration = match duration_i32(wall) {
            Ok(duration) => duration,
            Err(error) => return self.fail_clock(id, job_id, error, ended).await,
        };
        let finish = with_usage(Finish {
            status: TaskStatus::Completed,
            result: Some(result),
            error: None,
            usage: None,
            completed_at: ended,
            duration_ms: Some(duration),
        });
        if !self.store.finish(id, &finish).await? {
            self.jobs.complete(job_id).await?;
            return Ok(RunOutcome::AlreadyFinished(id));
        }
        self.publish(id);
        self.jobs.complete(job_id).await?;
        Ok(RunOutcome::Completed(id))
    }

    async fn record_failure(&self, failure: RecordedFailure) -> Result<RunOutcome, Error> {
        let RecordedFailure {
            id,
            job_id,
            status,
            result,
            error,
            ended,
        } = failure;
        let finish = Finish {
            status,
            usage: Some(result.usage),
            error: result.error.clone(),
            result: Some(result),
            completed_at: ended,
            duration_ms: None,
        };
        let wrote = match self.store.finish(id, &finish).await {
            Ok(wrote) => wrote,
            Err(cleanup) => {
                return Err(Error::Cleanup {
                    context: "finish task",
                    primary: error.to_string(),
                    cleanup: cleanup.to_string(),
                });
            }
        };
        if !wrote {
            self.jobs.complete(job_id).await?;
            return Ok(RunOutcome::AlreadyFinished(id));
        }
        self.publish(id);
        Err(self.fail_job(job_id, error).await)
    }

    async fn fail_job(&self, id: JobId, error: Error) -> Error {
        match self.jobs.fail(id, &error.to_string()).await {
            Ok(()) => error,
            Err(cleanup) => Error::Cleanup {
                context: "fail job",
                primary: error.to_string(),
                cleanup: cleanup.to_string(),
            },
        }
    }
}

struct RecordedFailure {
    id: Uuid,
    job_id: JobId,
    status: TaskStatus,
    result: TaskResult,
    error: Error,
    ended: DateTime<Utc>,
}

struct ParsedJob {
    task_id: Uuid,
}

fn parse_claimed(claimed: &ClaimedJob) -> Result<ParsedJob, Error> {
    let payload: TaskJobPayload =
        serde_json::from_value(serde_json::Value::Object(claimed.payload.as_map().clone()))
            .map_err(|source| Error::Payload(Box::new(source)))?;
    let task_id = Uuid::parse_str(&payload.task_id)
        .map_err(|_| Error::Stored("task id is not a uuid".to_string()))?;
    Ok(ParsedJob { task_id })
}

fn target_input(task: &TaskRecord) -> TargetInput {
    TargetInput {
        executor: task.executor,
        executor_target: task.executor_target.clone(),
        node_affinity: task.node_affinity.clone(),
        agent_id: task.agent_id.clone(),
    }
}

fn request_timeout(budget: &TaskBudget) -> Option<Duration> {
    budget.max_wall_clock_ms.map(Duration::from_millis)
}

fn prompt_text(goal: &str, pending: Option<&str>) -> String {
    match pending {
        Some(message) => message.to_string(),
        None => goal.to_string(),
    }
}

fn usage_of(
    message: &SessionMessage,
    rates: &std::collections::BTreeMap<String, crate::price::TokenRate>,
    wall: u64,
) -> Result<TaskUsage, Error> {
    let (input, output) = match &message.usage {
        Some(usage) => (usage.prompt_tokens, usage.completion_tokens),
        None if message.model.is_some() => return Err(Error::Pricing),
        None => (0, 0),
    };
    let total = input.checked_add(output).ok_or(Error::Pricing)?;
    Ok(TaskUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: total,
        cost_usd: cost(rates, message.model.as_deref(), input, output)?,
        turns: 1,
        wall_clock_ms: wall,
    })
}

fn budget_reason(budget: &TaskBudget, usage: &TaskUsage) -> Option<String> {
    if let Some(max) = budget.max_turns
        && usage.turns >= max
    {
        return Some(format!("maxTurns ({max}) reached"));
    }
    if let Some(max) = budget.max_tokens
        && usage.total_tokens >= max
    {
        return Some(format!("maxTokens ({max}) reached"));
    }
    if let Some(max) = budget.max_usd {
        let over = match usage.cost_usd {
            Some(cost) => cost >= max,
            None => true,
        };
        if over {
            return Some(format!("maxUsd ({max}) reached"));
        }
    }
    if let Some(max) = budget.max_wall_clock_ms
        && usage.wall_clock_ms >= max
    {
        return Some(format!("maxWallClockMs ({max}) reached"));
    }
    None
}

fn empty_usage(wall: u64) -> TaskUsage {
    TaskUsage {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cost_usd: None,
        turns: 0,
        wall_clock_ms: wall,
    }
}

async fn contained_string(
    root: &std::path::Path,
    working: &std::path::Path,
) -> Result<String, Error> {
    let path = contained(root, working).await?;
    path.into_os_string()
        .into_string()
        .map_err(|_| Error::PathEscape)
}

fn wall_ms(started: DateTime<Utc>, ended: DateTime<Utc>) -> Result<u64, Error> {
    let millis = ended.signed_duration_since(started).num_milliseconds();
    u64::try_from(millis).map_err(|_| Error::Clock)
}

fn duration_i32(millis: u64) -> Result<i32, Error> {
    i32::try_from(millis).map_err(|_| Error::Clock)
}

fn with_usage(mut finish: Finish) -> Finish {
    finish.usage = finish.result.as_ref().map(|result| result.usage);
    finish
}
