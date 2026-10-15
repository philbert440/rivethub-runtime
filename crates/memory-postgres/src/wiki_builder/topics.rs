use chrono::{DateTime, Utc};
use kernel::wiki::builder::{WikiAction, WikiPatch, WikiStoreError, WikiSummary};
use sqlx::{Postgres, Transaction};

use super::query_error;

#[derive(Debug)]
struct SlugRow {
    slug: String,
}

#[derive(Debug)]
struct OccupiedRow {
    occupied: bool,
}

pub(super) async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    summary: &WikiSummary,
    patch: WikiPatch,
    now: DateTime<Utc>,
) -> Result<Option<String>, WikiStoreError> {
    let slug = canonical(tx, &patch).await?;
    let mut aliases: Vec<String> = patch
        .aliases
        .iter()
        .map(|alias| alias.as_ref().to_owned())
        .chain((slug != patch.slug.as_ref()).then(|| patch.slug.as_ref().to_owned()))
        .filter(|alias| alias != &slug)
        .collect();
    aliases.sort();
    aliases.dedup();
    let related = patch.related.as_ref().map(|slugs| {
        slugs
            .iter()
            .map(|slug| slug.as_ref().to_owned())
            .collect::<Vec<_>>()
    });
    let search_text = format!(
        "{}\n{}\n{}\n{}\n{}",
        patch.title,
        patch.summary,
        aliases.join(" "),
        patch.tags.join(" "),
        patch.entities.join(" ")
    );
    let changed = sqlx::query_as!(SlugRow,
        "INSERT INTO rh_wiki_topics (slug, title, aliases, tags, entities, current_state, article, related,
            search_text, history_count, last_verified_at, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, coalesce($7, ''), coalesce($8::text[], '{}'::text[]), concat_ws(E'\n', $9::text, $7), $10, $11, $11, $11)
         ON CONFLICT (slug) DO UPDATE SET title = EXCLUDED.title, aliases = EXCLUDED.aliases,
            tags = EXCLUDED.tags, entities = EXCLUDED.entities, current_state = EXCLUDED.current_state,
            article = coalesce($7, rh_wiki_topics.article), related = coalesce($8, rh_wiki_topics.related),
            search_text = concat_ws(E'\n', $9::text, coalesce($7, rh_wiki_topics.article)), history_count = rh_wiki_topics.history_count + $10,
            last_verified_at = $11, updated_at = $11, embedding = NULL, embed_status = NULL,
            embed_failures = 0, embed_error = NULL
         WHERE (rh_wiki_topics.title, rh_wiki_topics.aliases, rh_wiki_topics.tags, rh_wiki_topics.entities,
                rh_wiki_topics.current_state, rh_wiki_topics.article, rh_wiki_topics.related)
           IS DISTINCT FROM (EXCLUDED.title, EXCLUDED.aliases, EXCLUDED.tags, EXCLUDED.entities,
                EXCLUDED.current_state, coalesce($7, rh_wiki_topics.article), coalesce($8, rh_wiki_topics.related))
         RETURNING slug",
        &slug, &patch.title, &aliases, &patch.tags, &patch.entities,
        &patch.summary, patch.article.as_deref(), related.as_deref(), search_text,
        i32::from(patch.history_entry.is_some()), now,
    )
    .fetch_optional(&mut **tx).await.map_err(query_error)?;
    redirects(tx, &slug, &aliases, now).await?;
    provenance(
        tx,
        &slug,
        summary,
        patch.history_entry.map(|entry| entry.body),
        now,
    )
    .await?;
    Ok(changed.map(|row| row.slug))
}

async fn redirects(
    tx: &mut Transaction<'_, Postgres>,
    slug: &str,
    aliases: &[String],
    now: DateTime<Utc>,
) -> Result<(), WikiStoreError> {
    for alias in aliases {
        let occupied = sqlx::query_as!(
            OccupiedRow,
            "SELECT EXISTS(SELECT 1 FROM rh_wiki_topics WHERE slug = $1) AS \"occupied!\"",
            alias
        )
        .fetch_one(&mut **tx)
        .await
        .map_err(query_error)?;
        if occupied.occupied {
            return Err(WikiStoreError::SlugConflict(alias.clone()));
        }
        let inserted = sqlx::query(
            "INSERT INTO rh_wiki_redirects (from_slug, to_slug, created_at) VALUES ($1, $2, $3)
             ON CONFLICT (from_slug) DO UPDATE SET to_slug = EXCLUDED.to_slug
             WHERE rh_wiki_redirects.to_slug = EXCLUDED.to_slug",
        )
        .bind(alias)
        .bind(slug)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(query_error)?;
        if inserted.rows_affected() != 1 {
            return Err(WikiStoreError::SlugConflict(alias.clone()));
        }
    }
    Ok(())
}

async fn provenance(
    tx: &mut Transaction<'_, Postgres>,
    slug: &str,
    summary: &WikiSummary,
    note: Option<String>,
    now: DateTime<Utc>,
) -> Result<(), WikiStoreError> {
    sqlx::query(
        "INSERT INTO rh_wiki_provenance (topic_slug, source_kind, source_id, conversation_id, created_at)
         VALUES ($1, 'summary', $2, $3, $4) ON CONFLICT DO NOTHING",
    ).bind(slug).bind(summary.id).bind(summary.conversation_id).bind(now)
        .execute(&mut **tx).await.map_err(query_error)?;
    sqlx::query(
        "INSERT INTO rh_wiki_citations (topic_slug, summary_id, kind, note, cited_at)
         VALUES ($1, $2, 'summary', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(slug)
    .bind(summary.id)
    .bind(note)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(query_error)?;
    Ok(())
}

async fn canonical(
    tx: &mut Transaction<'_, Postgres>,
    patch: &WikiPatch,
) -> Result<String, WikiStoreError> {
    let candidates: Vec<String> = std::iter::once(patch.slug.as_ref().to_owned())
        .chain(patch.aliases.iter().map(|alias| alias.as_ref().to_owned()))
        .collect();
    let existing = sqlx::query_as!(
        SlugRow,
        "SELECT slug AS \"slug!\" FROM rh_wiki_topics WHERE slug = ANY($1) OR aliases && $1
         UNION SELECT to_slug FROM rh_wiki_redirects WHERE from_slug = ANY($1)",
        &candidates,
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(query_error)?;
    match existing.as_slice() {
        [] if patch.action == WikiAction::Create => Ok(patch.slug.as_ref().to_owned()),
        [] => Err(WikiStoreError::TopicMissing(patch.slug.as_ref().to_owned())),
        [row] => Ok(row.slug.clone()),
        _ => Err(WikiStoreError::SlugConflict(patch.slug.as_ref().to_owned())),
    }
}
