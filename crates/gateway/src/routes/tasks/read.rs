use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::response::Response;
use kernel::tasks::TaskFilter;
use kernel::tasks::TaskStatus;
use serde::Deserialize;
use uuid::Uuid;
use wire::task::TaskResponse;
use wire::task::TasksListResponse;

use super::body::store_error;
use super::body::tasks_of;
use super::error::TaskHttpError;
use super::present::to_wire;
use super::wait::WaitHit;
use super::wait::wait_limit;
use super::wait::wait_until;
use crate::Gateway;
use crate::extract::Query;

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
    timeout_ms: Option<String>,
}

pub(super) async fn list(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<TasksListResponse>, TaskHttpError> {
    let filter = TaskFilter {
        status: status_of(query.status.as_deref())?,
        agent: query.agent_id,
        limit: limit_of(query.limit.as_deref())?,
    };
    let records = tasks_of(&gateway)?
        .store
        .list(&filter)
        .await
        .map_err(|error| store_error(&error))?;
    let tasks = records.iter().map(to_wire).collect::<Result<_, _>>()?;
    Ok(Json(TasksListResponse { tasks }))
}

pub(super) async fn get_one(
    State(gateway): State<Arc<Gateway>>,
    Path(id): Path<String>,
) -> Result<Json<TaskResponse>, TaskHttpError> {
    let id = task_id(&id)?;
    let record = tasks_of(&gateway)?
        .store
        .load(id)
        .await
        .map_err(|error| store_error(&error))?;
    Ok(Json(TaskResponse {
        task: to_wire(&record)?,
    }))
}

pub(super) async fn wait(
    State(gateway): State<Arc<Gateway>>,
    Path(id): Path<String>,
    Query(query): Query<WaitQuery>,
) -> Result<Response, TaskHttpError> {
    let id = task_id(&id)?;
    let timeout = wait_limit(query.timeout_ms.as_deref())?;
    match wait_until(tasks_of(&gateway)?, id, timeout).await? {
        WaitHit::Task(task) => Ok(Json(TaskResponse { task }).into_response()),
        WaitHit::Deadline(task) => Err(TaskHttpError::wait_left(task)),
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

fn status_of(raw: Option<&str>) -> Result<Option<TaskStatus>, TaskHttpError> {
    match raw {
        None => Ok(None),
        Some(text) => match text.parse::<TaskStatus>() {
            Ok(status) => Ok(Some(status)),
            Err(_) => Err(TaskHttpError::BadRequest(format!(
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
