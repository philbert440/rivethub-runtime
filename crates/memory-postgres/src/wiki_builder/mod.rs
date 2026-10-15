mod topics;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kernel::wiki::builder::{
    ExtractWiki, WIKI_PIPELINE_VERSION, WikiBuilderStore, WikiPatch, WikiStoreError, WikiSummary,
    WikiWork,
};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

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

#[derive(Debug)]
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
        let rows = sqlx::query_as!(
            BackfillRow,
            "SELECT s.id, s.conversation_id FROM rh_summaries s
             LEFT JOIN rh_wiki_extractions e ON e.summary_id = s.id
             WHERE e.summary_id IS NULL OR e.status = 'failed'
                OR (e.status = 'done' AND e.pipeline_version < $1)
             ORDER BY s.created_at, s.id LIMIT $2",
            WIKI_PIPELINE_VERSION,
            i64::from(limit),
        )
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
        mut patches: Vec<WikiPatch>,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, WikiStoreError> {
        for patch in &patches {
            if patch.title.trim().is_empty() || patch.summary.trim().is_empty() {
                return Err(WikiStoreError::InvalidPatch(
                    "title and summary must be nonempty".to_owned(),
                ));
            }
        }
        let mut tx = self.pool.begin().await.map_err(query_error)?;
        let summary = load_summary(&mut tx, job, true).await?;
        if let Some(topics) = completed(&mut tx, job.summary_id).await? {
            tx.commit().await.map_err(query_error)?;
            return Ok(topics);
        }
        lock_slugs(&mut tx, &patches).await?;
        patches.sort_by(|left, right| left.slug.as_ref().cmp(right.slug.as_ref()));
        let mut touched = Vec::new();
        for patch in patches {
            if let Some(slug) = topics::apply(&mut tx, &summary, patch, now).await? {
                touched.push(slug);
            }
        }
        touched.sort();
        touched.dedup();
        sqlx::query(
            "INSERT INTO rh_wiki_extractions (summary_id, status, pipeline_version, topics_touched, extracted_at)
             VALUES ($1, 'done', $2, $3, $4)
             ON CONFLICT (summary_id) DO UPDATE SET status = 'done', pipeline_version = EXCLUDED.pipeline_version,
                topics_touched = EXCLUDED.topics_touched, error = NULL, extracted_at = EXCLUDED.extracted_at",
        )
        .bind(job.summary_id).bind(WIKI_PIPELINE_VERSION).bind(&touched).bind(now)
        .execute(&mut *tx).await.map_err(query_error)?;
        tx.commit().await.map_err(query_error)?;
        Ok(touched)
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
             WHERE rh_wiki_extractions.status = 'failed' OR rh_wiki_extractions.pipeline_version < $2",
        )
        .bind(summary_id).bind(WIKI_PIPELINE_VERSION).bind(error).bind(now)
        .execute(&self.pool).await.map_err(query_error)?;
        Ok(())
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
    let row = sqlx::query_as!(
        CompletedRow,
        "SELECT topics_touched FROM rh_wiki_extractions WHERE summary_id = $1
         AND status IN ('done', 'skipped') AND pipeline_version >= $2",
        summary_id,
        WIKI_PIPELINE_VERSION,
    )
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
