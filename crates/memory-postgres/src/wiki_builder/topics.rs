use chrono::{DateTime, Utc};
use kernel::owner::Owner;
use kernel::wiki::builder::{WikiStoreError, WikiSummary};
use kernel::wiki::merge::MergedPage;
use kernel::wiki::page::WikiPatch;
use kernel::wiki::page::bound::MergeBase;
use kernel::wiki::page::{CitationKind, PageCitation, PagePatch, PageSource, WikiPage};
use kernel::wiki::page::{WikiSlug, WikiSourceKind};
use sqlx::{Postgres, Transaction};

use super::{gate, lock_slug, query_error};
use crate::wiki_scope::WikiScope;

pub(super) struct Source<'a> {
    pub(super) summary: &'a WikiSummary,
    pub(super) date: DateTime<Utc>,
    pub(super) topic_page: Option<&'a WikiSlug>,
    pub(super) merged: &'a [MergedPage],
    pub(super) home: &'a WikiScope,
}

pub(super) async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    repository: &mut crate::wiki_writer::LockedRepository,
    source: &Source<'_>,
    patch: WikiPatch,
    now: DateTime<Utc>,
) -> Result<(String, String), WikiStoreError> {
    let Source {
        summary,
        date,
        topic_page,
        home,
        ..
    } = *source;
    let gated = gate::gate(tx, &patch, topic_page, home).await?;
    resolved(&patch, &gated);
    let key = crate::wiki_scope::page_key(tx, home, &gated.slug).await?;
    lock_slug(tx, &key).await?;
    let slug = gated.slug;
    let note_entry = (slug != patch.slug.as_ref()).then(|| patch.note(date.date_naive()));
    let citation_note = patch
        .history_entry
        .as_ref()
        .map_or_else(|| patch.title.clone(), |entry| entry.title.clone());
    let summary_id = summary.id.to_string();
    let mut page_patch = PagePatch {
        action: gated.action,
        slug: slug.clone(),
        verified_at: now,
        title: Some(patch.title),
        add_aliases: patch
            .aliases
            .iter()
            .map(|alias| alias.as_ref().to_owned())
            .collect(),
        add_tags: patch.tags.clone(),
        add_entities: patch.entities.clone(),
        add_related: patch
            .related
            .iter()
            .flatten()
            .map(|slug| slug.as_ref().to_owned())
            .collect(),
        summary_delta: Some(patch.summary.clone()),
        current_state: None,
        article: patch.article.clone(),
        article_patches: Vec::new(),
        history_entry: patch.history_entry.clone(),
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
    let merge = merge_base(source.merged, &key, note_entry.is_some(), &mut page_patch);
    if let Some(entry) = note_entry {
        page_patch.title = None;
        page_patch.add_aliases = vec![patch.slug.as_ref().to_owned()];
        page_patch.add_tags.clear();
        page_patch.add_entities.clear();
        page_patch.summary_delta = None;
        page_patch.article = None;
        page_patch.history_entry = Some(entry);
    }
    let note = page_patch
        .history_entry
        .as_ref()
        .map(|entry| entry.body.clone());
    let applied = repository.apply(page_patch, &summary_id, merge).await?;
    store(tx, &applied.page, &key, Some(home), &applied.git_sha, now).await?;
    provenance(tx, &key, summary, note, now).await?;
    Ok((key, applied.git_sha))
}

fn resolved(patch: &WikiPatch, gated: &gate::Gated) {
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
}

fn merge_base<'a>(
    merged: &'a [MergedPage],
    slug: &str,
    note: bool,
    patch: &mut PagePatch,
) -> Option<&'a MergeBase> {
    let base = merged
        .iter()
        .find(|page| !note && page.slug.as_ref() == slug)
        .map(|page| &page.base)?;
    patch.current_state = patch.summary_delta.take();
    patch.allow_shrink = true;
    Some(base)
}

pub(super) async fn store_topic(
    tx: &mut Transaction<'_, Postgres>,
    page: &WikiPage,
    git_sha: &str,
    now: DateTime<Utc>,
) -> Result<(), WikiStoreError> {
    store(tx, page, page.meta.slug.as_ref(), None, git_sha, now).await
}

pub(super) async fn store(
    tx: &mut Transaction<'_, Postgres>,
    page: &WikiPage,
    key: &str,
    home: Option<&WikiScope>,
    git_sha: &str,
    now: DateTime<Utc>,
) -> Result<(), WikiStoreError> {
    let history_count = i32::try_from(page.history.len())
        .map_err(|error| WikiStoreError::InvalidPatch(error.to_string()))?;
    let search_text = kernel::wiki::page::build_wiki_search_text(page);
    let owner = home.map(|home| {
        if home.visibility == "private" {
            home.owner_user_id.as_str()
        } else {
            home.scope.as_str()
        }
    });
    sqlx::query(
        "INSERT INTO rh_wiki_topics (slug, title, aliases, tags, entities, current_state, article, related,
            search_text, history_count, last_verified_at, created_at, updated_at, git_sha,
            scope, local_slug, owner_user_id, visibility)
         VALUES ($1, $2, $3, $4, $5, $6, $7,
            ARRAY(SELECT coalesce((SELECT k.slug FROM rh_wiki_topics k WHERE k.scope = $14 AND k.local_slug = r.slug), r.slug)
                    FROM unnest($8::text[]) WITH ORDINALITY AS r(slug, n) ORDER BY r.n),
            $9, $10, $11, $12, $12, $13, $14, $15, $16, coalesce($17, 'private'))
         ON CONFLICT (slug) DO UPDATE SET title = EXCLUDED.title, aliases = EXCLUDED.aliases,
            tags = EXCLUDED.tags, entities = EXCLUDED.entities, current_state = EXCLUDED.current_state,
            article = EXCLUDED.article, related = EXCLUDED.related, search_text = EXCLUDED.search_text,
            history_count = EXCLUDED.history_count, last_verified_at = EXCLUDED.last_verified_at,
            updated_at = EXCLUDED.updated_at, git_sha = EXCLUDED.git_sha,
            embedding = NULL, embed_status = NULL, embed_failures = 0, embed_error = NULL",
    )
    .bind(key).bind(&page.meta.title).bind(&page.meta.aliases)
    .bind(&page.meta.tags).bind(&page.meta.entities).bind(&page.current_state)
    .bind(&page.article).bind(&page.meta.related).bind(search_text).bind(history_count)
    .bind(now).bind(now).bind(git_sha)
    .bind(home.map(|home| home.scope.as_str()))
    .bind(home.map(|_| page.meta.slug.as_ref()))
    .bind(owner)
    .bind(home.map(|home| home.visibility.as_str()))
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
        "INSERT INTO rh_wiki_provenance
             (topic_slug, source_kind, source_id, conversation_id, created_at, owner_user_id)
         VALUES ($1, 'summary', $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(slug)
    .bind(summary.id)
    .bind(summary.conversation_id)
    .bind(now)
    .bind(summary.owner.as_ref().map(Owner::as_str))
    .execute(&mut **tx)
    .await
    .map_err(query_error)?;
    sqlx::query(
        "INSERT INTO rh_wiki_citations (topic_slug, summary_id, kind, note, cited_at, owner_user_id)
         VALUES ($1, $2, 'summary', $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(slug)
    .bind(summary.id)
    .bind(note)
    .bind(now)
    .bind(summary.owner.as_ref().map(Owner::as_str))
    .execute(&mut **tx)
    .await
    .map_err(query_error)?;
    Ok(())
}
