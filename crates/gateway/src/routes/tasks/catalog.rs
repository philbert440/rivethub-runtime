use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use wire::gateway::catalog::CatalogAgentsResponse;

use super::body::tasks_of;
use super::error::TaskHttpError;
use crate::Gateway;

pub(super) async fn agents(
    State(gateway): State<Arc<Gateway>>,
) -> Result<Json<CatalogAgentsResponse>, TaskHttpError> {
    let tasks = tasks_of(&gateway)?;
    let sources = tasks.sources(&gateway).await.map_err(TaskHttpError::from)?;
    Ok(Json(CatalogAgentsResponse {
        agents: sources.catalog(),
    }))
}
