use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use chrono::Utc;
use wire::task::TaskKillResponse;

use super::body::store_error;
use super::body::tasks_of;
use super::error::TaskHttpError;
use super::present::status_out;
use super::read::task_id;
use crate::Gateway;

pub(super) async fn kill(
    State(gateway): State<Arc<Gateway>>,
    Path(id): Path<String>,
) -> Result<Json<TaskKillResponse>, TaskHttpError> {
    let id = task_id(&id)?;
    let tasks = tasks_of(&gateway)?;
    let prior = tasks
        .store
        .kill(id, Utc::now())
        .await
        .map_err(|error| store_error(&error))?;
    Ok(Json(TaskKillResponse {
        ok: true,
        prior: Some(status_out(prior)),
    }))
}
