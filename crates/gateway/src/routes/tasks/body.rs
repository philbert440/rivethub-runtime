use axum::extract::Request;
use kernel::tasks::TaskError;
use serde::de::DeserializeOwned;
use wire::task::TaskStatus;

use crate::error::ApiError;
use crate::extract::limited_bytes;

use super::error::TaskHttpError;

pub(super) async fn read_json<T: DeserializeOwned>(request: Request) -> Result<T, TaskHttpError> {
    let bytes = limited_bytes(request, &()).await.map_err(body_error)?;
    crate::extract::decode_json_object(&bytes).map_err(body_error)
}

pub(super) fn required(value: Option<String>, label: &str) -> Result<String, TaskHttpError> {
    match value {
        Some(text) if !text.is_empty() => Ok(text),
        Some(_) | None => Err(TaskHttpError::BadRequest(format!(
            "{label} (string) is required"
        ))),
    }
}

pub(super) fn store_error(error: &TaskError) -> TaskHttpError {
    let message = error.to_string();
    match error {
        TaskError::Missing { id } => TaskHttpError::NotFound(format!("no task {id}")),
        TaskError::Conflict { status, .. } => terminal(*status),
        TaskError::Unavailable(_) => TaskHttpError::Unavailable(message),
        TaskError::UnavailableCatalog(_)
        | TaskError::Operation { .. }
        | TaskError::QueueAbsent
        | TaskError::Cleanup { .. }
        | TaskError::Payload(_)
        | TaskError::Stored(_)
        | TaskError::MissingJob { .. }
        | TaskError::Clock => TaskHttpError::Failed(message),
    }
}

pub(super) fn catalog_error(error: &TaskError) -> TaskHttpError {
    let message = error.to_string();
    match error {
        TaskError::Unavailable(_) => TaskHttpError::Unavailable(message),
        TaskError::UnavailableCatalog(_)
        | TaskError::Operation { .. }
        | TaskError::QueueAbsent
        | TaskError::Cleanup { .. }
        | TaskError::Payload(_)
        | TaskError::Stored(_)
        | TaskError::Missing { .. }
        | TaskError::MissingJob { .. }
        | TaskError::Conflict { .. }
        | TaskError::Clock => TaskHttpError::Failed(message),
    }
}

pub(super) fn terminal(status: TaskStatus) -> TaskHttpError {
    TaskHttpError::Conflict(format!("task is terminal ({status})"))
}

fn body_error(error: ApiError) -> TaskHttpError {
    match error {
        ApiError::BodyMustBeObject => {
            TaskHttpError::BadRequest("body must be a JSON object".to_owned())
        }
        ApiError::BodyTooLarge => TaskHttpError::PayloadTooLarge,
        ApiError::BadRequest(message) | ApiError::Unprocessable(message) => {
            TaskHttpError::BadRequest(message)
        }
        ApiError::RequestTimeout => TaskHttpError::BadRequest("request timeout".to_owned()),
        ApiError::NotFound(message) => TaskHttpError::NotFound(message.to_owned()),
        ApiError::Missing(message) => TaskHttpError::NotFound(message),
        ApiError::Code(_, message) => TaskHttpError::BadRequest(message),
        ApiError::MethodNotAllowed => TaskHttpError::MethodNotAllowed,
        ApiError::Conflict(message) => TaskHttpError::Conflict(message),
        ApiError::Forbidden(message) | ApiError::Unavailable(message) => {
            TaskHttpError::Failed(message)
        }
        ApiError::SessionForbidden => {
            TaskHttpError::Failed("session is owned by another user".to_owned())
        }
        ApiError::OwnerRequired => {
            TaskHttpError::Failed("only the node's owner can do this".to_owned())
        }
        ApiError::Unauthorized => TaskHttpError::Failed("unauthorized".to_owned()),
        ApiError::ForbiddenOrigin => TaskHttpError::Failed("forbidden origin".to_owned()),
        ApiError::Internal => TaskHttpError::Failed("internal error".to_owned()),
        ApiError::UnknownCommand(command) => {
            TaskHttpError::Failed(format!("unknown command: {command}"))
        }
        ApiError::Transcript(error) => TaskHttpError::Failed(error.to_string()),
        ApiError::Terminal(error) => TaskHttpError::Failed(error.to_string()),
        ApiError::Harness(error) => TaskHttpError::Failed(error.to_string()),
        ApiError::Io(error) => TaskHttpError::Failed(error.to_string()),
        ApiError::Task(error) => TaskHttpError::Failed(error.to_string()),
        ApiError::Serialize(error) => TaskHttpError::Failed(error.to_string()),
        ApiError::MeshParse(code) => {
            TaskHttpError::Failed(format!("mesh.json parse failed: {code}"))
        }
        ApiError::Store(error) => TaskHttpError::Failed(error.to_string()),
    }
}
