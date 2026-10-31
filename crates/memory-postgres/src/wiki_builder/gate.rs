use kernel::wiki::builder::WikiStoreError;
use kernel::wiki::page::WikiSlug;
use kernel::wiki::page::{WikiAction, WikiPatch};
use kernel::wiki::page::{find_stem_match, normalize_slug, topic_identity};
use sqlx::{Postgres, Transaction};

use super::query_error;
use crate::wiki_scope::WikiScope;
const RRF_K: f64 = 20.0;
const RESOLVE_SEARCH_SCORE_MIN: f64 = 0.06;

const SEARCH_LIMIT: i64 = 15;

const SEARCH_QUERY_LIMIT: usize = 400;

const REDIRECT_HOPS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolveReason {
    Topic,
    Exact,
    Redirect,
    Alias,
    Identity,
    Stem,
    Search,
    None,
}

impl std::fmt::Display for ResolveReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Topic => "topic",
            Self::Exact => "exact",
            Self::Redirect => "redirect",
            Self::Alias => "alias",
            Self::Identity => "identity",
            Self::Stem => "stem",
            Self::Search => "search",
            Self::None => "none",
        })
    }
}

#[derive(Debug)]
pub(super) struct Gated {
    pub(super) slug: String,
    pub(super) action: WikiAction,
    pub(super) reason: ResolveReason,
}
pub(super) async fn gate(
    tx: &mut Transaction<'_, Postgres>,
    patch: &WikiPatch,
    page: Option<&WikiSlug>,
    home: &WikiScope,
) -> Result<Gated, WikiStoreError> {
    let proposed = patch.slug.as_ref();
    if let Some(page) = page.filter(|page| page.as_ref() == normalize_slug(proposed)) {
        return Ok(Gated {
            slug: page.as_ref().to_owned(),
            action: WikiAction::Update,
            reason: ResolveReason::Topic,
        });
    }
    let page = page.map(AsRef::as_ref);
    Ok(
        match resolve(tx, proposed, &patch.title, &patch.entities, page, home).await? {
            Some((slug, reason)) => Gated {
                slug,
                action: WikiAction::Update,
                reason,
            },
            None => Gated {
                slug: normalize_slug(proposed),
                action: WikiAction::Create,
                reason: ResolveReason::None,
            },
        },
    )
}
async fn resolve(
    tx: &mut Transaction<'_, Postgres>,
    proposed: &str,
    title: &str,
    entities: &[String],
    page: Option<&str>,
    home: &WikiScope,
) -> Result<Option<(String, ResolveReason)>, WikiStoreError> {
    let slug = normalize_slug(proposed);
    if let Some(found) = topic(tx, &slug, home).await? {
        return Ok(Some((found, ResolveReason::Exact)));
    }
    if home.primary() {
        let target = follow_redirect(tx, &slug).await?;
        if target != slug
            && let Some(found) = topic(tx, &target, home).await?
        {
            return Ok(Some((found, ResolveReason::Redirect)));
        }
    }
    let alias = sqlx::query_scalar::<_, String>(
        "SELECT coalesce(local_slug, slug) FROM rh_wiki_topics
          WHERE $1 = ANY(aliases) AND (scope = $2 OR ($3 AND scope IS NULL)) ORDER BY 1 LIMIT 1",
    )
    .bind(&slug)
    .bind(&home.scope)
    .bind(home.primary())
    .fetch_optional(&mut **tx)
    .await
    .map_err(query_error)?;
    if let Some(found) = alias
        && open(tx, &found, page, home).await?
    {
        return Ok(Some((found, ResolveReason::Alias)));
    }
    if let Some(found) = same_identity(tx, &slug, title, home).await?
        && open(tx, &found, page, home).await?
    {
        return Ok(Some((found, ResolveReason::Identity)));
    }
    let variants = sqlx::query_scalar::<_, String>(
        "SELECT local FROM (SELECT coalesce(local_slug, slug) AS local FROM rh_wiki_topics
                             WHERE scope = $2 OR ($3 AND scope IS NULL)) pages
         WHERE local = $1 OR local LIKE $1 || '-%' OR $1 LIKE local || '-%'
         ORDER BY length(local), local LIMIT 50",
    )
    .bind(&slug)
    .bind(&home.scope)
    .bind(home.primary())
    .fetch_all(&mut **tx)
    .await
    .map_err(query_error)?;
    if let Some(stem) = find_stem_match(&slug, &variants)
        && let Some(found) = topic(tx, &stem, home).await?
        && open(tx, &found, page, home).await?
    {
        return Ok(Some((found, ResolveReason::Stem)));
    }
    let query = search_query(proposed, title, entities);
    if let Some((top, score)) = search_top(tx, &query, home).await?
        && score >= RESOLVE_SEARCH_SCORE_MIN
        && (find_stem_match(&slug, &[&top]).as_deref() == Some(top.as_str())
            || score >= RESOLVE_SEARCH_SCORE_MIN * 1.5)
        && open(tx, &top, page, home).await?
    {
        return Ok(Some((top, ResolveReason::Search)));
    }
    Ok(None)
}

