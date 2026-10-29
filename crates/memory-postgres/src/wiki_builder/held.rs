use chrono::{DateTime, Utc};
use kernel::topics::Slug;
use kernel::wiki::builder::{DeferredWiki, ExtractWiki, HeldEntity, WikiPatch, WikiStoreError};
use sqlx::PgPool;
use uuid::Uuid;
use wire::gateway::wiki::WikiSlug;

use super::{load_summary, query_error, topics};

#[derive(Debug, sqlx::FromRow)]
struct HeldRow {
    key: String,
    source: String,
    summary_id: Uuid,
    patch: String,
    held_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct DeferredRow {
    summary_id: Uuid,
    session_key: String,
    patches: String,
    deferred_at: DateTime<Utc>,
}

pub(super) async fn hold(
    pool: &PgPool,
    held: HeldEntity,
) -> Result<Vec<HeldEntity>, WikiStoreError> {
    sqlx::query(
        "INSERT INTO rh_wiki_entity_candidates (key, source, summary_id, patch, held_at)
         VALUES ($1, $2, $3, $4::jsonb, $5)
         ON CONFLICT (key, source) DO UPDATE SET summary_id = EXCLUDED.summary_id,
            patch = EXCLUDED.patch, held_at = EXCLUDED.held_at",
    )
    .bind(held.key.as_str())
    .bind(&held.source)
    .bind(held.summary_id)
    .bind(encoded(&held.patch)?)
    .bind(held.at)
    .execute(pool)
    .await
    .map_err(query_error)?;
    sqlx::query_as::<_, HeldRow>(
        "SELECT key, source, summary_id, patch::text AS patch, held_at
           FROM rh_wiki_entity_candidates WHERE key = $1 ORDER BY held_at, source",
    )
    .bind(held.key.as_str())
    .fetch_all(pool)
    .await
    .map_err(query_error)?
    .into_iter()
    .map(|row| {
        Ok(HeldEntity {
            key: Slug::fold(&row.key).map_err(invalid)?,
            source: row.source,
            summary_id: row.summary_id,
            patch: serde_json::from_str(&row.patch).map_err(invalid)?,
            at: row.held_at,
        })
    })
    .collect()
}

pub(super) async fn release(pool: &PgPool, key: &Slug) -> Result<(), WikiStoreError> {
    sqlx::query("DELETE FROM rh_wiki_entity_candidates WHERE key = $1")
        .bind(key.as_str())
        .execute(pool)
        .await
        .map_err(query_error)?;
    Ok(())
}

pub(super) async fn defer(pool: &PgPool, deferred: DeferredWiki) -> Result<(), WikiStoreError> {
    sqlx::query(
        "INSERT INTO rh_wiki_deferred (summary_id, session_key, patches, deferred_at)
         VALUES ($1, $2, $3::jsonb, $4)
         ON CONFLICT (summary_id) DO UPDATE SET session_key = EXCLUDED.session_key,
            patches = EXCLUDED.patches, deferred_at = EXCLUDED.deferred_at",
    )
    .bind(deferred.summary_id)
    .bind(&deferred.session_key)
    .bind(encoded(&deferred.patches)?)
    .bind(deferred.at)
    .execute(pool)
    .await
    .map_err(query_error)?;
    Ok(())
}

pub(super) async fn filed(pool: &PgPool, limit: u32) -> Result<Vec<DeferredWiki>, WikiStoreError> {
    sqlx::query_as::<_, DeferredRow>(
        "SELECT d.summary_id, d.session_key, d.patches::text AS patches, d.deferred_at
           FROM rh_wiki_deferred d
          WHERE EXISTS (SELECT 1 FROM rh_session_placements p WHERE p.session_id = d.session_key)
          ORDER BY d.deferred_at, d.summary_id LIMIT $1",
    )
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(query_error)?
    .into_iter()
    .map(|row| {
        Ok(DeferredWiki {
            summary_id: row.summary_id,
            session_key: row.session_key,
            patches: serde_json::from_str::<Vec<WikiPatch>>(&row.patches).map_err(invalid)?,
            at: row.deferred_at,
        })
    })
    .collect()
}

pub(super) async fn cite(
    pool: &PgPool,
    summary_id: Uuid,
    slug: &WikiSlug,
    now: DateTime<Utc>,
) -> Result<(), WikiStoreError> {
    let mut tx = pool.begin().await.map_err(query_error)?;
    let job = ExtractWiki {
        summary_id,
        conversation_id: None,
    };
    let summary = load_summary(&mut tx, &job, false).await?;
    topics::cite(&mut tx, slug.as_ref(), &summary, now).await?;
    tx.commit().await.map_err(query_error)
}

pub(super) async fn forget(pool: &PgPool, summary_id: Uuid) -> Result<(), WikiStoreError> {
    sqlx::query("DELETE FROM rh_wiki_deferred WHERE summary_id = $1")
        .bind(summary_id)
        .execute(pool)
        .await
        .map_err(query_error)?;
    Ok(())
}

fn encoded<T: serde::Serialize>(value: &T) -> Result<String, WikiStoreError> {
    serde_json::to_string(value).map_err(invalid)
}

fn invalid(error: impl std::fmt::Display) -> WikiStoreError {
    WikiStoreError::InvalidPatch(error.to_string())
}
