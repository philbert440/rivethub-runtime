use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use kernel::tasks::Budget;
use kernel::tasks::Criterion;
use serde::Deserialize;
use wire::task::TaskCreateRequest;
use wire::task::TaskResponse;

use super::body::read_json;
use super::body::tasks_of;
use super::error::TaskHttpError;
use super::present::to_wire;
use super::wait::WaitHit;
use super::wait::wait_limit;
use super::wait::wait_until;
use crate::Gateway;
use crate::extract::Query;
use crate::tasks::Submission;
use crate::tenancy::UserId;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct CreateQuery {
    wait: Option<String>,
    timeout_ms: Option<String>,
}

pub(super) async fn create(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Query(query): Query<CreateQuery>,
    request: Request,
) -> Result<Response, TaskHttpError> {
    let body: TaskCreateRequest = read_json(request).await?;
    warn_ignored(&body.extra);
    let timeout = wait_limit(query.timeout_ms.as_deref())?;
    let tasks = tasks_of(&gateway)?;
    let submission = submission(body)?;
    let task = tasks.submit(&gateway, &user, submission).await?;
    if !matches!(query.wait.as_deref(), Some("1" | "true")) {
        let task = to_wire(&task)?;
        return Ok((StatusCode::CREATED, Json(TaskResponse { task })).into_response());
    }
    match wait_until(tasks, task.id, timeout).await? {
        WaitHit::Task(task) => Ok((StatusCode::OK, Json(TaskResponse { task })).into_response()),
        WaitHit::Deadline(task) => Err(TaskHttpError::wait_left(task)),
    }
}

fn warn_ignored(extra: &wire::Extra) {
    if !extra.is_empty() {
        let fields: Vec<&str> = extra.keys().collect();
        tracing::warn!(
            ?fields,
            "POST /api/tasks ignored fields it does not take; retired ones are refused from the next release"
        );
    }
}

fn submission(body: TaskCreateRequest) -> Result<Submission, TaskHttpError> {
    let goal = required(body.goal, "goal")?;
    let agent = required(body.agent_id, "agentId")?;
    let criteria = body
        .acceptance_criteria
        .into_iter()
        .flatten()
        .map(|criterion| Criterion {
            id: criterion.id,
            description: criterion.description,
        })
        .collect();
    let budget = body.budget.unwrap_or_default();
    Ok(Submission {
        goal,
        agent,
        pin: body.node_affinity,
        criteria,
        budget: Budget {
            wall_clock: budget.max_wall_clock_ms.map(Duration::from_millis),
            max_turns: budget.max_turns,
        },
        parent: body.parent_session_id,
        model: body.model.filter(|model| !model.trim().is_empty()),
        effort: body.effort.map(|effort| effort.to_string()),
        cwd: body.working_dir.filter(|cwd| !cwd.is_empty()),
        requested_by: body.requested_by,
    })
}

fn required(value: Option<String>, label: &str) -> Result<String, TaskHttpError> {
    match value {
        Some(text) if !text.is_empty() => Ok(text),
        Some(_) | None => Err(TaskHttpError::BadRequest(format!(
            "{label} (string) is required"
        ))),
    }
}
