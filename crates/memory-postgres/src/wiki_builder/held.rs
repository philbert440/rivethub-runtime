use chrono::{DateTime, Utc};
use kernel::filing::Target;
use kernel::topics::Slug;
use kernel::topics::TopicId;
use kernel::wiki::builder::{DeferredWiki, ExtractWiki, HeldEntity, WikiPatch, WikiStoreError};
use sqlx::PgPool;
use uuid::Uuid;
use wire::gateway::wiki::WikiSlug;
use wire::session_id::SessionId;

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
    waits_session: Option<String>,
    waits_conversation: Option<Uuid>,
    applied_topic: Option<Uuid>,
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
        "INSERT INTO rh_wiki_deferred
             (summary_id, waits_session, waits_conversation, applied_topic, patches, deferred_at)
         VALUES ($1, $2, $3, $4, $5::jsonb, $6)
         ON CONFLICT (summary_id) DO UPDATE SET waits_session = EXCLUDED.waits_session,
            waits_conversation = EXCLUDED.waits_conversation, applied_topic = EXCLUDED.applied_topic,
            patches = EXCLUDED.patches, deferred_at = EXCLUDED.deferred_at",
    )
    .bind(deferred.summary_id)
    .bind(deferred.waits.session().map(SessionId::as_str))
    .bind(deferred.waits.conversation())
    .bind(deferred.applied.map(TopicId::uuid))
    .bind(encoded(&deferred.patches)?)
    .bind(deferred.at)
    .execute(pool)
    .await
    .map_err(query_error)?;
    Ok(())
}

pub(super) async fn unsettled(
    pool: &PgPool,
    limit: u32,
) -> Result<Vec<DeferredWiki>, WikiStoreError> {
    sqlx::query_as::<_, DeferredRow>(
        "SELECT d.summary_id, d.waits_session, d.waits_conversation, d.applied_topic,
                d.patches::text AS patches, d.deferred_at
           FROM rh_wiki_deferred d
           LEFT JOIN rh_session_placements p ON p.session_id = d.waits_session
           LEFT JOIN rh_conversation_placements c ON c.conversation_id = d.waits_conversation
          WHERE COALESCE(p.topic_id, c.topic_id) IS DISTINCT FROM d.applied_topic
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
            waits: Target::stored(row.waits_session.as_deref(), row.waits_conversation)
                .ok_or_else(|| {
                    invalid("a deferral waits on neither a session nor a conversation")
                })?,
            applied: row.applied_topic.map(TopicId::from_uuid),
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

fn encoded<T: serde::Serialize>(value: &T) -> Result<String, WikiStoreError> {
    serde_json::to_string(value).map_err(invalid)
}

fn invalid(error: impl std::fmt::Display) -> WikiStoreError {
    WikiStoreError::InvalidPatch(error.to_string())
}
