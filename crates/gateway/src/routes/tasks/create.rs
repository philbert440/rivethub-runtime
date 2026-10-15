use std::sync::Arc;

use axum::Json;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use chrono::Utc;
use kernel::tasks::Finish;
use kernel::tasks::TaskError;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use serde::Deserialize;
use uuid::Uuid;
use wire::harness::HarnessId;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPromptMode;
use wire::task::TaskResponse;
use wire::task::TaskSpec;
use wire::task::TaskStatus;

use crate::extract::Query;
use crate::extract::parse_enum;

use super::TasksApi;
use super::body::read_json;
use super::body::required;
use super::body::store_error;
use super::error::TaskHttpError;
use super::job::new_job;
use super::present::to_wire;
use super::wait::OnTimeout;
use super::wait::WaitHit;
use super::wait::wait_limit;
use super::wait::wait_until;
use super::wait::wants_wait;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CreateBody {
    goal: Option<String>,
    agent_id: Option<String>,
    executor: Option<String>,
    executor_target: Option<String>,
    requested_by: Option<String>,
    parent_task_id: Option<String>,
    chain_depth: Option<u32>,
    node_affinity: Option<String>,
    spec: Option<TaskSpec>,
    budget: Option<TaskBudget>,
    context_refs: Option<Vec<ContextRef>>,
    acceptance_criteria: Option<Vec<AcceptanceCriterion>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct CreateQuery {
    wait: Option<String>,
    timeout_ms: Option<String>,
}

struct Accepted {
    goal: String,
    agent_id: String,
    executor: TaskExecutorKind,
    requested_by: Option<String>,
}

pub(super) async fn create(
    State(api): State<Arc<TasksApi>>,
    Query(query): Query<CreateQuery>,
    request: Request,
) -> Result<Response, TaskHttpError> {
    let body: CreateBody = read_json(request).await?;
    let timeout = wait_limit(query.timeout_ms.as_deref())?;
    let accepted = accept(&api, &body).await?;
    let id = Uuid::now_v7();
    let record = assemble(&api.store, id, &body, &accepted, Utc::now()).await?;
    persist(&api, &record).await?;
    respond(&api, &query, timeout, record).await
}

async fn accept(api: &TasksApi, body: &CreateBody) -> Result<Accepted, TaskHttpError> {
    let goal = required(body.goal.clone(), "goal")?;
    let agent_id = required(body.agent_id.clone(), "agentId")?;
    let executor = executor_of(body.executor.as_deref())?;
    harness_allowed(api, executor, body.executor_target.as_deref())?;
    ensure_agent(api, &agent_id).await?;
    Ok(Accepted {
        goal,
        agent_id,
        executor,
        requested_by: Some(requester(body.requested_by.clone())),
    })
}

fn executor_of(raw: Option<&str>) -> Result<TaskExecutorKind, TaskHttpError> {
    match raw {
        None => Ok(TaskExecutorKind::ChatLoop),
        Some(text) => match parse_enum::<TaskExecutorKind>(text) {
            Some(executor) => Ok(executor),
            None => Err(unknown_executor(text)),
        },
    }
}

fn unknown_executor(raw: &str) -> TaskHttpError {
    match serde_json::to_string(raw) {
        Ok(quoted) => TaskHttpError::BadRequest(format!("unknown executor {quoted}")),
        Err(error) => TaskHttpError::Failed(error.to_string()),
    }
}

fn harness_allowed(
    api: &TasksApi,
    executor: TaskExecutorKind,
    target: Option<&str>,
) -> Result<(), TaskHttpError> {
    if executor != TaskExecutorKind::HarnessSession {
        return Ok(());
    }
    let Some(target) = target else {
        return Ok(());
    };
    let Some(harness) = parse_enum::<HarnessId>(target) else {
        return Ok(());
    };
    match &api.allowed_harnesses {
        None => Ok(()),
        Some(allowed) if allowed.contains(&harness) => Ok(()),
        Some(_) => Err(TaskHttpError::HarnessDenied(harness)),
    }
}

async fn ensure_agent(api: &TasksApi, agent_id: &str) -> Result<(), TaskHttpError> {
    if api
        .agents
        .contains(agent_id)
        .await
        .map_err(|error| store_error(&error))?
    {
        return Ok(());
    }
    let message = if api.mesh {
        format!("agent \"{agent_id}\" not found locally or on the mesh")
    } else {
        format!("agent \"{agent_id}\" not found locally")
    };
    Err(TaskHttpError::NotFound(message))
}

fn requester(requested_by: Option<String>) -> String {
    match requested_by {
        Some(value) => value,
        None => "gateway".to_owned(),
    }
}

async fn assemble(
    store: &Arc<dyn TaskStore>,
    id: Uuid,
    body: &CreateBody,
    accepted: &Accepted,
    now: chrono::DateTime<Utc>,
) -> Result<TaskRecord, TaskHttpError> {
    let (parent, depth) = chain(store, body).await?;
    let budget = budget_of(body);
    let spec = spec_for(id, body, accepted, budget)?;
    let session_key = spec.session.clone();
    Ok(TaskRecord {
        id,
        goal: accepted.goal.clone(),
        context_refs: spec.resolved_context.clone(),
        acceptance_criteria: spec.acceptance_criteria.clone(),
        spec,
        executor: accepted.executor,
        executor_target: body.executor_target.clone(),
        requested_by: accepted.requested_by.clone(),
        agent_id: accepted.agent_id.clone(),
        origin: TaskOrigin::Api,
        parent_task_id: parent,
        chain_depth: depth,
        budget,
        node_affinity: body.node_affinity.clone(),
        usage: None,
        status: TaskStatus::Queued,
        attempt: 0,
        max_attempts: 1,
        result: None,
        error: None,
        conversation_id: None,
        session_key: Some(session_key),
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
    })
}

fn budget_of(body: &CreateBody) -> TaskBudget {
    if let Some(budget) = body.budget {
        return budget;
    }
    match body.spec.as_ref() {
        Some(spec) => spec.budget,
        None => TaskBudget::default(),
    }
}

fn spec_for(
    id: Uuid,
    body: &CreateBody,
    accepted: &Accepted,
    budget: TaskBudget,
) -> Result<TaskSpec, TaskHttpError> {
    match body.spec.clone() {
        None => Ok(blank_spec(
            id,
            accepted,
            budget,
            refs_of(body),
            criteria_of(body),
        )),
        Some(mut spec) => {
            spec.task_id = id.to_string();
            spec.budget = budget;
            session_of(id, &mut spec)?;
            Ok(spec)
        }
    }
}

fn blank_spec(
    id: Uuid,
    accepted: &Accepted,
    budget: TaskBudget,
    resolved_context: Vec<ContextRef>,
    acceptance_criteria: Vec<AcceptanceCriterion>,
) -> TaskSpec {
    TaskSpec {
        task_id: id.to_string(),
        agent_id: accepted.agent_id.clone(),
        goal: accepted.goal.clone(),
        resolved_context,
        acceptance_criteria,
        budget,
        tools: None,
        working_dir: None,
        model: None,
        effort: None,
        system_prompt_append: None,
        isolation: None,
        prompt_mode: Some(TaskPromptMode::Task),
        exclude_tools: None,
        resume_message: None,
        session: format!("task:{id}"),
        permission_decisions: None,
    }
}

fn refs_of(body: &CreateBody) -> Vec<ContextRef> {
    match &body.context_refs {
        Some(refs) => refs.clone(),
        None => Vec::new(),
    }
}

fn criteria_of(body: &CreateBody) -> Vec<AcceptanceCriterion> {
    match &body.acceptance_criteria {
        Some(items) => items.clone(),
        None => Vec::new(),
    }
}

fn session_of(id: Uuid, spec: &mut TaskSpec) -> Result<(), TaskHttpError> {
    if spec.prompt_mode == Some(TaskPromptMode::Heartbeat) {
        if spec.session.is_empty() {
            return Err(TaskHttpError::BadRequest(
                "spec session is empty".to_owned(),
            ));
        }
        return Ok(());
    }
    spec.session = format!("task:{id}");
    Ok(())
}

async fn chain(
    store: &Arc<dyn TaskStore>,
    body: &CreateBody,
) -> Result<(Option<Uuid>, u32), TaskHttpError> {
    let Some(parent) = body.parent_task_id.as_deref() else {
        return match body.chain_depth {
            Some(0) | None => Ok((None, 0)),
            Some(got) => Err(depth_error(got, 0)),
        };
    };
    let Ok(parent_id) = Uuid::parse_str(parent) else {
        return Err(TaskHttpError::BadRequest(
            "parent task id is not a uuid".to_owned(),
        ));
    };
    let Some(depth) = body.chain_depth else {
        return Err(TaskHttpError::Conflict(
            "chain depth is required when a parent task is set".to_owned(),
        ));
    };
    let loaded = match store.load(parent_id).await {
        Ok(task) => task,
        Err(TaskError::Missing { .. }) => {
            return Err(TaskHttpError::Conflict("parent task is missing".to_owned()));
        }
        Err(error) => return Err(store_error(&error)),
    };
    let Some(expected) = loaded.chain_depth.checked_add(1) else {
        return Err(depth_error(depth, loaded.chain_depth));
    };
    if depth != expected {
        return Err(depth_error(depth, loaded.chain_depth));
    }
    Ok((Some(parent_id), depth))
}

fn depth_error(got: u32, parent: u32) -> TaskHttpError {
    TaskHttpError::Conflict(format!(
        "chain depth {got} does not follow parent depth {parent}"
    ))
}

async fn persist(api: &TasksApi, record: &TaskRecord) -> Result<(), TaskHttpError> {
    api.store
        .insert(record)
        .await
        .map_err(|error| store_error(&error))?;
    if let Err(error) = api.jobs.enqueue(&new_job(record, &api.node)).await {
        return Err(enqueue_cleanup(api, record.id, &error).await);
    }
    Ok(())
}

async fn enqueue_cleanup(api: &TasksApi, id: Uuid, error: &TaskError) -> TaskHttpError {
    let primary = error.to_string();
    let finish = Finish {
        status: TaskStatus::Failed,
        result: None,
        error: Some(primary.clone()),
        usage: None,
        completed_at: Utc::now(),
        duration_ms: None,
    };
    match api.store.finish(id, &finish).await {
        Ok(_) => TaskHttpError::Failed(primary),
        Err(cleanup) => TaskHttpError::Failed(format!(
            "enqueue failed ({primary}); cleanup also failed ({cleanup})"
        )),
    }
}

async fn respond(
    api: &TasksApi,
    query: &CreateQuery,
    timeout: std::time::Duration,
    record: TaskRecord,
) -> Result<Response, TaskHttpError> {
    if !wants_wait(query.wait.as_deref()) {
        let task = to_wire(&record, &[])?;
        return Ok((StatusCode::CREATED, Json(TaskResponse { task })).into_response());
    }
    match wait_until(api, record.id, timeout, false, OnTimeout::Kill).await? {
        WaitHit::Task(task) => Ok((StatusCode::OK, Json(TaskResponse { task })).into_response()),
        WaitHit::Approval(_) => Err(TaskHttpError::Failed(
            "approval was not requested".to_owned(),
        )),
        WaitHit::Deadline(task) => Err(TaskHttpError::wait_killed(task)),
    }
}
