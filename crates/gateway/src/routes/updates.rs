use std::collections::BTreeMap;
use std::fs::File;
use std::io::ErrorKind;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;
use wire::gateway::GatewayError;

use crate::Gateway;

const FEED: &str = "builds/rivethub";
const MANIFEST: &str = "latest.json";
const MANIFEST_LIMIT: u64 = 1024 * 1024;
const CHUNK: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
enum UpdatesError {
    #[error("this node has no update feed")]
    NoFeed,
    #[error("no update manifest on this node")]
    NoManifest,
    #[error("the update manifest exceeds 1 MiB")]
    ManifestTooLarge,
    #[error("the update manifest does not parse: {0}")]
    ManifestInvalid(String),
    #[error("{0} is not a plain file name")]
    BadName(String),
    #[error("{0} is not named in the update manifest")]
    Unlisted(String),
    #[error("{0} is not a file in the update feed")]
    Missing(String),
    #[error("the update feed reaches outside its folder")]
    Escape,
    #[error("reading the update feed failed: {0}")]
    Io(String),
}

impl IntoResponse for UpdatesError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::NoFeed | Self::NoManifest | Self::Unlisted(_) | Self::Missing(_) => {
                StatusCode::NOT_FOUND
            }
            Self::BadName(_) => StatusCode::BAD_REQUEST,
            Self::Escape => StatusCode::FORBIDDEN,
            Self::ManifestTooLarge | Self::ManifestInvalid(_) | Self::Io(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (status, Json(GatewayError::new(self.to_string()))).into_response()
    }
}

#[derive(Debug, Deserialize)]
struct Release {
    file: String,
}

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route(
            "/api/updates/manifest",
            get(manifest).fallback(super::method_not_allowed),
        )
        .route(
            "/api/updates/{file}",
            get(artifact).fallback(super::method_not_allowed),
        )
}

async fn manifest(State(gateway): State<Arc<Gateway>>) -> Result<Response, UpdatesError> {
    let root = gateway.settings().files_root.clone();
    let bytes = blocking(move || manifest_bytes(&feed(root.as_deref())?)).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}

async fn artifact(
    State(gateway): State<Arc<Gateway>>,
    extract::Path(name): extract::Path<String>,
) -> Result<Response, UpdatesError> {
    if !plain(&name) {
        return Err(UpdatesError::BadName(name));
    }
    let root = gateway.settings().files_root.clone();
    let wanted = name.clone();
    let (file, length) = blocking(move || {
        let folder = feed(root.as_deref())?;
        if !listed(&manifest_bytes(&folder)?, &wanted)? {
            return Err(UpdatesError::Unlisted(wanted));
        }
        open_in(&folder, &wanted)
    })
    .await?;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .map_err(|error| UpdatesError::Io(error.to_string()))?;
    let stream =
        tokio_util::io::ReaderStream::with_capacity(tokio::fs::File::from_std(file), CHUNK);
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (header::CONTENT_LENGTH, HeaderValue::from(length)),
            (header::CONTENT_DISPOSITION, disposition),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

fn plain(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|next| next.is_ascii_alphanumeric() || matches!(next, '.' | '_' | '+' | '-'))
}

fn feed(configured: Option<&Path>) -> Result<PathBuf, UpdatesError> {
    let configured = configured
        .filter(|root| !root.as_os_str().is_empty())
        .ok_or(UpdatesError::NoFeed)?;
    let root = canonical(configured)?;
    let expected = root.join(FEED);
    let folder = canonical(&expected)?;
    if folder != expected {
        return Err(UpdatesError::Escape);
    }
    if folder.is_dir() {
        Ok(folder)
    } else {
        Err(UpdatesError::NoFeed)
    }
}

fn canonical(path: &Path) -> Result<PathBuf, UpdatesError> {
    std::fs::canonicalize(path).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            UpdatesError::NoFeed
        } else {
            UpdatesError::Io(error.to_string())
        }
    })
}

fn manifest_bytes(folder: &Path) -> Result<Vec<u8>, UpdatesError> {
    let (file, length) = open_in(folder, MANIFEST).map_err(|error| {
        if matches!(error, UpdatesError::Missing(_)) {
            UpdatesError::NoManifest
        } else {
            error
        }
    })?;
    if length > MANIFEST_LIMIT {
        return Err(UpdatesError::ManifestTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MANIFEST_LIMIT)
        .read_to_end(&mut bytes)
        .map_err(|error| UpdatesError::Io(error.to_string()))?;
    Ok(bytes)
}

fn listed(manifest: &[u8], name: &str) -> Result<bool, UpdatesError> {
    let releases: BTreeMap<String, Option<Release>> = serde_json::from_slice(manifest)
        .map_err(|error| UpdatesError::ManifestInvalid(error.to_string()))?;
    Ok(releases
        .values()
        .flatten()
        .any(|release| release.file == name))
}

fn open_in(folder: &Path, name: &str) -> Result<(File, u64), UpdatesError> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(folder.join(name))
        .map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                UpdatesError::Missing(name.to_owned())
            } else if error.raw_os_error() == Some(libc::ELOOP) {
                UpdatesError::Escape
            } else {
                UpdatesError::Io(error.to_string())
            }
        })?;
    let meta = file
        .metadata()
        .map_err(|error| UpdatesError::Io(error.to_string()))?;
    if meta.is_file() {
        Ok((file, meta.len()))
    } else {
        Err(UpdatesError::Missing(name.to_owned()))
    }
}

async fn blocking<T, F>(work: F) -> Result<T, UpdatesError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, UpdatesError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| UpdatesError::Io(error.to_string()))?
}
