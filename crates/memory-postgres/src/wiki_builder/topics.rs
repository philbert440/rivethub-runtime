use chrono::{DateTime, Utc};
use kernel::wiki::builder::{WikiPatch, WikiStoreError, WikiSummary};
use sqlx::{Postgres, Transaction};
use wire::gateway::wiki::{WikiSlug, WikiSourceKind};
use wire::wiki::{CitationKind, PageCitation, PagePatch, PageSource, WikiPage};

use super::{gate, lock_slug, query_error};

pub(super) struct Source<'a> {
    pub(super) summary: &'a WikiSummary,
    pub(super) date: DateTime<Utc>,
    pub(super) topic_page: Option<&'a WikiSlug>,
}

pub(super) async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    repository: &mut crate::wiki_writer::LockedRepository,
    source: &Source<'_>,
    mut patch: WikiPatch,
    now: DateTime<Utc>,
) -> Result<(String, String), WikiStoreError> {
    let Source {
        summary,
        date,
        topic_page,
    } = *source;
    let gated = gate::gate(tx, &patch, topic_page).await?;
    if gated.slug != patch.slug.as_ref() || gated.action != patch.action {
        tracing::info!(
            "extract-wiki: resolve {}/{} → {}/{} ({})",
            patch.slug.as_ref(),
            patch.action,
            gated.slug,
            gated.action,
            gated.reason
        );
    }
    lock_slug(tx, &gated.slug).await?;
    let slug = gated.slug;
    let resolved = slug != patch.slug.as_ref();
    if resolved {
        patch.aliases.push(patch.slug);
    }
    let note = patch.history_entry.as_ref().map(|entry| entry.body.clone());
    let citation_note = patch
        .history_entry
        .as_ref()
        .map_or_else(|| patch.title.clone(), |entry| entry.title.clone());
    let summary_id = summary.id.to_string();
    let page_patch = PagePatch {
        action: gated.action,
        slug: slug.clone(),
        verified_at: now,
        title: (!resolved).then_some(patch.title),
        add_aliases: patch.aliases.into_iter().map(String::from).collect(),
        add_tags: patch.tags,
        add_entities: patch.entities,
        add_related: patch
            .related
            .into_iter()
            .flatten()
            .map(String::from)
            .collect(),
        summary_delta: Some(patch.summary),
        current_state: None,
        article: patch.article,
        article_patches: Vec::new(),
        history_entry: patch.history_entry,
        add_citations: vec![PageCitation {
            summary_id: summary_id.clone(),
            date: Some(date.date_naive()),
            kind: Some(CitationKind::Leaf),
            note: Some(citation_note),
        }],
        add_sources: vec![PageSource {
            kind: WikiSourceKind::Summary,
            ids: vec![summary_id.clone()],
            conversation_id: summary.conversation_id.map(|id| id.to_string()),
            extra: wire::Extra::default(),
        }],
        allow_shrink: false,
    };
    let applied = repository.apply(page_patch, &summary_id).await?;
    store_topic(tx, &applied.page, &applied.git_sha, now).await?;
    provenance(tx, &slug, summary, note, now).await?;
    Ok((slug, applied.git_sha))
}

pub(super) async fn store_topic(
    tx: &mut Transaction<'_, Postgres>,
    page: &WikiPage,
    git_sha: &str,
    now: DateTime<Utc>,
) -> Result<(), WikiStoreError> {
    let history_count = i32::try_from(page.history.len())
        .map_err(|error| WikiStoreError::InvalidPatch(error.to_string()))?;
    let search_text = wire::wiki::build_wiki_search_text(page);
    sqlx::query(
        "INSERT INTO rh_wiki_topics (slug, title, aliases, tags, entities, current_state, article, related,
            search_text, history_count, last_verified_at, created_at, updated_at, git_sha)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12, $13)
         ON CONFLICT (slug) DO UPDATE SET title = EXCLUDED.title, aliases = EXCLUDED.aliases,
            tags = EXCLUDED.tags, entities = EXCLUDED.entities, current_state = EXCLUDED.current_state,
            article = EXCLUDED.article, related = EXCLUDED.related, search_text = EXCLUDED.search_text,
            history_count = EXCLUDED.history_count, last_verified_at = EXCLUDED.last_verified_at,
            updated_at = EXCLUDED.updated_at, git_sha = EXCLUDED.git_sha,
            embedding = NULL, embed_status = NULL, embed_failures = 0, embed_error = NULL",
    )
    .bind(page.meta.slug.as_ref()).bind(&page.meta.title).bind(&page.meta.aliases)
    .bind(&page.meta.tags).bind(&page.meta.entities).bind(&page.current_state)
    .bind(&page.article).bind(&page.meta.related).bind(search_text).bind(history_count)
    .bind(now).bind(now).bind(git_sha)
    .execute(&mut **tx).await.map_err(query_error)?;
    Ok(())
}

pub(super) async fn cite(
    tx: &mut Transaction<'_, Postgres>,
    slug: &str,
    summary: &WikiSummary,
    now: DateTime<Utc>,
) -> Result<(), WikiStoreError> {
    provenance(tx, slug, summary, None, now).await
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
