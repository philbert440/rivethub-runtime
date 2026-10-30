use std::collections::BTreeMap;

use crate::search::MemorySearchHit;
use chrono::{DateTime, Utc};
use kernel::memory::MemoryEntry;
use kernel::memory::{
    Limit, MemoryStore, MemoryStoreError, SearchQuery, SummaryDetail, SummaryExcerpt, TagListQuery,
};
use kernel::memory::{MemoryHitSource, SummaryKind, TagEntityType, TagState};
use sqlx::types::Json;
use uuid::Uuid;

use crate::{Error, PostgresMemory, read::MessageEntryRow};

#[derive(sqlx::FromRow)]
struct SummaryRow {
    kind: Json<SummaryKind>,
    conversation_id: Uuid,
    earliest_at: Option<DateTime<Utc>>,
    latest_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ChildRow {
    kind: Json<SummaryKind>,
    content: String,
}

impl PostgresMemory {
    pub(crate) async fn summary_details(
        &self,
        hits: &[MemorySearchHit],
        query: &SearchQuery,
    ) -> Result<BTreeMap<String, SummaryDetail>, MemoryStoreError> {
        let mut details = BTreeMap::new();
        for hit in hits
            .iter()
            .filter(|hit| hit.source == MemoryHitSource::Summary)
        {
            let id = Uuid::parse_str(&hit.id).map_err(Error::from)?;
            let row: SummaryRow = sqlx::query_as("SELECT to_json(kind) AS kind, conversation_id, earliest_at, latest_at FROM rh_summaries WHERE id = $1")
                .bind(id).fetch_one(&self.pool).await.map_err(|source| Error::operation("summary detail", source))?;
            let tags = self
                .list_tags(TagListQuery {
                    entity_type: Some(TagEntityType::Conversation),
                    entity_id: Some(row.conversation_id),
                    key: None,
                    value: None,
                    states: vec![TagState::Accepted],
                    limit: Limit::clamped(None, 1000, 1..=1000),
                })
                .await?;
            let (children, sources) = if query.expand == Some(false) {
                (Vec::new(), Vec::new())
            } else {
                tokio::try_join!(self.summary_children(id), self.summary_sources(id, query))?
            };
            details.insert(
                hit.id.clone(),
                SummaryDetail {
                    kind: row.kind.0,
                    earliest_at: row.earliest_at,
                    latest_at: row.latest_at,
                    tags,
                    children,
                    sources,
                },
            );
        }
        Ok(details)
    }

    async fn summary_children(&self, id: Uuid) -> Result<Vec<SummaryExcerpt>, Error> {
        let rows: Vec<ChildRow> = sqlx::query_as("SELECT to_json(kind) AS kind, content FROM rh_summaries WHERE parent_id = $1 ORDER BY earliest_at, id")
            .bind(id).fetch_all(&self.pool).await.map_err(|source| Error::operation("summary children", source))?;
        Ok(rows
            .into_iter()
            .map(|row| SummaryExcerpt {
                kind: row.kind.0,
                content: row.content,
            })
            .collect())
    }

    async fn summary_sources(
        &self,
        id: Uuid,
        query: &SearchQuery,
    ) -> Result<Vec<MemoryEntry>, Error> {
        let rows: Vec<MessageEntryRow> = sqlx::query_as(
            "SELECT m.id::text AS id, c.session_key, m.agent, m.channel, m.role AS role_text,
                    m.content, m.tool_name, m.tool_args::text AS tool_args, m.tool_result,
                    m.metadata::text AS metadata, m.created_at
             FROM rh_messages m JOIN rh_conversations c ON c.id = m.conversation_id
             WHERE EXISTS (SELECT 1 FROM rh_summary_sources ss WHERE ss.message_id = m.id
               AND (ss.summary_id = $1 OR ss.summary_id IN (SELECT id FROM rh_summaries WHERE parent_id = $1)))
             AND m.role <> 'tool' AND length(m.content) > 50
             AND ($2::text IS NULL OR m.agent = $2)
             AND ($3::timestamptz IS NULL OR m.created_at >= $3)
             AND ($4::timestamptz IS NULL OR m.created_at < $4)
             ORDER BY m.created_at, m.id LIMIT 200")
            .bind(id).bind(&query.agent).bind(query.since).bind(query.before)
            .fetch_all(&self.pool).await.map_err(|source| Error::operation("summary source messages", source))?;
        Ok(crate::read::readable_entries(rows))
    }
}
