use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use kernel::tasks::ApprovalBroker;
use kernel::tasks::ApprovalDecision;
use kernel::tasks::ApprovalError;
use kernel::tasks::ConflictAction;
use kernel::tasks::TaskError;
use serde::Deserialize;
use uuid::Uuid;
use wire::task::TaskApprovalAccepted;
use wire::task::TaskKillResponse;
use wire::task::TaskSteerAccepted;

use super::TasksApi;
use super::body::read_json;
use super::body::required;
use super::body::store_error;
use super::body::terminal;
use super::error::TaskHttpError;
use super::job::enqueue;
use super::read::task_id;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SteerBody {
    message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DecisionBody {
    decision: Option<String>,
}

pub(super) async fn steer(
    State(api): State<Arc<TasksApi>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<(StatusCode, Json<TaskSteerAccepted>), TaskHttpError> {
    let id = task_id(&id)?;
    let body: SteerBody = read_json(request).await?;
    let message = required(body.message, "message")?;
    let record = api
        .store
        .load(id)
        .await
        .map_err(|error| store_error(&error))?;
    if kernel::tasks::terminal(record.status) {
        return Err(terminal(record.status));
    }
    api.store
        .set_pending(id, &message)
        .await
        .map_err(|error| store_error(&error))?;
    let record = api
        .store
        .load(id)
        .await
        .map_err(|error| store_error(&error))?;
    enqueue(&api, &record).await?;
    Ok((StatusCode::ACCEPTED, Json(TaskSteerAccepted { ok: true })))
}

pub(super) async fn kill(
    State(api): State<Arc<TasksApi>>,
    Path(id): Path<String>,
) -> Result<Json<TaskKillResponse>, TaskHttpError> {
    let id = task_id(&id)?;
    let prior = match api.store.kill(id, Utc::now()).await {
        Ok(prior) => prior,
        Err(TaskError::Conflict {
            action: ConflictAction::Kill,
            status,
            ..
        }) => status,
        Err(error) => return Err(store_error(&error)),
    };
    Ok(Json(TaskKillResponse {
        ok: true,
        prior: Some(prior),
    }))
}

pub(super) async fn missing_request(Path(id): Path<String>) -> TaskHttpError {
    match task_id(&id) {
        Ok(_) => TaskHttpError::BadRequest("requestId is required".to_owned()),
        Err(error) => error,
    }
}

pub(super) async fn approve(
    State(api): State<Arc<TasksApi>>,
    Path((id, request_id)): Path<(String, String)>,
    request: Request,
) -> Result<(StatusCode, Json<TaskApprovalAccepted>), TaskHttpError> {
    let id = task_id(&id)?;
    if request_id.is_empty() {
        return Err(TaskHttpError::BadRequest(
            "requestId is required".to_owned(),
        ));
    }
    let Some(broker) = api.approvals.clone() else {
        return Err(TaskHttpError::NotFound("unknown approval".to_owned()));
    };
    let body: DecisionBody = read_json(request).await?;
    let decision = decision_of(body.decision.as_deref())?;
    reject_if_terminal(&api, broker.as_ref(), id).await?;
    broker
        .resolve(id, &request_id, decision)
        .await
        .map_err(|error| unknown_approval(&error))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(TaskApprovalAccepted { ok: true }),
    ))
}

fn decision_of(raw: Option<&str>) -> Result<ApprovalDecision, TaskHttpError> {
    match raw {
        Some("allow") => Ok(ApprovalDecision::Allow),
        Some("deny") => Ok(ApprovalDecision::Deny),
        Some(_) | None => Err(TaskHttpError::BadRequest(
            "decision must be \"allow\" or \"deny\"".to_owned(),
        )),
    }
}

async fn reject_if_terminal(
    api: &TasksApi,
    broker: &dyn ApprovalBroker,
    id: Uuid,
) -> Result<(), TaskHttpError> {
    let record = api
        .store
        .load(id)
        .await
        .map_err(|error| store_error(&error))?;
    if !kernel::tasks::terminal(record.status) {
        return Ok(());
    }
    broker
        .deny_pending(id, "task is terminal")
        .await
        .map_err(|error| approval_failed(&error))?;
    Err(terminal(record.status))
}

fn approval_failed(error: &ApprovalError) -> TaskHttpError {
    let message = error.to_string();
    match error {
        ApprovalError::Unknown => TaskHttpError::Failed(message),
    }
}

fn unknown_approval(error: &ApprovalError) -> TaskHttpError {
    match error {
        ApprovalError::Unknown => TaskHttpError::NotFound("unknown approval".to_owned()),
    }
}
