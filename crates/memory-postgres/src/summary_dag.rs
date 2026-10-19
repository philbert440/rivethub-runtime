use chrono::DateTime;
use chrono::Utc;
use kernel::memory::MemoryStoreError;
use wire::gateway::memory::MemorySummaryNode;
use wire::gateway::memory::SummaryKind;

use crate::PostgresMemory;
use crate::error::Error;

const DAG_SQL: &str = "with recursive conversation(id) as (
        select id from rh_conversations
        where session_key = $1 and active is not false
        order by updated_at desc, id limit 1
    ), dag(id, level) as (
        select s.id, 0 from rh_summaries s join conversation c on s.conversation_id = c.id
        union
        select s.parent_id, dag.level + 1 from rh_summaries s join dag on dag.id = s.id
        where s.parent_id is not null
    )
    select s.id::text as id, s.parent_id::text as parent_id, max(dag.level)::bigint as level,
           s.kind, s.content, s.message_count, s.earliest_at, s.latest_at, s.created_at,
           array(select ss.message_id::text from rh_summary_sources ss
                 where ss.summary_id = s.id order by ss.ordinal) as source_message_ids
    from dag join rh_summaries s on s.id = dag.id
    group by s.id
    order by level desc, s.latest_at, s.id";

#[derive(Debug, sqlx::FromRow)]
struct DagRow {
    id: String,
    parent_id: Option<String>,
    level: i64,
    kind: String,
    content: String,
    message_count: i32,
    earliest_at: Option<DateTime<Utc>>,
    latest_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    source_message_ids: Vec<String>,
}

impl PostgresMemory {
    pub(crate) async fn read_summary_dag(
        &self,
        session: &str,
    ) -> Result<Vec<MemorySummaryNode>, MemoryStoreError> {
        let rows: Vec<DagRow> = sqlx::query_as(DAG_SQL)
            .bind(session)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| Error::operation("summary dag", source))?;
        rows.into_iter().map(summary_node).collect()
    }
}

fn summary_node(row: DagRow) -> Result<MemorySummaryNode, MemoryStoreError> {
    let kind = row
        .kind
        .parse::<SummaryKind>()
        .map_err(|error| MemoryStoreError::Query {
            message: format!(
                "stored summary kind {} is not leaf, branch, or root",
                row.kind
            ),
            source: Box::new(error),
        })?;
    Ok(MemorySummaryNode {
        id: row.id,
        parent_id: row.parent_id,
        level: u32::try_from(row.level).map_err(count_error)?,
        kind,
        content: row.content,
        message_count: u32::try_from(row.message_count).map_err(count_error)?,
        earliest_at: row.earliest_at,
        latest_at: row.latest_at,
        created_at: row.created_at,
        source_message_ids: row.source_message_ids,
    })
}

fn count_error(error: std::num::TryFromIntError) -> MemoryStoreError {
    MemoryStoreError::Query {
        message: "a stored summary count does not fit an unsigned count".to_owned(),
        source: Box::new(error),
    }
}
