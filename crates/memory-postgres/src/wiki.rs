use chrono::DateTime;
use chrono::Utc;
use kernel::wiki::WikiFilter;
use kernel::wiki::WikiLookup;
use sqlx::Postgres;
use sqlx::QueryBuilder;
use wire::gateway::wiki::WikiCitation;
use wire::gateway::wiki::WikiGapsResponse;
use wire::gateway::wiki::WikiPageResponse;
use wire::gateway::wiki::WikiRedLink;
use wire::gateway::wiki::WikiSlug;
use wire::gateway::wiki::WikiSource;
use wire::gateway::wiki::WikiTopic;
use wire::gateway::wiki::WikiTopicList;

use crate::Error;
use crate::PostgresMemory;

#[derive(Debug, sqlx::FromRow)]
struct TopicRow {
    slug: String,
    title: String,
    tags: Vec<String>,
    entities: Vec<String>,
    current_state: String,
    updated_at: DateTime<Utc>,
}

fn topic_query(columns: &str, filter: &WikiFilter) -> QueryBuilder<Postgres> {
    let mut query = QueryBuilder::new(format!("SELECT {columns} FROM rh_wiki_topics WHERE true"));
    if let Some(text) = &filter.query {
        query.push(" AND (content_tsv @@ plainto_tsquery('english', ");
        query.push_bind(text);
        query.push(") OR strpos(lower(title), lower(");
        query.push_bind(text);
        query.push(")) > 0 OR strpos(lower(slug), lower(");
        query.push_bind(text);
        query.push(")) > 0)");
    }
    if let Some(tag) = &filter.tag {
        query.push(" AND ");
        query.push_bind(tag);
        query.push(" = ANY(tags)");
    }
    query
}

