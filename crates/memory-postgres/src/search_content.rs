use std::collections::BTreeMap;

use kernel::memory::MemoryHitSource;
use uuid::Uuid;

use crate::error::Error;
use crate::search::MemorySearchHit;
use crate::search::hit_uuids;

const MESSAGE_CONTENTS: &str = "SELECT id, content FROM rh_messages WHERE id = ANY($1)";
const SUMMARY_CONTENTS: &str = "SELECT id, content FROM rh_summaries WHERE id = ANY($1)";
const CHUNK_CONTENTS: &str = "SELECT c.message_id AS id, c.idx, c.content FROM rh_message_chunks c \
     JOIN unnest($1::uuid[], $2::int4[]) AS hit(message_id, idx) \
       ON hit.message_id = c.message_id AND hit.idx = c.idx";

#[derive(Debug, sqlx::FromRow)]
struct ContentRow {
    id: Uuid,
    content: String,
}

#[derive(Debug, sqlx::FromRow)]
struct ChunkRow {
    id: Uuid,
    idx: i32,
    content: String,
}

impl crate::PostgresMemory {
    pub(crate) async fn fill_contents(&self, hits: &mut Vec<MemorySearchHit>) -> Result<(), Error> {
        let (mut messages, mut summaries, mut chunks) = tokio::try_join!(
            self.contents(MESSAGE_CONTENTS, hit_uuids(hits, MemoryHitSource::Message)?),
            self.contents(SUMMARY_CONTENTS, hit_uuids(hits, MemoryHitSource::Summary)?),
            self.chunk_contents(hits),
        )?;
        hits.retain_mut(|hit| {
            if let Some(chunk) = &mut hit.chunk
                && let Some(content) = chunks.remove(&(hit.id.clone(), chunk.index))
            {
                chunk.content = content;
            }
            let found = match hit.source {
                MemoryHitSource::Message => messages.remove(&hit.id),
                MemoryHitSource::Summary => summaries.remove(&hit.id),
            };
            found.map(|content| hit.content = content).is_some()
        });
        Ok(())
    }

    async fn chunk_contents(
        &self,
        hits: &[MemorySearchHit],
    ) -> Result<BTreeMap<(String, u32), String>, Error> {
        let mut messages = Vec::new();
        let mut indexes = Vec::new();
        for hit in hits {
            if let Some(chunk) = &hit.chunk {
                messages.push(Uuid::parse_str(&hit.id).map_err(|error| {
                    Error::protocol(format!("search hit id {} is not a uuid: {error}", hit.id))
                })?);
                indexes.push(i32::try_from(chunk.index)?);
            }
        }
        if messages.is_empty() {
            return Ok(BTreeMap::new());
        }
        let rows: Vec<ChunkRow> = sqlx::query_as(CHUNK_CONTENTS)
            .bind(messages)
            .bind(indexes)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| Error::operation("search contents", source))?;
        rows.into_iter()
            .map(|row| Ok(((row.id.to_string(), u32::try_from(row.idx)?), row.content)))
            .collect()
    }

    async fn contents(
        &self,
        statement: &'static str,
        ids: Vec<Uuid>,
    ) -> Result<BTreeMap<String, String>, Error> {
        if ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let rows: Vec<ContentRow> = sqlx::query_as(statement)
            .bind(ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| Error::operation("search contents", source))?;
        Ok(rows
            .into_iter()
            .map(|row| (row.id.to_string(), row.content))
            .collect())
    }
}
