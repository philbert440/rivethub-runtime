use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kernel::wiki::builder::WikiStoreError;
use kernel::wiki::page::WikiSlug;
use kernel::wiki::recompile::{RecompileSource, Recompiled, SourceSummary, WikiRecompileStore};
use uuid::Uuid;

use super::{lock_slug, query_error, settle, topics};
use crate::PostgresMemory;
use crate::wiki_writer::{LockedRepository, WikiRepository};

const SUMMARIES: i64 = 400;

#[derive(Debug, sqlx::FromRow)]
struct SummaryRow {
    topic_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    content: String,
}

#[async_trait]
impl WikiRecompileStore for PostgresMemory {
    async fn recompile_source(
        &self,
        slug: &WikiSlug,
        since: DateTime<Utc>,
    ) -> Result<Option<RecompileSource>, WikiStoreError> {
        let repository = WikiRepository::new(self.directory()?);
        let Some(page) = repository.read_unlocked(slug).await? else {
            return Ok(None);
        };
        let history = repository.history_since(slug, since).await?;
        let summaries = sqlx::query_as::<_, SummaryRow>(
            "SELECT p.topic_id, s.created_at, s.content
               FROM rh_wiki_citations w
               JOIN rh_summaries s ON s.id = w.summary_id
               LEFT JOIN rh_conversations c ON c.id = s.conversation_id
               LEFT JOIN rh_session_placements p ON p.session_id = c.session_key
              WHERE w.topic_slug = $1
              ORDER BY s.created_at DESC, s.id
              LIMIT $2",
        )
        .bind(slug.as_ref())
        .bind(SUMMARIES)
        .fetch_all(&self.pool)
        .await
        .map_err(query_error)?
        .into_iter()
        .map(|row| SourceSummary {
            topic: row.topic_id.map(|topic| topic.to_string()),
            at: row.created_at,
            content: row.content,
        })
        .collect();
        Ok(Some(RecompileSource {
            page,
            history,
            summaries,
        }))
    }

    async fn store_recompiled(
        &self,
        recompiled: Recompiled,
        now: DateTime<Utc>,
    ) -> Result<String, WikiStoreError> {
        let mut repository = WikiRepository::new(self.directory()?).lock().await?;
        let stored = self.store_locked(&mut repository, recompiled, now).await;
        settle(stored, repository.release().await)
    }
}

impl PostgresMemory {
    fn directory(&self) -> Result<std::path::PathBuf, WikiStoreError> {
        self.wiki_directory
            .clone()
            .ok_or(WikiStoreError::DirectoryMissing)
    }

    async fn store_locked(
        &self,
        repository: &mut LockedRepository,
        recompiled: Recompiled,
        now: DateTime<Utc>,
    ) -> Result<String, WikiStoreError> {
        let Recompiled {
            base,
            mut page,
            archived,
            message,
        } = recompiled;
        let slug = page.meta.slug.as_ref().to_owned();
        let mut tx = self.pool.begin().await.map_err(query_error)?;
        lock_slug(&mut tx, &slug).await?;
        if repository.read(&slug).await?.as_ref() != Some(&base) {
            return Err(WikiStoreError::InvalidPatch(format!(
                "wiki page {slug} changed while it was recompiled"
            )));
        }
        let git_sha = repository
            .recompile(&mut page, archived, &message, now)
            .await?;
        topics::store_topic(&mut tx, &page, &git_sha, now).await?;
        tx.commit().await.map_err(query_error)?;
        Ok(git_sha)
    }
}