#[derive(Debug, sqlx::FromRow)]
struct NamedTopic {
    slug: String,
    title: String,
}

async fn same_identity(
    tx: &mut Transaction<'_, Postgres>,
    slug: &str,
    title: &str,
    home: &WikiScope,
) -> Result<Option<String>, WikiStoreError> {
    let identity = topic_identity(slug, title);
    let Some(probe) = identity.probe() else {
        return Ok(None);
    };
    let candidates = sqlx::query_as::<_, NamedTopic>(
        "SELECT coalesce(local_slug, slug) AS slug, title FROM rh_wiki_topics
          WHERE strpos(coalesce(local_slug, slug), $1) > 0 AND (scope = $2 OR ($3 AND scope IS NULL))",
    )
    .bind(probe)
    .bind(&home.scope)
    .bind(home.primary())
    .fetch_all(&mut **tx)
    .await
    .map_err(query_error)?;
    Ok(identity.closest(
        candidates
            .into_iter()
            .map(|topic| (topic.slug, topic.title)),
    ))
}

async fn topic(
    tx: &mut Transaction<'_, Postgres>,
    slug: &str,
    home: &WikiScope,
) -> Result<Option<String>, WikiStoreError> {
    sqlx::query_scalar::<_, String>(
        "SELECT coalesce(local_slug, slug) FROM rh_wiki_topics
          WHERE coalesce(local_slug, slug) = $1 AND (scope = $2 OR ($3 AND scope IS NULL))",
    )
    .bind(slug)
    .bind(&home.scope)
    .bind(home.primary())
    .fetch_optional(&mut **tx)
    .await
    .map_err(query_error)
}

async fn open(
    tx: &mut Transaction<'_, Postgres>,
    slug: &str,
    page: Option<&str>,
    home: &WikiScope,
) -> Result<bool, WikiStoreError> {
    if page == Some(slug) {
        return Ok(true);
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS (SELECT 1 FROM rh_wiki_topics WHERE coalesce(local_slug, slug) = $1
            AND (scope = $2 OR ($3 AND scope IS NULL)) AND topic_id IS NOT NULL)",
    )
    .bind(slug)
    .bind(&home.scope)
    .bind(home.primary())
    .fetch_one(&mut **tx)
    .await
    .map_err(query_error)
}

async fn follow_redirect(
    tx: &mut Transaction<'_, Postgres>,
    slug: &str,
) -> Result<String, WikiStoreError> {
    let mut current = slug.to_owned();
    for _ in 0..REDIRECT_HOPS {
        let next = sqlx::query_scalar::<_, String>(
            "SELECT to_slug FROM rh_wiki_redirects WHERE from_slug = $1",
        )
        .bind(&current)
        .fetch_optional(&mut **tx)
        .await
        .map_err(query_error)?;
        match next {
            Some(next) => current = next,
            None => break,
        }
    }
    Ok(current)
}
fn search_query(proposed: &str, title: &str, entities: &[String]) -> String {
    let joined = [proposed, title]
        .into_iter()
        .chain(entities.iter().map(String::as_str))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().take(SEARCH_QUERY_LIMIT).collect()
}
async fn search_top(
    tx: &mut Transaction<'_, Postgres>,
    query: &str,
    home: &WikiScope,
) -> Result<Option<(String, f64)>, WikiStoreError> {
    let full_text = sqlx::query_scalar::<_, String>(
        "SELECT coalesce(local_slug, slug) FROM rh_wiki_topics
          WHERE content_tsv @@ websearch_to_tsquery('english', $1) AND (scope = $3 OR ($4 AND scope IS NULL))
         ORDER BY ts_rank(content_tsv, websearch_to_tsquery('english', $1)) DESC, slug LIMIT $2",
    )
    .bind(query)
    .bind(SEARCH_LIMIT)
    .bind(&home.scope)
    .bind(home.primary())
    .fetch_all(&mut **tx)
    .await
    .map_err(query_error)?;
    let trigram = sqlx::query_scalar::<_, String>(
        "SELECT coalesce(local_slug, slug) FROM rh_wiki_topics
          WHERE $1 <% search_text AND (scope = $3 OR ($4 AND scope IS NULL))
         ORDER BY word_similarity($1, search_text) DESC, slug LIMIT $2",
    )
    .bind(query)
    .bind(SEARCH_LIMIT)
    .bind(&home.scope)
    .bind(home.primary())
    .fetch_all(&mut **tx)
    .await
    .map_err(query_error)?;
    let mut fused: Vec<(String, f64)> = Vec::new();
    for list in [full_text, trigram] {
        for (slug, rank) in list.into_iter().zip(1_u32..) {
            let score = 1.0 / (RRF_K + f64::from(rank));
            match fused.iter_mut().find(|(seen, _)| *seen == slug) {
                Some(entry) => entry.1 += score,
                None => fused.push((slug, score)),
            }
        }
    }
    Ok(fused
        .into_iter()
        .reduce(|best, next| if next.1 > best.1 { next } else { best }))
}
