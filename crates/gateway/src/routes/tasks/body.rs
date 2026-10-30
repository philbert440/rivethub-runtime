use std::sync::Arc;

use axum::extract::Request;
use kernel::tasks::TaskError;
use serde::de::DeserializeOwned;

use super::error::TaskHttpError;
use crate::Gateway;
use crate::error::ApiError;
use crate::extract::limited_bytes;
use crate::tasks::Tasks;

pub(super) fn tasks_of(gateway: &Gateway) -> Result<&Arc<Tasks>, TaskHttpError> {
    gateway
        .tasks()
        .ok_or_else(|| TaskHttpError::Unavailable("tasks are not configured on this node".into()))
}

pub(super) async fn read_json<T: DeserializeOwned>(request: Request) -> Result<T, TaskHttpError> {
    let bytes = limited_bytes(request, &()).await.map_err(body_error)?;
    crate::extract::decode_json_object(&bytes).map_err(body_error)
}

pub(super) fn store_error(error: &TaskError) -> TaskHttpError {
    let message = error.to_string();
    match error {
        TaskError::Missing { id } => TaskHttpError::NotFound(format!("no task {id}")),
        TaskError::Unavailable(_) => TaskHttpError::Unavailable(message),
        TaskError::Operation { .. }
        | TaskError::QueueAbsent
        | TaskError::Cleanup { .. }
        | TaskError::Payload(_)
        | TaskError::Stored(_)
        | TaskError::MissingJob { .. } => TaskHttpError::Failed(message),
    }
}

fn body_error(error: ApiError) -> TaskHttpError {
    match error {
        ApiError::BodyTooLarge => TaskHttpError::PayloadTooLarge,
        ApiError::BodyMustBeObject => {
            TaskHttpError::BadRequest("body must be a JSON object".to_owned())
        }
        ApiError::BadRequest(message) | ApiError::Unprocessable(message) => {
            TaskHttpError::BadRequest(message)
        }
        other @ (ApiError::Transcript(_)
        | ApiError::UnknownCommand(_)
        | ApiError::Terminal(_)
        | ApiError::Harness(_)
        | ApiError::Io(_)
        | ApiError::Task(_)
        | ApiError::SessionForbidden
        | ApiError::OwnerRequired
        | ApiError::Forbidden(_)
        | ApiError::Conflict(_)
        | ApiError::Unavailable(_)
        | ApiError::NotFound(_)
        | ApiError::Missing(_)
        | ApiError::Code(..)
        | ApiError::MethodNotAllowed
        | ApiError::RequestTimeout
        | ApiError::Unauthorized
        | ApiError::ForbiddenOrigin
        | ApiError::Serialize(_)
        | ApiError::Internal
        | ApiError::MeshParse(_)
        | ApiError::Store(_)) => TaskHttpError::Failed(other.to_string()),
    }
}
