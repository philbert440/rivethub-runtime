use kernel::memory::SummaryKind;
use kernel::memory::names_session;
use kernel::memory::summary_title;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::PostgresMemory;
use crate::error::Error;

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
