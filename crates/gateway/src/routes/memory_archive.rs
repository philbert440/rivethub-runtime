use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
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
    let summaries = gateway.store().summary_dag(&session).await?;
    Ok(Json(MemorySummaryDagResponse {
        session_id: session,
        summaries,
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
