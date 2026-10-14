use chrono::DateTime;
use chrono::Utc;
use kernel::queue::JobKeyMode;
use uuid::Uuid;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::TaskBudget;
use wire::task::TaskCreateRequest;
use wire::task::TaskExecutorKind;
use wire::task::TaskPromptMode;
use wire::task::TaskSpec;
use wire::task::TaskStatus;

use crate::auth::authorize;
use crate::directory::Prepared;
use crate::directory::TargetInput;
use crate::engine::Engine;
use crate::error::Error;
use crate::jobs::NewJob;
use crate::jobs::TaskJobs;
use crate::origin::TaskOrigin;
use crate::payload::TaskJobPayload;
use crate::record::Finish;
use crate::record::TaskRecord;
use crate::store::TaskStore;

#[derive(Debug)]
pub struct NewTask {
    pub request: TaskCreateRequest,
    pub origin: TaskOrigin,
}

#[derive(Debug)]
pub struct HeartbeatSpec {
    pub agent: String,
    pub index: u32,
    pub goal: String,
}

impl<S, J> Engine<S, J>
where
    S: TaskStore,
    J: TaskJobs,
{
    pub async fn submit(&self, task: NewTask) -> Result<Uuid, Error> {
        let id = Uuid::now_v7();
        let (record, prepared) = self
            .record(id, &task.request, task.origin, Utc::now())
            .await?;
        self.store.insert(&record).await?;
        let job = new_job(&record, &prepared);
        if let Err(error) = self.jobs.enqueue(&job).await {
            return Err(self.mark_enqueue_failed(id, error).await);
        }
        self.notify_jobs();
        Ok(id)
    }

    pub async fn submit_with_token(
        &self,
        presented: Option<&str>,
        task: NewTask,
    ) -> Result<Uuid, Error> {
        authorize(self.settings.api_token.as_deref(), presented)?;
        self.submit(task).await
    }

    pub async fn steer(&self, id: Uuid, message: &str) -> Result<(), Error> {
        self.store.set_pending(id, message).await?;
        let task = self.store.load(id).await?;
        let prepared = self.directory.prepare(&TargetInput {
            executor: task.executor,
            executor_target: task.executor_target.clone(),
            node_affinity: task.node_affinity.clone(),
            agent_id: task.agent_id.clone(),
        })?;
        self.jobs.enqueue(&new_job(&task, &prepared)).await?;
        self.notify_jobs();
        Ok(())
    }

    pub async fn kill(&self, id: Uuid) -> Result<TaskStatus, Error> {
        let status = self.store.kill(id, Utc::now()).await?;
        self.cancel_run(id);
        Ok(status)
    }

    pub async fn load(&self, id: Uuid) -> Result<TaskRecord, Error> {
        self.store.load(id).await
    }

    pub async fn schedule_heartbeat(&self, spec: HeartbeatSpec) -> Result<Uuid, Error> {
        if spec.agent.is_empty() {
            return Err(Error::HeartbeatAgent);
        }
        let now = Utc::now();
        let session_key = format!("heartbeat:{}:{}", spec.agent, spec.index);
        self.store.kill_queued_by_session(&session_key, now).await?;
        let request = heartbeat_request(&spec, &session_key);
        let (record, prepared) = self
            .record(Uuid::now_v7(), &request, TaskOrigin::Heartbeat, now)
            .await?;
        self.store.insert(&record).await?;
        if let Err(error) = self.jobs.enqueue(&new_job(&record, &prepared)).await {
            return Err(self.mark_enqueue_failed(record.id, error).await);
        }
        self.notify_jobs();
        Ok(record.id)
    }

    async fn record(
        &self,
        id: Uuid,
        request: &TaskCreateRequest,
        origin: TaskOrigin,
        now: DateTime<Utc>,
    ) -> Result<(TaskRecord, Prepared), Error> {
        let executor = checked(request)?;
        let (parent, depth) = chain(&self.store, request).await?;
        let budget = budget_of(request);
        let spec = spec_for(id, request, budget, TaskPromptMode::Task)?;
        let input = TargetInput {
            executor,
            executor_target: request.executor_target.clone(),
            node_affinity: request.node_affinity.clone(),
            agent_id: request.agent_id.clone(),
        };
        let prepared = self.directory.prepare(&input)?;
        let record = TaskRecord {
            id,
            goal: request.goal.clone(),
            context_refs: spec.resolved_context.clone(),
            acceptance_criteria: spec.acceptance_criteria.clone(),
            spec,
            executor,
            executor_target: request.executor_target.clone(),
            requested_by: request.requested_by.clone(),
            agent_id: request.agent_id.clone(),
            origin,
            parent_task_id: parent,
            chain_depth: depth,
            budget,
            node_affinity: request.node_affinity.clone(),
            usage: None,
            status: TaskStatus::Queued,
            attempt: 0,
            max_attempts: 1,
            result: None,
            error: None,
            conversation_id: None,
            session_key: None,
            pending_message: None,
            claimed_by: None,
            harness_session_ids: Vec::new(),
            created_at: now,
            started_at: None,
            last_heartbeat_at: None,
            completed_at: None,
            duration_ms: None,
            eval: None,
            eval_attempt: 0,
        };
        let session_key = record.spec.session.clone();
        let mut record = record;
        record.session_key = Some(session_key);
        Ok((record, prepared))
    }

    async fn mark_enqueue_failed(&self, id: Uuid, error: Error) -> Error {
        let finish = Finish {
            status: TaskStatus::Failed,
            result: None,
            error: Some(error.to_string()),
            usage: None,
            completed_at: Utc::now(),
            duration_ms: None,
        };
        match self.store.finish(id, &finish).await {
            Ok(_wrote) => Error::Enqueue {
                id,
                source: Box::new(error),
            },
            Err(cleanup) => Error::Cleanup {
                context: "enqueue",
                primary: error.to_string(),
                cleanup: cleanup.to_string(),
            },
        }
    }
}

