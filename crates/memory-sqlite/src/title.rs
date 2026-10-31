use kernel::memory::SummaryKind;
use kernel::memory::names_session;
use kernel::memory::needs_title;
use kernel::memory::summary_title;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::SqliteError;
use crate::SqliteStore;

const SUMMARIZED_SQL: &str = "SELECT id, title FROM ros_conversations c \
      WHERE EXISTS (SELECT 1 FROM ros_summaries s WHERE s.conversation_id = c.id AND s.kind = ?1) \
      ORDER BY c.id";

struct Retitle {
    current: Option<String>,
    title: String,
}

struct Stored {
    title: Option<String>,
}

impl SqliteStore {
    pub async fn backfill_titles(&self, dry_run: bool) -> Result<u64, SqliteError> {
        self.with_conn(move |conn| backfill(conn, dry_run)).await
    }
}

fn backfill(conn: &mut Connection, dry_run: bool) -> Result<u64, SqliteError> {
    let tx = conn
        .transaction()
        .map_err(SqliteError::query("title backfill begin"))?;
    let rows = summarized(&tx)?;
    let mut titled = 0u64;
    for (id, title) in rows {
        if title.as_deref().is_some_and(names_session) {
            continue;
        }
        let changed = if dry_run {
            retitle(&tx, &id)?.is_some()
        } else {
            title_from_first_leaf(&tx, &id)?
        };
        if changed {
            titled += 1;
        }
    }
    tx.commit()
        .map_err(SqliteError::query("title backfill commit"))?;
    Ok(titled)
}

pub(crate) fn title_from_first_leaf(conn: &Connection, id: &str) -> Result<bool, SqliteError> {
    let Some(retitle) = retitle(conn, id)? else {
        return Ok(false);
    };
    set_title(conn, id, retitle.current, retitle.title)
}

pub(crate) fn name_conversation(
    conn: &Connection,
    id: &str,
    title: &str,
) -> Result<bool, SqliteError> {
    let Some(Stored { title: current }) = current_title(conn, id)? else {
        return Ok(false);
    };
    if !needs_title(current.as_deref()) {
        return Ok(false);
    }
    set_title(conn, id, current, title.to_owned())
}

fn set_title(
    conn: &Connection,
    id: &str,
    current: Option<String>,
    title: String,
) -> Result<bool, SqliteError> {
    let updated = conn
        .execute(
            "UPDATE ros_conversations SET title = ?1 WHERE id = ?2 AND title IS ?3",
            params_from_iter([
                Value::Text(title),
                Value::Text(id.to_owned()),
                current.map_or(Value::Null, Value::Text),
            ]),
        )
        .map_err(SqliteError::query("set conversation title"))?;
    Ok(updated == 1)
}

fn current_title(conn: &Connection, id: &str) -> Result<Option<Stored>, SqliteError> {
    conn.query_row(
        "SELECT title FROM ros_conversations WHERE id = ?1",
        params_from_iter([Value::Text(id.to_owned())]),
        |row| Ok(Stored { title: row.get(0)? }),
    )
    .optional()
    .map_err(SqliteError::query("conversation title"))
}

fn retitle(conn: &Connection, id: &str) -> Result<Option<Retitle>, SqliteError> {
    let Some(Stored { title: current }) = current_title(conn, id)? else {
        return Ok(None);
    };
    if current.as_deref().is_some_and(names_session) {
        return Ok(None);
    }
    let mut statement = conn
        .prepare(
            "SELECT content FROM ros_summaries \
              WHERE conversation_id = ?1 AND kind = ?2 ORDER BY earliest_at, id",
        )
        .map_err(SqliteError::query("leaf contents"))?;
    let contents = statement
        .query_map(
            params_from_iter([
                Value::Text(id.to_owned()),
                Value::Text(SummaryKind::Leaf.as_str().to_owned()),
            ]),
            |row| row.get::<_, String>(0),
        )
        .map_err(SqliteError::query("leaf contents"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(SqliteError::query("leaf contents"))?;
    Ok(contents
        .iter()
        .find_map(|content| summary_title(content))
        .map(|title| Retitle { current, title }))
}

fn summarized(conn: &Connection) -> Result<Vec<(String, Option<String>)>, SqliteError> {
    let mut statement = conn
        .prepare(SUMMARIZED_SQL)
        .map_err(SqliteError::query("summarized conversations"))?;
    statement
        .query_map(
            params_from_iter([Value::Text(SummaryKind::Leaf.as_str().to_owned())]),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(SqliteError::query("summarized conversations"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(SqliteError::query("summarized conversations"))
}
