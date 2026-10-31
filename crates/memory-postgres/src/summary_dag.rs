use chrono::DateTime;
use chrono::Utc;
use kernel::memory::MemoryMessageTally;
use kernel::memory::MemoryStoreError;
use kernel::memory::MemorySummaryNode;
use kernel::memory::SUMMARY_PREVIEW_BYTES;
use kernel::memory::SummaryDag;
use kernel::memory::SummaryKind;
use kernel::memory::SummaryOwner;
use kernel::memory::VectorState;
use uuid::Uuid;

use crate::PostgresMemory;
use crate::error::Error;

const SESSION_SQL: &str = "select id, session_key from rh_conversations where session_key = any($1)
    order by active is not false desc, updated_at desc, id limit 1";

const CONVERSATION_SQL: &str = "select id, session_key from rh_conversations where id = $1";

const DAG_SQL: &str = "with recursive dag(id, level) as (
        select id, 0 from rh_summaries where conversation_id = $1
        union
        select up.parent_id, dag.level + 1
          from dag
          cross join lateral (
              select s.parent_id from rh_summaries s where s.id = dag.id offset 0
          ) up
         where up.parent_id is not null
    ),
    levels as (select id, max(level) as level from dag group by id)
    select s.id::text as id, s.parent_id::text as parent_id, l.level::bigint as level, s.kind,
           case when $2::int is null then convert_to(s.content, 'UTF8')
                else substring(convert_to(s.content, 'UTF8') from 1 for $2) end as content,
           $2::int is not null and octet_length(s.content) > $2 as truncated,
           s.message_count, s.earliest_at, s.latest_at, s.created_at,
           case when $2::int is null
                then array(select ss.message_id::text from rh_summary_sources ss
                            where ss.summary_id = s.id order by ss.ordinal)
                else '{}'::text[] end as source_message_ids,
           case when s.embedding is not null then 'embedded'
                when s.embed_status = 'failed' then 'failed'
                when s.embed_status = 'unembeddable' then 'skipped'
                else 'pending' end as vector
    from levels l
    cross join lateral (select * from rh_summaries s where s.id = l.id offset 0) s
    where $3::uuid is null or l.id = $3
    order by l.level desc, s.latest_at, s.id";

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
    content: Vec<u8>,
    truncated: bool,
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

#[derive(Debug, Clone, Copy)]
enum Detail {
    Full,
    Outline,
    One(Uuid),
}

impl PostgresMemory {
    pub(crate) async fn read_summary_dag(
        &self,
        owner: &SummaryOwner,
    ) -> Result<SummaryDag, MemoryStoreError> {
        self.dag(owner, Detail::Full).await
    }

    pub(crate) async fn read_summary_outline(
        &self,
        owner: &SummaryOwner,
    ) -> Result<SummaryDag, MemoryStoreError> {
        self.dag(owner, Detail::Outline).await
    }

    pub(crate) async fn read_summary_node(
        &self,
        owner: &SummaryOwner,
        id: &str,
    ) -> Result<Option<MemorySummaryNode>, MemoryStoreError> {
        let id = Uuid::parse_str(id).map_err(|error| MemoryStoreError::Query {
            message: format!("summary id {id} is not a uuid"),
            source: Box::new(error),
        })?;
        Ok(self.dag(owner, Detail::One(id)).await?.summaries.pop())
    }

    async fn conversation_of(
        &self,
        owner: &SummaryOwner,
    ) -> Result<Option<(Uuid, String)>, MemoryStoreError> {
        match owner {
            SummaryOwner::Session(session) => {
                sqlx::query_as(SESSION_SQL).bind(kernel::filing::stored_keys(session))
            }
            SummaryOwner::Conversation(id) => sqlx::query_as(CONVERSATION_SQL).bind(*id),
        }
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| Error::operation("summary dag", source).into())
    }

    async fn dag(
        &self,
        owner: &SummaryOwner,
        detail: Detail,
    ) -> Result<SummaryDag, MemoryStoreError> {
        let Some((conversation, session_key)) = self.conversation_of(owner).await? else {
            return Ok(SummaryDag::default());
        };
        let (preview, only) = match detail {
            Detail::Full => (None, None),
            Detail::Outline => (
                Some(i32::try_from(SUMMARY_PREVIEW_BYTES).map_err(count_error)?),
                None,
            ),
            Detail::One(id) => (None, Some(id)),
        };
        let rows = sqlx::query_as::<_, DagRow>(DAG_SQL)
            .bind(conversation)
            .bind(preview)
            .bind(only)
            .fetch_all(&self.pool);
        let tally = async {
            if only.is_some() {
                return Ok(None);
            }
            sqlx::query_as::<_, TallyRow>(TALLY_SQL)
                .bind(conversation)
                .fetch_one(&self.pool)
                .await
                .map(Some)
        };
        let (rows, tally) = tokio::try_join!(rows, tally)
            .map_err(|source| Error::operation("summary dag", source))?;
        Ok(SummaryDag {
            session_key: Some(session_key),
            summaries: rows
                .into_iter()
                .map(summary_node)
                .collect::<Result<_, _>>()?,
            messages: tally
                .as_ref()
                .map(message_tally)
                .transpose()?
                .unwrap_or_default(),
        })
    }
}

fn text(bytes: Vec<u8>, truncated: bool) -> Result<String, MemoryStoreError> {
    String::from_utf8(bytes).or_else(|error| {
        let cut = error.utf8_error();
        if truncated && cut.error_len().is_none() {
            let valid = cut.valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).map_err(stored_text)
        } else {
            Err(stored_text(error))
        }
    })
}

fn stored_text(error: std::string::FromUtf8Error) -> MemoryStoreError {
    MemoryStoreError::Query {
        message: "a stored summary is not UTF-8".to_owned(),
        source: Box::new(error),
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
        content: text(row.content, row.truncated)?,
        message_count: u32::try_from(row.message_count).map_err(count_error)?,
        earliest_at: row.earliest_at,
        latest_at: row.latest_at,
        created_at: row.created_at,
        source_message_ids: row.source_message_ids,
        vector,
        truncated: row.truncated,
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
