use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::response::Response;
use kernel::tasks::ApprovalError;
use kernel::tasks::TaskListFilter;
use serde::Deserialize;
use uuid::Uuid;
use wire::task::TaskApprovalRequest;
use wire::task::TaskApprovalWaitResponse;
use wire::task::TaskResponse;
use wire::task::TaskStatus;
use wire::task::TasksListResponse;

use crate::extract::Query;
use crate::extract::parse_enum;

use super::TasksApi;
use super::body::store_error;
use super::error::TaskHttpError;
use super::present::to_wire;
use super::wait::OnTimeout;
use super::wait::WaitHit;
use super::wait::wait_limit;
use super::wait::wait_until;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ListQuery {
    status: Option<String>,
    agent_id: Option<String>,
    limit: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct WaitQuery {
    on_approval: Option<String>,
    timeout_ms: Option<String>,
}

pub(super) async fn list(
    State(api): State<Arc<TasksApi>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<TasksListResponse>, TaskHttpError> {
    let filter = list_filter(&query)?;
    let records = api
        .query
        .list(&filter)
        .await
        .map_err(|error| store_error(&error))?;
    let mut tasks = Vec::with_capacity(records.len());
    for record in &records {
        let pending = approvals_of(&api, record.id).await?;
        tasks.push(to_wire(record, &pending)?);
    }
    Ok(Json(TasksListResponse { tasks }))
}

pub(super) async fn get_one(
    State(api): State<Arc<TasksApi>>,
    Path(id): Path<String>,
) -> Result<Json<TaskResponse>, TaskHttpError> {
    let id = task_id(&id)?;
    let record = api
        .store
        .load(id)
        .await
        .map_err(|error| store_error(&error))?;
    let pending = approvals_of(&api, id).await?;
    Ok(Json(TaskResponse {
        task: to_wire(&record, &pending)?,
    }))
}

pub(super) async fn wait(
    State(api): State<Arc<TasksApi>>,
    Path(id): Path<String>,
    Query(query): Query<WaitQuery>,
) -> Result<Response, TaskHttpError> {
    let id = task_id(&id)?;
    let on_approval = on_approval(query.on_approval.as_deref())?;
    let timeout = wait_limit(query.timeout_ms.as_deref())?;
    match wait_until(&api, id, timeout, on_approval, OnTimeout::Leave).await? {
        WaitHit::Task(task) => Ok(Json(TaskResponse { task }).into_response()),
        WaitHit::Approval(approval) => {
            Ok(Json(TaskApprovalWaitResponse { approval }).into_response())
        }
        WaitHit::Deadline(_) => Err(TaskHttpError::wait_left()),
    }
}

pub(super) fn task_id(raw: &str) -> Result<Uuid, TaskHttpError> {
    match Uuid::parse_str(raw) {
        Ok(id) => Ok(id),
        Err(_) => Err(TaskHttpError::BadRequest(
            "Invalid task ID. Check the task link.".to_owned(),
        )),
    }
}

fn list_filter(query: &ListQuery) -> Result<TaskListFilter, TaskHttpError> {
    Ok(TaskListFilter {
        status: status_of(query.status.as_deref())?,
        agent_id: query.agent_id.clone(),
        limit: limit_of(query.limit.as_deref())?,
    })
}

fn status_of(raw: Option<&str>) -> Result<Option<TaskStatus>, TaskHttpError> {
    match raw {
        None => Ok(None),
        Some(text) => match parse_enum::<TaskStatus>(text) {
            Some(status) => Ok(Some(status)),
            None => Err(TaskHttpError::BadRequest(format!(
                "unknown status \"{text}\""
            ))),
        },
    }
}

fn limit_of(raw: Option<&str>) -> Result<Option<u32>, TaskHttpError> {
    match raw {
        None => Ok(None),
        Some(text) => match text.parse::<u32>() {
            Ok(limit) => Ok(Some(limit)),
            Err(_) => Err(TaskHttpError::BadRequest("invalid limit".to_owned())),
        },
    }
}

fn on_approval(raw: Option<&str>) -> Result<bool, TaskHttpError> {
    match raw {
        None => Ok(false),
        Some("return") => Ok(true),
        Some(_) => Err(TaskHttpError::BadRequest(
            "onApproval must be \"return\"".to_owned(),
        )),
    }
}

async fn approvals_of(api: &TasksApi, id: Uuid) -> Result<Vec<TaskApprovalRequest>, TaskHttpError> {
    let Some(broker) = &api.approvals else {
        return Ok(Vec::new());
    };
    broker
        .pending(id)
        .await
        .map_err(|error| approval_failed(&error))
}

fn approval_failed(error: &ApprovalError) -> TaskHttpError {
    let message = error.to_string();
    match error {
        ApprovalError::Unknown => TaskHttpError::Failed(message),
    }
}
