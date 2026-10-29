use chrono::DateTime;
use chrono::Utc;
use kernel::memory::MemoryStoreError;
use kernel::memory::SummaryDag;
use uuid::Uuid;
use wire::gateway::memory::MemoryMessageTally;
use wire::gateway::memory::MemorySummaryNode;
use wire::gateway::memory::SummaryKind;
use wire::gateway::memory::VectorState;

use crate::PostgresMemory;
use crate::error::Error;

const CONVERSATION_SQL: &str = "select id from rh_conversations where session_key = $1
    order by active is not false desc, updated_at desc, id limit 1";

const DAG_SQL: &str = "with recursive dag(id, level) as (
        select id, 0 from rh_summaries where conversation_id = $1
        union
        select s.parent_id, dag.level + 1 from rh_summaries s join dag on dag.id = s.id
        where s.parent_id is not null
    )
    select s.id::text as id, s.parent_id::text as parent_id, max(dag.level)::bigint as level,
           s.kind, s.content, s.message_count, s.earliest_at, s.latest_at, s.created_at,
           array(select ss.message_id::text from rh_summary_sources ss
                 where ss.summary_id = s.id order by ss.ordinal) as source_message_ids,
           case when s.embedding is not null then 'embedded'
                when s.embed_status = 'failed' then 'failed'
                when s.embed_status = 'unembeddable' then 'skipped'
                else 'pending' end as vector
    from dag join rh_summaries s on s.id = dag.id
    group by s.id
    order by level desc, s.latest_at, s.id";

const TALLY_SQL: &str = "select count(*) as total,
        count(*) filter (where role = 'user') as \"user\",
        count(*) filter (where role = 'assistant') as assistant,
        count(*) filter (where role = 'system') as \"system\",
        count(*) filter (where role = 'tool' or tool_name is not null) as tool,
        count(embedding) as embedded,
        count(*) filter (where embedding is null
                           and embed_status is distinct from 'failed'
                           and embed_status is distinct from 'unembeddable') as pending,
        count(*) filter (where embedding is null and embed_status = 'failed') as failed,
        count(*) filter (where embedding is null and embed_status = 'unembeddable') as skipped
    from rh_messages where conversation_id = $1";

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
    vector: String,
}

#[derive(Debug, sqlx::FromRow)]
struct TallyRow {
    total: i64,
    user: i64,
    assistant: i64,
    system: i64,
    tool: i64,
    embedded: i64,
    pending: i64,
    failed: i64,
    skipped: i64,
}

impl PostgresMemory {
    pub(crate) async fn read_summary_dag(
        &self,
        session: &str,
    ) -> Result<SummaryDag, MemoryStoreError> {
        let conversation: Option<Uuid> = sqlx::query_scalar(CONVERSATION_SQL)
            .bind(session)
            .fetch_optional(&self.pool)
            .await
            .map_err(|source| Error::operation("summary dag", source))?;
        let Some(conversation) = conversation else {
            return Ok(SummaryDag::default());
        };
        let rows: Vec<DagRow> = sqlx::query_as(DAG_SQL)
            .bind(conversation)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| Error::operation("summary dag", source))?;
        let tally: TallyRow = sqlx::query_as(TALLY_SQL)
            .bind(conversation)
            .fetch_one(&self.pool)
            .await
            .map_err(|source| Error::operation("summary dag messages", source))?;
        Ok(SummaryDag {
            summaries: rows
                .into_iter()
                .map(summary_node)
                .collect::<Result<_, _>>()?,
            messages: message_tally(&tally)?,
        })
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
    let vector = row
        .vector
        .parse::<VectorState>()
        .map_err(|error| MemoryStoreError::Query {
            message: format!("summary vector state {} is not known", row.vector),
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
        vector,
    })
}

fn message_tally(row: &TallyRow) -> Result<MemoryMessageTally, MemoryStoreError> {
    let count = |value: i64| u32::try_from(value).map_err(count_error);
    Ok(MemoryMessageTally {
        total: count(row.total)?,
        user: count(row.user)?,
        assistant: count(row.assistant)?,
        system: count(row.system)?,
        tool: count(row.tool)?,
        embedded: count(row.embedded)?,
        pending: count(row.pending)?,
        failed: count(row.failed)?,
        skipped: count(row.skipped)?,
    })
}

fn count_error(error: std::num::TryFromIntError) -> MemoryStoreError {
    MemoryStoreError::Query {
        message: "a stored summary dag count does not fit an unsigned count".to_owned(),
        source: Box::new(error),
    }
}
