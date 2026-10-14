mod disk;
mod fence;
mod mime;
mod upload;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::Request;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::Method;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use wire::gateway::GatewayError;
use wire::gateway::files::FileChanged;
use wire::gateway::files::FileList;

use crate::Gateway;

#[derive(Debug, thiserror::Error)]
pub(super) enum FilesError {
    #[error("files browser disabled on this node")]
    Disabled,
    #[error("files root not available: {root}")]
    Unavailable { root: PathBuf },
    #[error("path escapes the files root")]
    Escape,
    #[error("invalid file name")]
    InvalidFileName,
    #[error("invalid directory name")]
    InvalidDirectoryName,
    #[error("no such directory")]
    NoSuchDirectory,
    #[error("no such file")]
    NoSuchFile,
    #[error("no such path")]
    NoSuchPath,
    #[error("destination directory does not exist")]
    DestinationMissing,
    #[error("from and to are required")]
    FromToRequired,
    #[error("from and to are the same path")]
    SamePath,
    #[error("cannot rename the files root")]
    RenameRoot,
    #[error("cannot move a directory into itself")]
    MoveIntoSelf,
    #[error("rename failed: {0}")]
    RenameFailed(String),
    #[error("path is required")]
    PathRequired,
    #[error("cannot delete the files root")]
    DeleteRoot,
    #[error("directory not empty (pass recursive=1)")]
    NotEmpty,
    #[error("delete failed: {0}")]
    DeleteFailed(String),
    #[error("upload exceeds 1 GiB cap")]
    TooLarge,
    #[error("download exceeds 1 GiB cap")]
    DownloadTooLarge,
    #[error("file exists")]
    FileExists,
    #[error("directory exists")]
    DirectoryExists,
    #[error("upload failed: {0}")]
    UploadFailed(String),
    #[error("mkdir failed: {0}")]
    MkdirFailed(String),
    #[error("list failed: {0}")]
    ListFailed(String),
    #[error("download failed: {0}")]
    DownloadFailed(String),
    #[error("{0}")]
    BadQuery(String),
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("not found")]
    NotFound,
    #[error("file work failed: {0}")]
    Worker(String),
}

impl IntoResponse for FilesError {
    fn into_response(self) -> Response {
        let status = status_of(&self);
        let message = self.to_string();
        (status, Json(GatewayError::new(message))).into_response()
    }
}

