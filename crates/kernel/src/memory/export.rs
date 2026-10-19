use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use tokio_util::sync::CancellationToken;

use super::MemoryStoreError;

#[async_trait]
pub trait MemoryExport: Send + Sync {
    async fn export(
        &self,
        since: Option<DateTime<Utc>>,
        cancellation: CancellationToken,
    ) -> Result<Vec<u8>, MemoryStoreError>;
}