fn checked(request: &TaskCreateRequest) -> Result<TaskExecutorKind, Error> {
    if request.goal.is_empty() {
        return Err(Error::GoalRequired);
    }
    if request.agent_id.is_empty() {
        return Err(Error::AgentRequired);
    }
    request.executor.ok_or(Error::ExecutorRequired)
}

fn budget_of(request: &TaskCreateRequest) -> TaskBudget {
    if let Some(budget) = request.budget {
        return budget;
    }
    match request.spec.as_ref() {
        Some(spec) => spec.budget,
        None => TaskBudget::default(),
    }
}

fn spec_for(
    id: Uuid,
    request: &TaskCreateRequest,
    budget: TaskBudget,
    prompt_mode: TaskPromptMode,
) -> Result<TaskSpec, Error> {
    let context_refs = clone_or_empty(request.context_refs.as_ref());
    let acceptance_criteria = clone_criteria(request.acceptance_criteria.as_ref());
    match request.spec.clone() {
        None => Ok(TaskSpec {
            task_id: id.to_string(),
            agent_id: request.agent_id.clone(),
            goal: request.goal.clone(),
            resolved_context: context_refs,
            acceptance_criteria,
            budget,
            tools: None,
            working_dir: None,
            model: None,
            effort: None,
            system_prompt_append: None,
            isolation: None,
            prompt_mode: Some(prompt_mode),
            exclude_tools: None,
            resume_message: None,
            session: format!("task:{id}"),
            permission_decisions: None,
        }),
        Some(mut spec) => {
            spec.task_id = id.to_string();
            spec.budget = budget;
            if spec.prompt_mode == Some(TaskPromptMode::Heartbeat) {
                if spec.session.is_empty() {
                    return Err(Error::EmptySession);
                }
            } else {
                spec.session = format!("task:{id}");
            }
            Ok(spec)
        }
    }
}

fn clone_or_empty(refs: Option<&Vec<ContextRef>>) -> Vec<ContextRef> {
    match refs {
        Some(refs) => refs.clone(),
        None => Vec::new(),
    }
}

fn clone_criteria(items: Option<&Vec<AcceptanceCriterion>>) -> Vec<AcceptanceCriterion> {
    match items {
        Some(items) => items.clone(),
        None => Vec::new(),
    }
}

async fn chain<S: TaskStore>(
    store: &S,
    request: &TaskCreateRequest,
) -> Result<(Option<Uuid>, u32), Error> {
    let Some(parent) = request.parent_task_id.as_deref() else {
        return match request.chain_depth {
            Some(0) | None => Ok((None, 0)),
            Some(got) => Err(Error::ChainDepth { got, parent: 0 }),
        };
    };
    let parent_id = Uuid::parse_str(parent).map_err(|_| Error::ParentId)?;
    let depth = request.chain_depth.ok_or(Error::ChainDepthRequired)?;
    let loaded = match store.load(parent_id).await {
        Ok(task) => task,
        Err(Error::Missing { .. }) => return Err(Error::MissingParent),
        Err(error) => return Err(error),
    };
    let expected = loaded.chain_depth.checked_add(1).ok_or(Error::ChainDepth {
        got: depth,
        parent: loaded.chain_depth,
    })?;
    if depth != expected {
        return Err(Error::ChainDepth {
            got: depth,
            parent: loaded.chain_depth,
        });
    }
    Ok((Some(parent_id), depth))
}

fn new_job(record: &TaskRecord, prepared: &Prepared) -> NewJob {
    NewJob {
        name: prepared.job_name.clone(),
        payload: TaskJobPayload {
            task_id: record.id.to_string(),
        },
        key: format!("task:{}", record.id),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        max_attempts: 1,
    }
}

fn heartbeat_request(spec: &HeartbeatSpec, session_key: &str) -> TaskCreateRequest {
    TaskCreateRequest {
        goal: spec.goal.clone(),
        agent_id: spec.agent.clone(),
        executor: Some(TaskExecutorKind::ChatLoop),
        executor_target: None,
        requested_by: None,
        parent_task_id: None,
        chain_depth: Some(0),
        node_affinity: None,
        spec: Some(TaskSpec {
            task_id: String::new(),
            agent_id: spec.agent.clone(),
            goal: spec.goal.clone(),
            resolved_context: Vec::new(),
            acceptance_criteria: Vec::new(),
            budget: TaskBudget::default(),
            tools: None,
            working_dir: None,
            model: None,
            effort: None,
            system_prompt_append: None,
            isolation: None,
            prompt_mode: Some(TaskPromptMode::Heartbeat),
            exclude_tools: None,
            resume_message: None,
            session: session_key.to_string(),
            permission_decisions: None,
        }),
        budget: None,
        context_refs: None,
        acceptance_criteria: None,
    }
}
