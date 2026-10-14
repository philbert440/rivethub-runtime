use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::Response;
use serde::Deserialize;
use wire::JsonObject;
use wire::gateway::OkReply;

use crate::Gateway;
use crate::error::ApiError;
use crate::error::invalid_json;
use crate::extract::Query;
use crate::extract::limited_bytes;

#[derive(Debug, Deserialize)]
pub(super) struct LayoutQuery {
    #[serde(default = "default_viewer")]
    viewer: String,
}

fn default_viewer() -> String {
    "default".to_owned()
}

pub(super) async fn get(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<LayoutQuery>,
) -> Result<Response, ApiError> {
    let key = viewer_key(&query.viewer)?;
    let dir = layouts_dir(gateway.settings());
    let bytes = read_layout(dir, key).await?;
    json_bytes(bytes)
}

pub(super) async fn post(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<LayoutQuery>,
    request: Request,
) -> Result<Json<OkReply>, ApiError> {
    let key = viewer_key(&query.viewer)?;
    let bytes = limited_bytes(request, gateway.as_ref()).await?;
    if serde_json::from_slice::<JsonObject>(&bytes).is_err() {
        return Err(invalid_json());
    }
    let dir = layouts_dir(gateway.settings());
    write_layout(dir, key, bytes.to_vec()).await?;
    Ok(Json(OkReply { ok: true }))
}

fn viewer_key(raw: &str) -> Result<String, ApiError> {
    if valid_viewer(raw) {
        Ok(raw.to_owned())
    } else {
        Err(ApiError::BadRequest("bad viewer key".to_owned()))
    }
}

fn valid_viewer(raw: &str) -> bool {
    let len = raw.chars().count();
    (1..=64).contains(&len)
        && raw
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-')
}

fn layouts_dir(settings: &crate::Settings) -> PathBuf {
    settings.state_dir.join("layouts")
}

async fn read_layout(dir: PathBuf, key: String) -> Result<Vec<u8>, ApiError> {
    let joined = tokio::task::spawn_blocking(move || load_layout(&dir, &key)).await;
    super::super::join_task(joined)?
}

async fn write_layout(dir: PathBuf, key: String, bytes: Vec<u8>) -> Result<(), ApiError> {
    let joined = tokio::task::spawn_blocking(move || store_layout(&dir, &key, &bytes)).await;
    super::super::join_task(joined)?
}

fn load_layout(dir: &Path, key: &str) -> Result<Vec<u8>, ApiError> {
    if let Some(bytes) = read_optional(&layout_path(dir, key))? {
        return Ok(bytes);
    }
    if key != "default"
        && let Some(bytes) = read_optional(&layout_path(dir, "default"))?
    {
        return Ok(bytes);
    }
    Err(ApiError::NotFound("no layout"))
}

fn store_layout(dir: &Path, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
    std::fs::create_dir_all(dir).map_err(io_error)?;
    let target = layout_path(dir, key);
    let temporary = dir.join(format!("{key}.json.tmp"));
    std::fs::write(&temporary, bytes).map_err(io_error)?;
    std::fs::rename(&temporary, &target).map_err(io_error)?;
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, ApiError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn layout_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

fn io_error(error: io::Error) -> ApiError {
    tracing::error!("layout file: {error}");
    ApiError::Io(error)
}

fn json_bytes(bytes: Vec<u8>) -> Result<Response, ApiError> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .map_err(|error| {
            tracing::error!("layout response: {error}");
            ApiError::Internal
        })
}

#[cfg(test)]
mod tests {
    use super::valid_viewer;

    #[test]
    fn viewer_keys_accept_the_recorded_alphabet_and_refuse_traversal() {
        assert!(valid_viewer("default"));
        assert!(valid_viewer("a.b-c_d"));
        assert!(valid_viewer("a"));
        assert!(valid_viewer(&"k".repeat(64)));
        assert!(!valid_viewer(""));
        assert!(!valid_viewer("a/b"));
        assert!(!valid_viewer("../x"));
        assert!(!valid_viewer(&"k".repeat(65)));
        assert!(!valid_viewer("has space"));
        assert!(!valid_viewer("café"));
    }
}
