mod file;
mod normalize;

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing;
use kernel::hub_state::HubStateStore;
use kernel::hub_state::HubStateStoreError;
use kernel::hub_state::HubStateWrite;
use serde_json::error::Category;
use wire::JsonObject;
use wire::gateway::hub_state::HubStatePutRequest;
use wire::gateway::hub_state::HubStateResponse;

use crate::Gateway;
use crate::error::ApiError;
use crate::error::body_must_be_object;
use crate::error::invalid_json;
use crate::extract::limited_bytes;
use crate::tenancy::UserId;

pub(crate) use file::FileHubState;

const BODY_LIMIT: usize = 1024 * 1024;

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route(
            "/api/hub/state",
            routing::get(load)
                .put(replace)
                .fallback(super::method_not_allowed),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

pub(crate) fn open(settings: &crate::Settings) -> Arc<dyn HubStateStore> {
    Arc::new(FileHubState::new(&settings.state_dir))
}

async fn load(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
) -> Result<Json<HubStateResponse>, ApiError> {
    let current = gateway
        .hub_state()
        .load(user.as_str())
        .await
        .map_err(|error| unavailable(&error))?;
    Ok(Json(current))
}

async fn replace(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    request: Request,
) -> Result<Response, ApiError> {
    let body = read_request(request).await?;
    let state = normalize::normalize(body.state)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let written = gateway
        .hub_state()
        .replace(user.as_str(), body.base_version, state)
        .await
        .map_err(|error| unavailable(&error))?;
    match written {
        HubStateWrite::Saved(saved) => {
            gateway.sessions.hub_state_changed(&user, saved.version);
            Ok(Json(saved).into_response())
        }
        HubStateWrite::Conflict(current) => {
            Ok((StatusCode::CONFLICT, Json(current)).into_response())
        }
    }
}

async fn read_request(request: Request) -> Result<HubStatePutRequest, ApiError> {
    let bytes = limited_bytes(request, &()).await?;
    if let Err(error) = serde_json::from_slice::<JsonObject>(&bytes) {
        return Err(if error.classify() == Category::Data {
            body_must_be_object()
        } else {
            invalid_json()
        });
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        if error.classify() == Category::Data {
            ApiError::BadRequest(without_location(&error.to_string()))
        } else {
            invalid_json()
        }
    })
}

fn without_location(message: &str) -> String {
    match message.split_once(" at line ") {
        Some((head, _)) => head.to_owned(),
        None => message.to_owned(),
    }
}

fn unavailable(error: &HubStateStoreError) -> ApiError {
    tracing::warn!(%error, "hub state store failed");
    ApiError::Unavailable(error.to_string())
}
