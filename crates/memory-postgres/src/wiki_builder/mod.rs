mod consolidate;
mod gate;
mod held;
mod topic_pages;
mod topics;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kernel::owner::Owner;
use kernel::topics::Slug;
use kernel::wiki::builder::{
    DeferredWiki, ExtractWiki, HeldEntity, WIKI_PIPELINE_VERSION, WikiBuilderStore, WikiRouting,
    WikiStoreError, WikiSummary, WikiWork, WikiWriteError,
};
use kernel::wiki::page::WikiPatch;
use kernel::wiki::page::WikiSlug;
use kernel::wiki::page::normalize_slug;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::PostgresMemory;

#[derive(Debug, sqlx::FromRow)]
struct SummaryRow {
    id: Uuid,
    conversation_id: Option<Uuid>,
    content: String,
    session_key: Option<String>,
    owner_user_id: String,
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
        routing: &WikiRouting,
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
            let applied = self
                .apply_pages(&mut repository, job, patches, routing, now)
                .await;
            (applied, repository.release().await)
        };
        settle(applied, released)
    }

    async fn fail_wiki(
        &self,
        summary_id: Uuid,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError> {
        sqlx::query(
            "INSERT INTO rh_wiki_extractions (summary_id, status, pipeline_version, error, extracted_at, owner_user_id)
             VALUES ($1, 'failed', $2, $3, $4, (SELECT owner_user_id FROM rh_summaries WHERE id = $1))
             ON CONFLICT (summary_id) DO UPDATE SET status = 'failed', pipeline_version = EXCLUDED.pipeline_version,
                error = EXCLUDED.error, extracted_at = EXCLUDED.extracted_at
             WHERE rh_wiki_extractions.status = 'failed' OR rh_wiki_extractions.pipeline_version < $2
                OR (rh_wiki_extractions.status = 'done' AND rh_wiki_extractions.git_sha IS NULL AND cardinality(rh_wiki_extractions.topics_touched) > 0)",
        )
        .bind(summary_id).bind(WIKI_PIPELINE_VERSION).bind(error).bind(now)
        .execute(&self.pool).await.map_err(query_error)?;
        Ok(())
    }

    async fn hold_entity(&self, held: HeldEntity) -> Result<Vec<HeldEntity>, WikiStoreError> {
        held::hold(&self.pool, held).await
    }

    async fn held_entities(&self, key: &Slug) -> Result<Vec<HeldEntity>, WikiStoreError> {
        held::candidates(&self.pool, key).await
    }

    async fn release_entity(&self, held: &[HeldEntity]) -> Result<(), WikiStoreError> {
        held::release(&self.pool, held).await
    }

    async fn defer_wiki(&self, deferred: DeferredWiki) -> Result<(), WikiStoreError> {
        held::defer(&self.pool, deferred).await
    }

    async fn unsettled_notes(&self, limit: u32) -> Result<Vec<DeferredWiki>, WikiStoreError> {
        held::unsettled(&self.pool, limit).await
    }

    async fn cite_wiki(
        &self,
        summary_id: Uuid,
        slug: &WikiSlug,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError> {
        held::cite(&self.pool, summary_id, slug, now).await
    }

    async fn uncite_wiki(&self, summary_id: Uuid, slug: &WikiSlug) -> Result<(), WikiStoreError> {
        held::uncite(&self.pool, summary_id, slug).await
    }
}

impl PostgresMemory {
    async fn apply_pages(
        &self,
        repository: &mut crate::wiki_writer::LockedRepository,
        job: &ExtractWiki,
        mut patches: Vec<WikiPatch>,
        routing: &WikiRouting,
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
        lock_slugs(&mut tx, &patches, &routing.cited).await?;
        let date: DateTime<Utc> =
            sqlx::query_scalar("SELECT created_at FROM rh_summaries WHERE id = $1")
                .bind(job.summary_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(query_error)?;
        let source = topics::Source {
            summary: &summary,
            date,
            topic_page: routing.topic_page.as_ref(),
        };
        let mut touched = Vec::new();
        let mut last_sha = None;
        for patch in patches {
            let (slug, sha) = topics::apply(&mut tx, repository, &source, patch, now).await?;
            touched.push(slug);
            last_sha = Some(sha);
        }
        for slug in &routing.cited {
            topics::cite(&mut tx, slug.as_ref(), &summary, now).await?;
        }
        touched.sort();
        touched.dedup();
        sqlx::query(
            "INSERT INTO rh_wiki_extractions (summary_id, status, pipeline_version, topics_touched, extracted_at, git_sha, owner_user_id)
             VALUES ($1, 'done', $2, $3, $4, $5, $6)
             ON CONFLICT (summary_id) DO UPDATE SET status = 'done', pipeline_version = EXCLUDED.pipeline_version,
                topics_touched = EXCLUDED.topics_touched, error = NULL, extracted_at = EXCLUDED.extracted_at, git_sha = EXCLUDED.git_sha",
        )
        .bind(job.summary_id).bind(WIKI_PIPELINE_VERSION).bind(&touched).bind(now).bind(last_sha)
        .bind(summary.owner.as_ref().map(Owner::as_str))
        .execute(&mut *tx).await.map_err(query_error)?;
        tx.commit().await.map_err(query_error)?;
        Ok(touched)
    }
}

fn settle<T>(
    applied: Result<T, WikiStoreError>,
    released: Result<(), WikiWriteError>,
) -> Result<T, WikiStoreError> {
    match (applied, released) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(error), Err(release)) => {
            tracing::error!(%release, "wiki writer lock release failed");
            Err(error)
        }
    }
}

async fn load_summary(
    connection: &mut PgConnection,
    job: &ExtractWiki,
    lock: bool,
) -> Result<WikiSummary, WikiStoreError> {
    let mut query = sqlx::QueryBuilder::<Postgres>::new(
        "SELECT s.id, s.conversation_id, s.content, c.session_key, s.owner_user_id FROM rh_summaries s
         LEFT JOIN rh_conversations c ON c.id = s.conversation_id WHERE s.id = ",
    );
    query.push_bind(job.summary_id);
    if lock {
        query.push(" FOR UPDATE OF s");
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
        session_key: row.session_key,
        owner: Some(
            Owner::new(row.owner_user_id)
                .map_err(|error| WikiStoreError::Query(Box::new(error)))?,
        ),
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
    cited: &[WikiSlug],
) -> Result<(), WikiStoreError> {
    let mut slugs: Vec<&str> = patches
        .iter()
        .flat_map(|patch| {
            std::iter::once(patch.slug.as_ref()).chain(patch.aliases.iter().map(AsRef::as_ref))
        })
        .chain(cited.iter().map(AsRef::as_ref))
        .collect();
    slugs.sort_unstable();
    slugs.dedup();
    for slug in slugs {
        lock_slug(tx, slug).await?;
    }
    Ok(())
}

async fn lock_slug(tx: &mut Transaction<'_, Postgres>, slug: &str) -> Result<(), WikiStoreError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(slug)
        .execute(&mut **tx)
        .await
        .map_err(query_error)?;
    Ok(())
}

fn query_error(error: sqlx::Error) -> WikiStoreError {
    WikiStoreError::Query(Box::new(error))
}
