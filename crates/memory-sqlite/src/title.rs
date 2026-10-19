use kernel::memory::SummaryKind;
use kernel::memory::summary_title;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params_from_iter;
use rusqlite::types::Value;

use crate::SqliteError;
use crate::SqliteStore;

const UNTITLED_SQL: &str = "SELECT id FROM ros_conversations c \
      WHERE (c.title IS NULL OR trim(c.title) = '') \
        AND EXISTS (SELECT 1 FROM ros_summaries s WHERE s.conversation_id = c.id AND s.kind = ?1) \
      ORDER BY c.id";

impl SqliteStore {
    pub async fn backfill_titles(&self, dry_run: bool) -> Result<u64, SqliteError> {
        self.with_conn(move |conn| backfill(conn, dry_run)).await
    }
}

fn backfill(conn: &mut Connection, dry_run: bool) -> Result<u64, SqliteError> {
    let tx = conn
        .transaction()
        .map_err(SqliteError::query("title backfill begin"))?;
    let ids = untitled(&tx)?;
    let mut titled = 0u64;
    for id in ids {
        let changed = if dry_run {
            derived_title(&tx, &id)?.is_some()
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
    let Some(title) = derived_title(conn, id)? else {
        return Ok(false);
    };
    let updated = conn
        .execute(
            "UPDATE ros_conversations SET title = ?1 \
              WHERE id = ?2 AND (title IS NULL OR trim(title) = '')",
            params_from_iter([Value::Text(title), Value::Text(id.to_owned())]),
        )
        .map_err(SqliteError::query("set summary title"))?;
    Ok(updated == 1)
}

fn derived_title(conn: &Connection, id: &str) -> Result<Option<String>, SqliteError> {
    let current: Option<Option<String>> = conn
        .query_row(
            "SELECT title FROM ros_conversations WHERE id = ?1",
            params_from_iter([Value::Text(id.to_owned())]),
            |row| row.get(0),
        )
        .optional()
        .map_err(SqliteError::query("conversation title"))?;
    match current {
        None => return Ok(None),
        Some(Some(title)) if !title.trim().is_empty() => return Ok(None),
        Some(_) => {}
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
    Ok(contents.iter().find_map(|content| summary_title(content)))
}

fn untitled(conn: &Connection) -> Result<Vec<String>, SqliteError> {
    let mut statement = conn
        .prepare(UNTITLED_SQL)
        .map_err(SqliteError::query("untitled conversations"))?;
    statement
        .query_map(
            params_from_iter([Value::Text(SummaryKind::Leaf.as_str().to_owned())]),
            |row| row.get::<_, String>(0),
        )
        .map_err(SqliteError::query("untitled conversations"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(SqliteError::query("untitled conversations"))
}