fn status_of(error: &FilesError) -> StatusCode {
    match error {
        FilesError::Disabled | FilesError::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
        FilesError::Escape | FilesError::RenameRoot | FilesError::DeleteRoot => {
            StatusCode::FORBIDDEN
        }
        FilesError::InvalidFileName
        | FilesError::InvalidDirectoryName
        | FilesError::FromToRequired
        | FilesError::SamePath
        | FilesError::MoveIntoSelf
        | FilesError::PathRequired => StatusCode::UNPROCESSABLE_ENTITY,
        FilesError::NoSuchDirectory
        | FilesError::NoSuchFile
        | FilesError::NoSuchPath
        | FilesError::DestinationMissing
        | FilesError::NotFound => StatusCode::NOT_FOUND,
        FilesError::NotEmpty | FilesError::FileExists | FilesError::DirectoryExists => {
            StatusCode::CONFLICT
        }
        FilesError::TooLarge | FilesError::DownloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        FilesError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        FilesError::BadQuery(_) => StatusCode::BAD_REQUEST,
        FilesError::RenameFailed(_)
        | FilesError::DeleteFailed(_)
        | FilesError::UploadFailed(_)
        | FilesError::MkdirFailed(_)
        | FilesError::ListFailed(_)
        | FilesError::DownloadFailed(_)
        | FilesError::Worker(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub(crate) fn mount(router: Router<Arc<Gateway>>) -> Router<Arc<Gateway>> {
    router
        .route("/files/list", routing::any(list))
        .route("/files/download", routing::any(download))
        .route("/files/upload", routing::any(upload))
        .route("/files/mkdir", routing::any(mkdir))
        .route("/files/rename", routing::any(rename_path))
        .route("/files/delete", routing::any(delete_path))
        .route("/api/files/list", routing::any(list))
        .route("/api/files/download", routing::any(download))
        .route("/api/files/upload", routing::any(upload))
        .route("/api/files/mkdir", routing::any(mkdir))
        .route("/api/files/rename", routing::any(rename_path))
        .route("/api/files/delete", routing::any(delete_path))
        .route("/files/{*rest}", routing::any(unknown))
        .route("/api/files/{*rest}", routing::any(unknown))
}

async fn list(
    State(gateway): State<Arc<Gateway>>,
    request: Request,
) -> Result<Json<FileList>, FilesError> {
    let configured = configured_root(&gateway)?;
    require_method(request.method(), &Method::GET)?;
    let params: ListQuery = decode_query(request.uri())?;
    let raw = params.path.unwrap_or_default();
    let listed = blocking(move || disk::list(&configured, &raw)).await?;
    Ok(Json(listed))
}

async fn download(
    State(gateway): State<Arc<Gateway>>,
    request: Request,
) -> Result<Response, FilesError> {
    let configured = configured_root(&gateway)?;
    require_method(request.method(), &Method::GET)?;
    let params: DownloadQuery = decode_query(request.uri())?;
    let raw = params.path.unwrap_or_default();
    let file = blocking(move || disk::download(&configured, &raw)).await?;
    download_response(file)
}

async fn upload(
    State(gateway): State<Arc<Gateway>>,
    request: Request,
) -> Result<Json<FileChanged>, FilesError> {
    let configured = configured_root(&gateway)?;
    require_method(request.method(), &Method::POST)?;
    let (parts, mut body) = request.into_parts();
    let params: UploadQuery = decode_query(&parts.uri)?;
    let name = require_name(params.name, NameKind::File)?;
    let dir = params.dir.unwrap_or_default();
    let overwrite = flag_one(params.overwrite.as_deref());
    let seq = upload::next_seq();
    let plan =
        blocking(move || disk::prepare_upload(&configured, &dir, &name, overwrite, seq)).await?;
    let _guard = upload::TempUpload::new(plan.temp.clone());
    upload::write_capped(&plan.temp, &mut body).await?;
    let path = blocking(move || disk::finish_upload(&plan, overwrite)).await?;
    Ok(Json(FileChanged { ok: true, path }))
}

async fn mkdir(
    State(gateway): State<Arc<Gateway>>,
    request: Request,
) -> Result<Json<FileChanged>, FilesError> {
    let configured = configured_root(&gateway)?;
    require_method(request.method(), &Method::POST)?;
    let params: MkdirQuery = decode_query(request.uri())?;
    let name = require_name(params.name, NameKind::Directory)?;
    let dir = params.dir.unwrap_or_default();
    let path = blocking(move || disk::mkdir(&configured, &dir, &name)).await?;
    Ok(Json(FileChanged { ok: true, path }))
}

async fn rename_path(
    State(gateway): State<Arc<Gateway>>,
    request: Request,
) -> Result<Json<FileChanged>, FilesError> {
    let configured = configured_root(&gateway)?;
    require_method(request.method(), &Method::POST)?;
    let params: RenameQuery = decode_query(request.uri())?;
    let (from, to) = required_pair(params.from, params.to)?;
    let path = blocking(move || disk::rename(&configured, &from, &to)).await?;
    Ok(Json(FileChanged { ok: true, path }))
}

async fn delete_path(
    State(gateway): State<Arc<Gateway>>,
    request: Request,
) -> Result<Json<FileChanged>, FilesError> {
    let configured = configured_root(&gateway)?;
    require_method(request.method(), &Method::DELETE)?;
    let params: DeleteQuery = decode_query(request.uri())?;
    let raw = required_path(params.path)?;
    let recursive = flag_one(params.recursive.as_deref());
    let path = blocking(move || disk::delete(&configured, &raw, recursive)).await?;
    Ok(Json(FileChanged { ok: true, path }))
}

async fn unknown(State(gateway): State<Arc<Gateway>>) -> Result<Json<FileList>, FilesError> {
    configured_root(&gateway)?;
    Err(FilesError::NotFound)
}

fn download_response(file: disk::Download) -> Result<Response, FilesError> {
    let class = mime::classify(&file.name);
    let disposition = mime::content_disposition(class.inline, &file.name)?;
    let stream = tokio_util::io::ReaderStream::with_capacity(
        tokio::fs::File::from_std(file.file),
        64 * 1024,
    );
    let mut response = Response::new(axum::body::Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(class.content_type),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .map_err(|error| FilesError::DownloadFailed(error.to_string()))?,
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

fn configured_root(gateway: &Gateway) -> Result<PathBuf, FilesError> {
    match gateway.settings().files_root.as_ref() {
        Some(path) if !path.as_os_str().is_empty() => Ok(path.clone()),
        Some(_) | None => Err(FilesError::Disabled),
    }
}

fn require_method(actual: &Method, expected: &Method) -> Result<(), FilesError> {
    if actual == expected {
        Ok(())
    } else {
        Err(FilesError::MethodNotAllowed)
    }
}

fn flag_one(value: Option<&str>) -> bool {
    matches!(value, Some("1"))
}

fn required_path(raw: Option<String>) -> Result<String, FilesError> {
    match raw {
        Some(path) if !path.is_empty() => Ok(path),
        Some(_) | None => Err(FilesError::PathRequired),
    }
}

fn required_pair(from: Option<String>, to: Option<String>) -> Result<(String, String), FilesError> {
    let (Some(from), Some(to)) = (from, to) else {
        return Err(FilesError::FromToRequired);
    };
    if from.is_empty() || to.is_empty() {
        return Err(FilesError::FromToRequired);
    }
    Ok((from, to))
}

#[derive(Debug, Clone, Copy)]
enum NameKind {
    File,
    Directory,
}

fn require_name(raw: Option<String>, kind: NameKind) -> Result<String, FilesError> {
    let Some(name) = raw else {
        return Err(invalid_name(kind));
    };
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(invalid_name(kind));
    }
    Ok(name)
}

fn invalid_name(kind: NameKind) -> FilesError {
    match kind {
        NameKind::File => FilesError::InvalidFileName,
        NameKind::Directory => FilesError::InvalidDirectoryName,
    }
}

fn decode_query<T: DeserializeOwned>(uri: &axum::http::Uri) -> Result<T, FilesError> {
    let text = uri.query().unwrap_or_default();
    serde_urlencoded::from_str(text).map_err(|error| FilesError::BadQuery(error.to_string()))
}

async fn blocking<T, F>(work: F) -> Result<T, FilesError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, FilesError> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(FilesError::Worker(error.to_string())),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListQuery {
    path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DownloadQuery {
    path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UploadQuery {
    dir: Option<String>,
    name: Option<String>,
    overwrite: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MkdirQuery {
    dir: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RenameQuery {
    from: Option<String>,
    to: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DeleteQuery {
    path: Option<String>,
    recursive: Option<String>,
}
