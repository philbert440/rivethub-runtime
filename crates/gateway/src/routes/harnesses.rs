use std::pin::Pin;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Buf;
use chrono::Utc;
use http_body::Body as HttpBody;
use serde::Deserialize;
use wire::gateway::uploads::UploadList;
use wire::harness::HarnessOfferingList;

use super::sessions::Accepted;
use crate::Gateway;
use crate::error::ApiError;
use crate::tenancy::UserId;
use crate::uploads::{NewUpload, UploadError, Uploads};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadQuery {
    name: Option<String>,
    media_type: Option<String>,
    session: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    session: Option<String>,
}

pub(crate) fn mount(router: Router<Arc<Gateway>>) -> Router<Arc<Gateway>> {
    router.merge(routes())
}

fn routes() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/api/harnesses", get(list_harnesses))
        .route("/api/uploads", get(list_uploads).post(create_upload))
        .route(
            "/api/uploads/{id}",
            get(download_upload).delete(delete_upload),
        )
        .layer(DefaultBodyLimit::disable())
}

async fn list_harnesses(
    State(gateway): State<Arc<Gateway>>,
) -> Result<Json<HarnessOfferingList>, ApiError> {
    Ok(Json(HarnessOfferingList {
        harnesses: crate::history::offerings(&gateway).await?,
    }))
}

async fn list_uploads(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Query(query): Query<ListQuery>,
) -> Result<Json<UploadList>, ApiError> {
    let store = open_store(&gateway).await?;
    let uploads = store
        .list(&user, query.session.as_deref(), Utc::now())
        .await
        .map_err(upload_error)?;
    Ok(Json(UploadList { uploads }))
}

async fn create_upload(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Query(query): Query<UploadQuery>,
    request: axum::extract::Request,
) -> Result<(StatusCode, Json<wire::gateway::uploads::UploadCreated>), ApiError> {
    let store = open_store(&gateway).await?;
    let session = bound_session(&gateway, &user, query.session.as_deref())?;
    let name = required(query.name.as_deref(), "upload name is required")?;
    let media_type = required(query.media_type.as_deref(), "media type is required")?;
    let bytes = collect_upload(request.into_body(), store.max_bytes()).await?;
    let created = store
        .create(NewUpload {
            owner: &user,
            name,
            media_type,
            session,
            bytes: &bytes,
            now: Utc::now(),
        })
        .await
        .map_err(upload_error)?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn download_upload(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let store = open_store(&gateway).await?;
    let (media_type, bytes) = store
        .read(&user, &id, Utc::now())
        .await
        .map_err(upload_error)?;
    let header = HeaderValue::from_str(&media_type)
        .map_err(|_| ApiError::Io(std::io::Error::other("stored media type is not a header")))?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, header);
    Ok(response)
}

async fn delete_upload(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(id): Path<String>,
) -> Result<Json<Accepted>, ApiError> {
    let store = open_store(&gateway).await?;
    store
        .delete(&user, &id, Utc::now())
        .await
        .map_err(upload_error)?;
    Ok(Json(Accepted { ok: true }))
}

pub(super) async fn open_store(gateway: &Gateway) -> Result<Arc<Uploads>, ApiError> {
    let service = gateway
        .uploads()
        .ok_or_else(|| ApiError::Unavailable("uploads are not configured".to_owned()))?;
    service.store().await.map_err(upload_error)
}

fn bound_session(
    gateway: &Gateway,
    user: &UserId,
    session: Option<&str>,
) -> Result<Option<String>, ApiError> {
    match session {
        Some(session) => Ok(Some(gateway.sessions().authorize(user, session)?)),
        None => Ok(None),
    }
}

fn required<'a>(value: Option<&'a str>, message: &str) -> Result<&'a str, ApiError> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) | None => Err(ApiError::BadRequest(message.to_owned())),
    }
}

async fn collect_upload(mut body: Body, max: u64) -> Result<Vec<u8>, ApiError> {
    let mut collected = Vec::new();
    loop {
        let next = std::future::poll_fn(|context| Pin::new(&mut body).poll_frame(context)).await;
        let Some(next) = next else {
            break;
        };
        let frame = next.map_err(|_| ApiError::BadRequest("upload body failed".to_owned()))?;
        match frame.into_data() {
            Ok(data) => append_buf(&mut collected, data, max)?,
            Err(frame) => note_trailer(&frame)?,
        }
    }
    Ok(collected)
}

fn note_trailer<T>(frame: &http_body::Frame<T>) -> Result<(), ApiError> {
    if frame.is_trailers() {
        return Ok(());
    }
    Err(ApiError::BadRequest("upload body failed".to_owned()))
}

fn append_buf(collected: &mut Vec<u8>, mut data: impl Buf, max: u64) -> Result<(), ApiError> {
    let added = u64::try_from(data.remaining()).map_err(|_| ApiError::BodyTooLarge)?;
    let len = u64::try_from(collected.len()).map_err(|_| ApiError::BodyTooLarge)?;
    let Some(next) = len.checked_add(added) else {
        return Err(ApiError::BodyTooLarge);
    };
    if next > max {
        return Err(ApiError::BodyTooLarge);
    }
    collected.reserve(data.remaining());
    while data.has_remaining() {
        let chunk = data.chunk();
        collected.extend_from_slice(chunk);
        let read = chunk.len();
        data.advance(read);
    }
    Ok(())
}

pub(super) fn upload_error(error: UploadError) -> ApiError {
    match error {
        UploadError::TooLarge { .. } => ApiError::BodyTooLarge,
        UploadError::Empty => ApiError::BadRequest("upload is empty".to_owned()),
        UploadError::InvalidName => ApiError::BadRequest("upload name is invalid".to_owned()),
        UploadError::InvalidMediaType => ApiError::BadRequest("media type is invalid".to_owned()),
        UploadError::InvalidId => ApiError::BadRequest("invalid upload id".to_owned()),
        UploadError::Ttl => ApiError::BadRequest("upload ttl must be positive".to_owned()),
        UploadError::Limits => ApiError::BadRequest("upload limits must be positive".to_owned()),
        UploadError::NotFound => ApiError::NotFound("unknown upload"),
        UploadError::Escape => {
            ApiError::Forbidden("upload path escapes the upload root".to_owned())
        }
        UploadError::Corrupt => ApiError::Io(std::io::Error::other("upload metadata is corrupt")),
        UploadError::Io(error) => ApiError::Io(error),
        UploadError::Task(error) => ApiError::Task(error),
    }
}
