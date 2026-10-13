use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use wire::capture::CaptureResult;

use crate::Gateway;
use crate::error::ApiError;
use crate::routes::capture_schema::ValidatedBatch;

pub(crate) async fn ingest(
    State(gateway): State<Arc<Gateway>>,
    batch: ValidatedBatch,
) -> Result<Json<CaptureResult>, ApiError> {
    let entries = batch.entries();
    let total = entries.len() as u64;
    let inserted = gateway.store().append(entries).await?;
    Ok(Json(CaptureResult {
        ok: true,
        conversation_id: batch.session_key,
        inserted,
        skipped: total.saturating_sub(inserted),
    }))
}
