mod topics;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kernel::wiki::builder::{
    ExtractWiki, WIKI_PIPELINE_VERSION, WikiBuilderStore, WikiPatch, WikiStoreError, WikiSummary,
    WikiWork,
};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;
use wire::gateway::wiki::WikiSlug;
use wire::wiki::normalize_slug;

use crate::PostgresMemory;

#[derive(Debug, sqlx::FromRow)]
struct SummaryRow {
    id: Uuid,
    conversation_id: Option<Uuid>,
    content: String,
}

#[derive(Debug, sqlx::FromRow)]
struct CompletedRow {
    topics_touched: Vec<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct BackfillRow {
    id: Uuid,
    conversation_id: Option<Uuid>,
}

#[async_trait]
impl WikiBuilderStore for PostgresMemory {
    async fn wiki_work(&self, job: &ExtractWiki) -> Result<WikiWork, WikiStoreError> {
        let mut connection = self.pool.acquire().await.map_err(query_error)?;
        let summary = load_summary(&mut connection, job, false).await?;
        if let Some(topics) = completed(&mut connection, job.summary_id).await? {
            return Ok(WikiWork::Complete { topics });
        }
        Ok(WikiWork::Pending(summary))
    }

    async fn wiki_backfill(&self, limit: u32) -> Result<Vec<ExtractWiki>, WikiStoreError> {
        let rows = sqlx::query_as::<_, BackfillRow>(
            "SELECT s.id, s.conversation_id FROM rh_summaries s
             LEFT JOIN rh_wiki_extractions e ON e.summary_id = s.id
             WHERE e.summary_id IS NULL OR e.status = 'failed'
                OR (e.status = 'done' AND (e.pipeline_version < $1 OR (cardinality(e.topics_touched) > 0 AND e.git_sha IS NULL)))
             ORDER BY s.created_at, s.id LIMIT $2",
        )
        .bind(WIKI_PIPELINE_VERSION)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(query_error)?;
        Ok(rows
            .into_iter()
            .map(|row| ExtractWiki {
                summary_id: row.id,
                conversation_id: row.conversation_id,
            })
            .collect())
    }