impl PostgresMemory {
    pub(crate) async fn read_wiki_topics(
        &self,
        filter: WikiFilter,
    ) -> Result<WikiTopicList, Error> {
        let mut transaction = self.pool.begin().await.map_err(Error::Connection)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *transaction)
            .await
            .map_err(|source| Error::operation("wiki read snapshot", source))?;
        let (total,): (i64,) = topic_query("count(*)", &filter)
            .build_query_as()
            .fetch_one(&mut *transaction)
            .await
            .map_err(|source| Error::operation("wiki topic count", source))?;
        let rows: Vec<TopicRow> = topic_query(
            "slug, title, tags, entities, current_state, updated_at",
            &filter,
        )
        .push(" ORDER BY updated_at DESC, slug LIMIT ")
        .push_bind(i64::from(filter.limit))
        .push(" OFFSET ")
        .push_bind(i64::from(filter.offset))
        .build_query_as()
        .fetch_all(&mut *transaction)
        .await
        .map_err(|source| Error::operation("wiki topics", source))?;
        transaction
            .commit()
            .await
            .map_err(|source| Error::operation("wiki read snapshot", source))?;
        Ok(WikiTopicList {
            topics: rows
                .into_iter()
                .map(|row| WikiTopic {
                    slug: row.slug,
                    title: row.title,
                    tags: row.tags,
                    entities: row.entities,
                    updated_at: row.updated_at,
                    excerpt: row.current_state.chars().take(240).collect(),
                })
                .collect(),
            total: u64::try_from(total).map_err(|error| Error::protocol(error.to_string()))?,
        })
    }

    pub(crate) async fn read_wiki_page(&self, slug: &WikiSlug) -> Result<WikiLookup, Error> {
        let row = sqlx::query!(
            "SELECT slug, title, aliases, tags, entities, current_state, article, related,
                    git_sha, last_verified_at, updated_at
             FROM rh_wiki_topics WHERE slug = $1",
            slug.as_ref()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| Error::operation("wiki page", source))?;
        let Some(row) = row else {
            return self.wiki_redirect(slug).await;
        };
        let (citations, sources) =
            tokio::try_join!(self.wiki_citations(&row.slug), self.wiki_sources(&row.slug))?;
        let markdown = if let Some(directory) = &self.wiki_directory {
            read_markdown(directory, slug).await?
        } else if row.article.is_empty() {
            format!("# {}\n\n{}\n", row.title, row.current_state)
        } else {
            format!(
                "# {}\n\n{}\n\n{}\n",
                row.title, row.current_state, row.article
            )
        };
        Ok(WikiLookup::Page(Box::new(WikiPageResponse {
            slug: row.slug,
            title: row.title,
            aliases: row.aliases,
            tags: row.tags,
            entities: row.entities,
            current_state: row.current_state,
            article: (!row.article.is_empty()).then_some(row.article),
            see_also: None,
            history: Vec::new(),
            citations,
            markdown,
            sources,
            git_sha: row.git_sha,
            last_verified: row.last_verified_at,
            updated_at: row.updated_at,
            related: Some(row.related),
        })))
    }

    async fn wiki_redirect(&self, slug: &WikiSlug) -> Result<WikiLookup, Error> {
        let redirect = sqlx::query!(
            "SELECT r.to_slug FROM rh_wiki_redirects r
             JOIN rh_wiki_topics t ON t.slug = r.to_slug WHERE r.from_slug = $1",
            slug.as_ref()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| Error::operation("wiki redirect", source))?;
        Ok(match redirect {
            Some(row) => WikiLookup::Redirect(row.to_slug),
            None => WikiLookup::Missing,
        })
    }

    async fn wiki_citations(&self, slug: &str) -> Result<Vec<WikiCitation>, Error> {
        let rows = sqlx::query!(
            "SELECT summary_id, kind, note, cited_at FROM rh_wiki_citations
             WHERE topic_slug = $1 ORDER BY cited_at, summary_id",
            slug
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("wiki citations", source))?;
        Ok(rows
            .into_iter()
            .map(|row| WikiCitation {
                summary_id: row.summary_id.to_string(),
                date: Some(row.cited_at),
                kind: row.kind,
                note: row.note,
            })
            .collect())
    }

    async fn wiki_sources(&self, slug: &str) -> Result<Vec<WikiSource>, Error> {
        let rows = sqlx::query!(
            "SELECT source_kind, source_id, conversation_id FROM rh_wiki_provenance
             WHERE topic_slug = $1 ORDER BY source_kind, source_id",
            slug
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("wiki sources", source))?;
        rows.into_iter()
            .map(|row| {
                Ok(WikiSource {
                    kind: row
                        .source_kind
                        .parse::<wire::gateway::wiki::WikiSourceKind>()
                        .map_err(|source| {
                            Error::operation(
                                "wiki source kind",
                                sqlx::Error::ColumnDecode {
                                    index: "source_kind".to_owned(),
                                    source: Box::new(source),
                                },
                            )
                        })?,
                    ids: vec![row.source_id.to_string()],
                    conversation_id: row.conversation_id.map(|id| id.to_string()),
                    span: None,
                })
            })
            .collect()
    }

    pub(crate) async fn read_wiki_gaps(&self, limit: u32) -> Result<WikiGapsResponse, Error> {
        let limit = i64::from(limit.clamp(1, 200));
        let red_links = sqlx::query!(
            "SELECT entity AS \"entity!\", array_agg(DISTINCT t.slug ORDER BY t.slug) AS \"referenced_by!\"
             FROM rh_wiki_topics t CROSS JOIN LATERAL unnest(t.entities) entity
             WHERE NOT EXISTS (
                 SELECT 1 FROM rh_wiki_topics known
                 WHERE lower(known.slug) = lower(entity) OR lower(known.title) = lower(entity)
                 OR EXISTS (SELECT 1 FROM unnest(known.aliases) alias WHERE lower(alias) = lower(entity))
             ) AND NOT EXISTS (
                 SELECT 1 FROM rh_wiki_redirects r WHERE lower(r.from_slug) = lower(entity)
             )
             GROUP BY entity ORDER BY count(DISTINCT t.slug) DESC, entity LIMIT $1",
            limit
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("wiki red links", source))?;
        let stalest = sqlx::query!(
            "SELECT slug FROM rh_wiki_topics
             ORDER BY coalesce(last_verified_at, updated_at), slug LIMIT $1",
            limit
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("wiki stale topics", source))?;
        Ok(WikiGapsResponse {
            red_links: red_links
                .into_iter()
                .map(|row| WikiRedLink {
                    entity: row.entity,
                    referenced_by: row.referenced_by,
                })
                .collect(),
            stalest: stalest.into_iter().map(|row| row.slug).collect(),
        })
    }
}

async fn read_markdown(directory: &std::path::Path, slug: &WikiSlug) -> Result<String, Error> {
    let root = tokio::fs::canonicalize(directory)
        .await
        .map_err(Error::WikiIo)?;
    let topics = tokio::fs::canonicalize(root.join("topics"))
        .await
        .map_err(Error::WikiIo)?;
    if !topics.starts_with(&root) {
        return Err(Error::WikiContainment);
    }
    let path = tokio::fs::canonicalize(topics.join(format!("{}.md", slug.as_ref())))
        .await
        .map_err(Error::WikiIo)?;
    if !path.starts_with(&topics) {
        return Err(Error::WikiContainment);
    }
    tokio::fs::read_to_string(path).await.map_err(Error::WikiIo)
}
