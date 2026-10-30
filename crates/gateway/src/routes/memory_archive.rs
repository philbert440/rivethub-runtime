use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use kernel::memory::SummaryOwner;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::memory::MemorySummaryDagResponse;

use crate::Gateway;
use crate::error::ApiError;
use crate::extract::Query;
use crate::extract::SinceParam;

const EXPORT_MEDIA_TYPE: &str = "application/gzip";

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ExportParams {
    since: Option<SinceParam>,
}

pub(crate) async fn summaries(
    State(gateway): State<Arc<Gateway>>,
    Path(session): Path<String>,
) -> Result<Json<MemorySummaryDagResponse>, ApiError> {
    let dag = gateway
        .store()
        .summary_dag(&SummaryOwner::Session(session.clone()))
        .await?;
    Ok(Json(MemorySummaryDagResponse {
        session_id: session,
        summaries: dag.summaries,
        messages: dag.messages,
    }))
}

pub(crate) async fn conversation_summaries(
    State(gateway): State<Arc<Gateway>>,
    Path(segment): Path<String>,
) -> Result<Json<MemorySummaryDagResponse>, ApiError> {
    let conversation = Uuid::parse_str(&segment)
        .map_err(|_| ApiError::BadRequest(format!("{segment} is not a conversation id")))?;
    let dag = gateway
        .store()
        .summary_dag(&SummaryOwner::Conversation(conversation))
        .await?;
    let session_id = dag
        .session_key
        .ok_or_else(|| ApiError::Missing(format!("unknown conversation {segment}")))?;
    Ok(Json(MemorySummaryDagResponse {
        session_id,
        summaries: dag.summaries,
        messages: dag.messages,
    }))
}

pub(crate) async fn export(
    State(gateway): State<Arc<Gateway>>,
    Query(params): Query<ExportParams>,
) -> Result<Response, ApiError> {
    let exporter = gateway
        .memory_export()
        .ok_or_else(|| ApiError::Unavailable("memory export is not configured".to_owned()))?;
    let cancellation = CancellationToken::new();
    let abandoned = cancellation.clone().drop_guard();
    let dump = exporter
        .export(params.since.map(SinceParam::get), cancellation)
        .await?;
    drop(abandoned);
    let disposition = format!(
        "attachment; filename=\"memory-{}.sql.gz\"",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    Ok((
        [
            (header::CONTENT_TYPE, EXPORT_MEDIA_TYPE.to_owned()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        dump,
    )
        .into_response())
}
