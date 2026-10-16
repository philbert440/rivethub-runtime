use async_trait::async_trait;
use kernel::memory::jobs::{MemoryJobError, MemoryJobStore, ToolCallMessage};
use kernel::memory::{EmbedTable, EmbedTarget};
use serde::Deserialize;
use sqlx::types::Json;
use uuid::Uuid;
use wire::JsonObject;

use crate::PostgresMemory;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolArguments {
    Object(JsonObject),
    Encoded(String),
}

#[derive(sqlx::FromRow)]
struct ToolRow {
    id: Uuid,
    tool_name: Option<String>,
    tool_args: Option<Json<ToolArguments>>,
    content: String,
}

#[async_trait]
impl MemoryJobStore for PostgresMemory {
    async fn tool_call(&self, id: Uuid) -> Result<Option<ToolCallMessage>, MemoryJobError> {
        let row = sqlx::query_as::<_, ToolRow>(
            "SELECT id, tool_name, NULLIF(tool_args, 'null'::jsonb) AS tool_args, content FROM rh_messages WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(query_error)?;
        row.map(|row| {
            Ok(ToolCallMessage {
                id: row.id,
                tool_name: row.tool_name,
                tool_args: arguments(row.tool_args)?,
                content: row.content,
            })
        })
        .transpose()
    }

    async fn write_tool_sentence(
        &self,
        id: Uuid,
        previous_content: &str,
        content: &str,
    ) -> Result<(), MemoryJobError> {
        sqlx::query("UPDATE rh_messages SET content = $2 WHERE id = $1 AND content = $3")
            .bind(id)
            .bind(content)
            .bind(previous_content)
            .execute(&self.pool)
            .await
            .map_err(query_error)?;
        Ok(())
    }

    async fn unchunked(
        &self,
        limit: u32,
        chunk_chars: u32,
        tool_result_cap: u32,
    ) -> Result<Vec<EmbedTarget>, MemoryJobError> {
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT m.id FROM rh_messages m WHERE m.embedding IS NOT NULL
             AND m.embed_status IS NULL
             AND length(concat_ws(E'\\n', NULLIF(btrim(m.content), ''),
                 left(NULLIF(btrim(m.tool_result), ''), $2))) > $1
             AND NOT EXISTS (SELECT 1 FROM rh_message_chunks c WHERE c.message_id = m.id)
             ORDER BY m.created_at DESC, m.id LIMIT $3",
        )
        .bind(i64::from(chunk_chars))
        .bind(
            i32::try_from(tool_result_cap)
                .map_err(|error| MemoryJobError::Arguments(Box::new(error)))?,
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(query_error)?;
        Ok(ids
            .into_iter()
            .map(|id| EmbedTarget {
                table: EmbedTable::Messages,
                id: id.to_string(),
            })
            .collect())
    }

    async fn stale_wiki(
        &self,
        limit: u32,
    ) -> Result<Vec<kernel::wiki::builder::ExtractWiki>, MemoryJobError> {
        let rows = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
            "SELECT s.id, s.conversation_id FROM rh_summaries s
             JOIN rh_wiki_extractions e ON e.summary_id = s.id
             WHERE e.status = 'done' AND e.pipeline_version < $1
             ORDER BY s.created_at, s.id LIMIT $2",
        )
        .bind(kernel::wiki::builder::WIKI_PIPELINE_VERSION)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(query_error)?;
        Ok(rows
            .into_iter()
            .map(
                |(summary_id, conversation_id)| kernel::wiki::builder::ExtractWiki {
                    summary_id,
                    conversation_id,
                },
            )
            .collect())
    }

    async fn unembedded(
        &self,
        table: EmbedTable,
        limit: u32,
    ) -> Result<Vec<EmbedTarget>, MemoryJobError> {
        let result = unembedded(self, table, limit).await;
        match result {
            Err(error)
                if matches!(table, EmbedTable::WikiTopics | EmbedTable::MessageChunks)
                    && crate::Error::relation_missing(&error) =>
            {
                Err(MemoryJobError::MissingTable {
                    table,
                    source: Box::new(error),
                })
            }
            result => result.map_err(query_error),
        }
    }
}

fn arguments(args: Option<Json<ToolArguments>>) -> Result<JsonObject, MemoryJobError> {
    let args = match args {
        Some(Json(ToolArguments::Object(args))) => args,
        Some(Json(ToolArguments::Encoded(text))) if !text.trim().is_empty() => {
            serde_json::from_str(&text)
                .map_err(|error| MemoryJobError::Arguments(Box::new(error)))?
        }
        Some(Json(ToolArguments::Encoded(_))) | None => JsonObject::default(),
    };
    Ok(args)
}

async fn unembedded(
    store: &PostgresMemory,
    table: EmbedTable,
    limit: u32,
) -> Result<Vec<EmbedTarget>, sqlx::Error> {
    let (relation, id, predicate) = match table {
        EmbedTable::Messages => (
            "rh_messages",
            "id",
            "(length(content) > 20 OR length(tool_result) > 20)",
        ),
        EmbedTable::Summaries => ("rh_summaries", "id", "length(content) > 20"),
        EmbedTable::WikiTopics => ("rh_wiki_topics", "slug", "length(search_text) > 20"),
        EmbedTable::MessageChunks => ("rh_message_chunks", "id", "btrim(content) <> ''"),
    };
    let mut connection = store.pool.acquire().await?;
    if table != EmbedTable::MessageChunks {
        let mut heal = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE ");
        heal.push(relation).push(" SET embed_status = NULL, embed_error = NULL, embed_failures = 0 WHERE embedding IS NULL AND embed_status = 'failed' AND embed_error = ")
            .push_bind("Embedding returned null").push(" AND ").push(predicate);
        heal.build().execute(&mut *connection).await?;
    }
    let mut select = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    select
        .push(id)
        .push("::text FROM ")
        .push(relation)
        .push(" WHERE embedding IS NULL AND embed_status IS NULL AND ")
        .push(predicate)
        .push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(i64::from(limit));
    let ids = select
        .build_query_scalar::<String>()
        .fetch_all(&mut *connection)
        .await?;
    Ok(ids
        .into_iter()
        .map(|id| EmbedTarget { table, id })
        .collect())
}

fn query_error(error: sqlx::Error) -> MemoryJobError {
    MemoryJobError::Query(Box::new(error))
}
