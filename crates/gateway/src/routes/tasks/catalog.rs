use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::State;
use axum::routing::get;
use wire::gateway::catalog::CatalogAgentsResponse;
use wire::gateway::catalog::CatalogCommand;
use wire::gateway::catalog::CatalogSheet;

use super::TasksApi;
use super::body::catalog_error;
use super::error::TaskHttpError;

pub(super) fn routes() -> Router<Arc<TasksApi>> {
    Router::new()
        .route("/", get(sheet).fallback(super::method_not_allowed))
        .route("/agents", get(agents).fallback(super::method_not_allowed))
        .route(
            "/{section}",
            get(missing).fallback(super::method_not_allowed),
        )
        .fallback(super::method_not_allowed)
}

async fn sheet(State(api): State<Arc<TasksApi>>) -> Result<Json<CatalogSheet>, TaskHttpError> {
    let agents = api
        .catalog
        .agents()
        .await
        .map_err(|error| catalog_error(&error))?;
    let mut executors = api
        .catalog
        .executors()
        .await
        .map_err(|error| catalog_error(&error))?;
    for entry in &mut executors {
        entry.commands = listed_commands(&api, &entry.key).await?;
    }
    Ok(Json(CatalogSheet {
        node: api.node.clone(),
        agents,
        executors,
        tools: api
            .catalog
            .tools()
            .await
            .map_err(|error| catalog_error(&error))?,
        skills: api
            .catalog
            .skills()
            .await
            .map_err(|error| catalog_error(&error))?,
    }))
}

async fn agents(
    State(api): State<Arc<TasksApi>>,
) -> Result<Json<CatalogAgentsResponse>, TaskHttpError> {
    Ok(Json(CatalogAgentsResponse {
        agents: api
            .catalog
            .agents()
            .await
            .map_err(|error| catalog_error(&error))?,
    }))
}

async fn missing(Path(section): Path<String>) -> TaskHttpError {
    TaskHttpError::NotFound(format!("no catalog section \"{section}\""))
}

async fn listed_commands(api: &TasksApi, key: &str) -> Result<Vec<CatalogCommand>, TaskHttpError> {
    match tokio::time::timeout(api.command_timeout, api.catalog.commands(key)).await {
        Ok(result) => result.map_err(|error| catalog_error(&error)),
        Err(_elapsed) => Ok(Vec::new()),
    }
}
