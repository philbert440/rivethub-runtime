use std::collections::BTreeMap;

use kernel::memory::ConversationHead;
use kernel::memory::SummaryKind;
use kernel::memory::names_session;
use kernel::memory::needs_title;
use kernel::memory::summary_title;
use kernel::memory::wants_model_title;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::PostgresMemory;
use crate::error::Error;
use crate::summary_store::HeadRow;

const SUMMARIZED_SQL: &str = "select c.id, c.title from rh_conversations c
    where exists (
        select 1 from rh_summaries s where s.conversation_id = c.id and s.kind = $1
      )
    order by c.id";

const CURRENT_SQL: &str = "select title from rh_conversations where id = $1";

const LEAVES_SQL: &str = "select content from rh_summaries
    where conversation_id = $1 and kind = $2
    order by earliest_at, id";

const SET_SQL: &str = "update rh_conversations set title = $1
    where id = $2 and title is not distinct from $3";

const HEADS_SQL: &str = "select c.id, c.session_key, c.agent, c.channel, c.title
    from rh_conversations c
    where exists (
        select 1 from rh_summaries s where s.conversation_id = c.id and s.kind = $1
      )
    order by c.id";

const FIRST_LEAVES_SQL: &str = "select conversation_id, content from (
        select conversation_id, content, earliest_at, id,
               row_number() over (partition by conversation_id order by earliest_at, id) as place
        from rh_summaries
        where kind = $1 and conversation_id = any($2)
      ) leaves
    where place <= $3
    order by conversation_id, earliest_at, id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Untitled {
    pub head: ConversationHead,
    pub leaves: Vec<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct LeafRow {
    conversation_id: Uuid,
    content: String,
}

#[derive(Debug, sqlx::FromRow)]
struct TitledRow {
    id: Uuid,
    title: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct TitleRow {
    title: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ContentRow {
    content: String,
}

struct Retitle {
    current: Option<String>,
    title: String,
}

impl PostgresMemory {
    pub async fn backfill_titles(&self, dry_run: bool) -> Result<u64, Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|source| Error::operation("title backfill begin", source))?;
        let rows = sqlx::query_as::<_, TitledRow>(SUMMARIZED_SQL)
            .bind(SummaryKind::Leaf.as_str())
            .fetch_all(&mut *tx)
            .await
            .map_err(|source| Error::operation("summarized conversations", source))?;
        let mut titled = 0u64;
        for row in rows {
            if row.title.as_deref().is_some_and(names_session) {
                continue;
            }
            let changed = if dry_run {
                retitle(&mut tx, row.id).await?.is_some()
            } else {
                title_from_first_leaf(&mut tx, row.id).await?
            };
            if changed {
                titled += 1;
            }
        }
        tx.commit()
            .await
            .map_err(|source| Error::operation("title backfill commit", source))?;
        Ok(titled)
    }

    pub async fn untitled(&self, leaves: i64) -> Result<Vec<Untitled>, Error> {
        let heads: Vec<ConversationHead> = sqlx::query_as::<_, HeadRow>(HEADS_SQL)
            .bind(SummaryKind::Leaf.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(|source| Error::operation("summarized conversation heads", source))?
            .into_iter()
            .map(ConversationHead::from)
            .filter(wants_model_title)
            .collect();
        let ids: Vec<Uuid> = heads.iter().map(|head| head.id).collect();
        let rows = sqlx::query_as::<_, LeafRow>(FIRST_LEAVES_SQL)
            .bind(SummaryKind::Leaf.as_str())
            .bind(&ids)
            .bind(leaves)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| Error::operation("first leaves", source))?;
        let mut grouped: BTreeMap<Uuid, Vec<String>> = BTreeMap::new();
        for row in rows {
            grouped
                .entry(row.conversation_id)
                .or_default()
                .push(row.content);
        }
        Ok(heads
            .into_iter()
            .filter_map(|head| {
                grouped
                    .remove(&head.id)
                    .map(|leaves| Untitled { head, leaves })
            })
            .collect())
    }
}

pub(crate) async fn name_conversation(
    conn: &mut PgConnection,
    id: Uuid,
    title: &str,
) -> Result<bool, Error> {
    let current = sqlx::query_as::<_, TitleRow>(CURRENT_SQL)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| Error::operation("conversation title", source))?;
    let Some(TitleRow { title: current }) = current else {
        return Ok(false);
    };
    if !needs_title(current.as_deref()) {
        return Ok(false);
    }
    let updated = sqlx::query(SET_SQL)
        .bind(title)
        .bind(id)
        .bind(current)
        .execute(&mut *conn)
        .await
        .map_err(|source| Error::operation("set model title", source))?;
    Ok(updated.rows_affected() == 1)
}

pub(crate) async fn title_from_first_leaf(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<bool, Error> {
    let Some(retitle) = retitle(conn, id).await? else {
        return Ok(false);
    };
    let updated = sqlx::query(SET_SQL)
        .bind(retitle.title)
        .bind(id)
        .bind(retitle.current)
        .execute(&mut *conn)
        .await
        .map_err(|source| Error::operation("set summary title", source))?;
    Ok(updated.rows_affected() == 1)
}

async fn retitle(conn: &mut PgConnection, id: Uuid) -> Result<Option<Retitle>, Error> {
    let current = sqlx::query_as::<_, TitleRow>(CURRENT_SQL)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| Error::operation("conversation title", source))?;
    let Some(TitleRow { title: current }) = current else {
        return Ok(None);
    };
    if current.as_deref().is_some_and(names_session) {
        return Ok(None);
    }
    let leaves = sqlx::query_as::<_, ContentRow>(LEAVES_SQL)
        .bind(id)
        .bind(SummaryKind::Leaf.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(|source| Error::operation("leaf contents", source))?;
    Ok(leaves
        .iter()
        .find_map(|leaf| summary_title(&leaf.content))
        .map(|title| Retitle { current, title }))
}
