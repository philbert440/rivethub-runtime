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
    repository: &mut crate::wiki_writer::LockedRepository,
    summary: &WikiSummary,
    date: DateTime<Utc>,
    mut patch: WikiPatch,
    now: DateTime<Utc>,
) -> Result<(String, String), WikiStoreError> {
    let slug = canonical(tx, &patch).await?;
    if slug != patch.slug.as_ref() {
        patch.aliases.push(patch.slug);
        patch.slug = wire::gateway::wiki::WikiSlug::try_from(slug.clone())
            .map_err(|error| WikiStoreError::InvalidPatch(error.to_string()))?;
    }
    let note = patch.history_entry.as_ref().map(|entry| entry.body.clone());
    let citation = wire::wiki::PageCitation {
        summary_id: summary.id.to_string(),
        date: date.date_naive(),
        kind: wire::wiki::CitationKind::Leaf,
        note: patch
            .history_entry
            .as_ref()
            .map_or_else(|| patch.title.clone(), |entry| entry.title.clone()),
    };
    let source = wire::gateway::wiki::WikiSource {
        kind: wire::gateway::wiki::WikiSourceKind::Summary,
        ids: vec![summary.id.to_string()],
        conversation_id: summary.conversation_id.map(|id| id.to_string()),
        span: None,
    };
    let applied = repository.apply(patch, citation, source, now).await?;
    let page = applied.page;
    let related = page.see_also.as_deref().map(wire::wiki::extract_wiki_links);
    let history_count = i32::try_from(page.history.len())
        .map_err(|error| WikiStoreError::InvalidPatch(error.to_string()))?;
    let search_text = wire::wiki::build_wiki_search_text(&page);
    let current_state = page
        .summary
        .as_deref()
        .ok_or_else(|| WikiStoreError::InvalidPatch("applied page has no summary".to_owned()))?;
    sqlx::query(
        "INSERT INTO rh_wiki_topics (slug, title, aliases, tags, entities, current_state, article, related,
            search_text, history_count, last_verified_at, created_at, updated_at, git_sha)
         VALUES ($1, $2, $3, $4, $5, $6, coalesce($7, ''), coalesce($8::text[], '{}'::text[]), $9, $10, $11, $12, $12, $13)
         ON CONFLICT (slug) DO UPDATE SET title = EXCLUDED.title, aliases = EXCLUDED.aliases,
            tags = EXCLUDED.tags, entities = EXCLUDED.entities, current_state = EXCLUDED.current_state,
            article = EXCLUDED.article, related = EXCLUDED.related, search_text = EXCLUDED.search_text,
            history_count = EXCLUDED.history_count, last_verified_at = EXCLUDED.last_verified_at,
            updated_at = EXCLUDED.updated_at, git_sha = EXCLUDED.git_sha,
            embedding = NULL, embed_status = NULL, embed_failures = 0, embed_error = NULL",
    )
    .bind(&slug).bind(&page.frontmatter.title).bind(&page.frontmatter.aliases)
    .bind(&page.frontmatter.tags).bind(&page.frontmatter.entities).bind(current_state)
    .bind(page.article.as_deref()).bind(related).bind(search_text).bind(history_count)
    .bind(page.frontmatter.last_verified).bind(now).bind(&applied.git_sha)
    .execute(&mut **tx).await.map_err(query_error)?;
    redirects(tx, &slug, &page.frontmatter.aliases, now).await?;
    provenance(tx, &slug, summary, note, now).await?;
    Ok((slug, applied.git_sha))
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

pub(super) async fn canonical(
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
