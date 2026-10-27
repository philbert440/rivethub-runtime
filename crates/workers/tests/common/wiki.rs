use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kernel::wiki::builder::{
    ExtractWiki, WikiBuilderStore, WikiPatch, WikiRouting, WikiStoreError, WikiWork,
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct WikiStore {
    work: Mutex<HashMap<Uuid, WikiWork>>,
    failures: Mutex<HashMap<Uuid, String>>,
}

#[async_trait::async_trait]
impl WikiBuilderStore for WikiStore {
    async fn wiki_work(&self, job: &ExtractWiki) -> Result<WikiWork, WikiStoreError> {
        self.work
            .lock()
            .await
            .get(&job.summary_id)
            .cloned()
            .ok_or(WikiStoreError::Missing(job.summary_id))
    }

    async fn wiki_backfill(&self, limit: u32) -> Result<Vec<ExtractWiki>, WikiStoreError> {
        Ok(self
            .work
            .lock()
            .await
            .values()
            .filter_map(|work| match work {
                WikiWork::Pending(summary) => Some(ExtractWiki {
                    summary_id: summary.id,
                    conversation_id: summary.conversation_id,
                }),
                WikiWork::Complete { .. } => None,
            })
            .take(usize::try_from(limit).expect("test limit"))
            .collect())
    }

    async fn apply_wiki(
        &self,
        job: &ExtractWiki,
        patches: Vec<WikiPatch>,
        _routing: &WikiRouting,
        _now: DateTime<Utc>,
    ) -> Result<Vec<String>, WikiStoreError> {
        let topics: Vec<String> = patches
            .into_iter()
            .map(|patch| String::from(patch.slug))
            .collect();
        self.work.lock().await.insert(
            job.summary_id,
            WikiWork::Complete {
                topics: topics.clone(),
            },
        );
        Ok(topics)
    }

    async fn fail_wiki(
        &self,
        summary_id: Uuid,
        error: &str,
        _now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError> {
        self.failures
            .lock()
            .await
            .insert(summary_id, error.to_owned());
        Ok(())
    }
}
