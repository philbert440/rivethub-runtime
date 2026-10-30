use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use wire::capture::CaptureResult;

use crate::Gateway;
use crate::error::ApiError;
use crate::routes::capture_schema::ValidatedBatch;

pub(crate) async fn ingest(
    State(gateway): State<Arc<Gateway>>,
    ValidatedBatch(batch): ValidatedBatch,
) -> Result<Json<CaptureResult>, ApiError> {
    let session_key = batch.session_key.clone();
    let captured = gateway
        .store()
        .capture(batch, Some(&gateway.settings().node))
        .await?;
    Ok(Json(CaptureResult {
        ok: true,
        conversation_id: session_key,
        inserted: captured.inserted,
        skipped: captured.skipped,
    }))
}
