use kernel::memory::DeletedSession;
use kernel::memory::SessionDeletion;
use kernel::memory::TOMBSTONE_PURGE_BATCH;
use tokio_util::sync::CancellationToken;

use crate::WorkerError;

pub async fn handle_purge(
    deletions: &dyn SessionDeletion,
    cancel: &CancellationToken,
) -> Result<DeletedSession, WorkerError> {
    let purged = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(WorkerError::Cancelled),
        result = deletions.purge_tombstoned(TOMBSTONE_PURGE_BATCH) => result?,
    };
    if purged != DeletedSession::default() {
        tracing::info!(
            conversations = purged.conversations,
            messages = purged.messages,
            summaries = purged.summaries,
            "purged memory rows written for deleted sessions"
        );
    }
    Ok(purged)
}