    async fn apply_wiki(
        &self,
        job: &ExtractWiki,
        patches: Vec<WikiPatch>,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, WikiStoreError> {
        let directory = self
            .wiki_directory
            .as_ref()
            .ok_or(WikiStoreError::DirectoryMissing)?;
        let (applied, released) = {
            let mut repository = crate::wiki_writer::WikiRepository::new(directory.clone())
                .lock()
                .await?;
            let applied = self.apply_pages(&mut repository, job, patches, now).await;
            (applied, repository.release().await)
        };
        match (applied, released) {
            (Ok(topics), Ok(())) => Ok(topics),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
            (Err(error), Err(release)) => {
                tracing::error!(%release, "wiki writer lock release failed");
                Err(error)
            }
        }
    }

    async fn fail_wiki(
        &self,
        summary_id: Uuid,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError> {
        sqlx::query(
            "INSERT INTO rh_wiki_extractions (summary_id, status, pipeline_version, error, extracted_at)
             VALUES ($1, 'failed', $2, $3, $4)
             ON CONFLICT (summary_id) DO UPDATE SET status = 'failed', pipeline_version = EXCLUDED.pipeline_version,
                error = EXCLUDED.error, extracted_at = EXCLUDED.extracted_at
             WHERE rh_wiki_extractions.status = 'failed' OR rh_wiki_extractions.pipeline_version < $2
                OR (rh_wiki_extractions.status = 'done' AND rh_wiki_extractions.git_sha IS NULL AND cardinality(rh_wiki_extractions.topics_touched) > 0)",
        )
        .bind(summary_id).bind(WIKI_PIPELINE_VERSION).bind(error).bind(now)
        .execute(&self.pool).await.map_err(query_error)?;
        Ok(())
    }
}

impl PostgresMemory {
    async fn apply_pages(
        &self,
        repository: &mut crate::wiki_writer::LockedRepository,
        job: &ExtractWiki,
        mut patches: Vec<WikiPatch>,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, WikiStoreError> {
        for patch in &mut patches {
            if patch.title.trim().is_empty() || patch.summary.trim().is_empty() {
                return Err(WikiStoreError::InvalidPatch(
                    "title and summary must be nonempty".to_owned(),
                ));
            }
            patch.slug = WikiSlug::try_from(normalize_slug(patch.slug.as_ref()))
                .map_err(|error| WikiStoreError::InvalidPatch(error.to_string()))?;
        }
        let mut tx = self.pool.begin().await.map_err(query_error)?;
        let summary = load_summary(&mut tx, job, true).await?;
        if let Some(topics) = completed(&mut tx, job.summary_id).await? {
            tx.commit().await.map_err(query_error)?;
            return Ok(topics);
        }
        lock_slugs(&mut tx, &patches).await?;
        for patch in &patches {
            topics::canonical(&mut tx, patch).await?;
        }
        let date: DateTime<Utc> =
            sqlx::query_scalar("SELECT created_at FROM rh_summaries WHERE id = $1")
                .bind(job.summary_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(query_error)?;
        patches.sort_by(|left, right| left.slug.as_ref().cmp(right.slug.as_ref()));
        let mut touched = Vec::new();
        let mut last_sha = None;
        for patch in patches {
            let (slug, sha) =
                topics::apply(&mut tx, repository, &summary, date, patch, now).await?;
            touched.push(slug);
            last_sha = Some(sha);
        }
        touched.sort();
        touched.dedup();
        sqlx::query(
            "INSERT INTO rh_wiki_extractions (summary_id, status, pipeline_version, topics_touched, extracted_at, git_sha)
             VALUES ($1, 'done', $2, $3, $4, $5)
             ON CONFLICT (summary_id) DO UPDATE SET status = 'done', pipeline_version = EXCLUDED.pipeline_version,
                topics_touched = EXCLUDED.topics_touched, error = NULL, extracted_at = EXCLUDED.extracted_at, git_sha = EXCLUDED.git_sha",
        )
        .bind(job.summary_id).bind(WIKI_PIPELINE_VERSION).bind(&touched).bind(now).bind(last_sha)
        .execute(&mut *tx).await.map_err(query_error)?;
        tx.commit().await.map_err(query_error)?;
        Ok(touched)
    }
}

async fn load_summary(
    connection: &mut PgConnection,
    job: &ExtractWiki,
    lock: bool,
) -> Result<WikiSummary, WikiStoreError> {
    let mut query = sqlx::QueryBuilder::<Postgres>::new(
        "SELECT id, conversation_id, content FROM rh_summaries WHERE id = ",
    );
    query.push_bind(job.summary_id);
    if lock {
        query.push(" FOR UPDATE");
    }
    let row = query
        .build_query_as::<SummaryRow>()
        .fetch_optional(connection)
        .await
        .map_err(query_error)?
        .ok_or(WikiStoreError::Missing(job.summary_id))?;
    if job
        .conversation_id
        .is_some_and(|id| Some(id) != row.conversation_id)
    {
        return Err(WikiStoreError::ConversationMismatch(job.summary_id));
    }
    Ok(WikiSummary {
        id: row.id,
        conversation_id: row.conversation_id,
        content: row.content,
    })
}

async fn completed(
    connection: &mut PgConnection,
    summary_id: Uuid,
) -> Result<Option<Vec<String>>, WikiStoreError> {
    let row = sqlx::query_as::<_, CompletedRow>(
        "SELECT topics_touched FROM rh_wiki_extractions WHERE summary_id = $1
         AND status IN ('done', 'skipped') AND pipeline_version >= $2
         AND (status = 'skipped' OR cardinality(topics_touched) = 0 OR git_sha IS NOT NULL)",
    )
    .bind(summary_id)
    .bind(WIKI_PIPELINE_VERSION)
    .fetch_optional(connection)
    .await
    .map_err(query_error)?;
    Ok(row.map(|row| row.topics_touched))
}

async fn lock_slugs(
    tx: &mut Transaction<'_, Postgres>,
    patches: &[WikiPatch],
) -> Result<(), WikiStoreError> {
    let mut slugs: Vec<&str> = patches
        .iter()
        .flat_map(|patch| {
            std::iter::once(patch.slug.as_ref()).chain(patch.aliases.iter().map(AsRef::as_ref))
        })
        .collect();
    slugs.sort_unstable();
    slugs.dedup();
    for slug in slugs {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(slug)
            .execute(&mut **tx)
            .await
            .map_err(query_error)?;
    }
    Ok(())
}

fn query_error(error: sqlx::Error) -> WikiStoreError {
    WikiStoreError::Query(Box::new(error))
}
