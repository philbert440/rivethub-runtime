use std::collections::BTreeMap;
use std::collections::BTreeSet;

use kernel::memory::names_session;
use kernel::memory::summary_title;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;

use crate::error::SqliteError;

const TITLE_SQL: &str = "SELECT c.title,
        CASE WHEN c.title IS NULL OR trim(c.title) = '' THEN coalesce(
            (SELECT substr(s.content, 1, 2000) FROM ros_summaries s
              WHERE s.conversation_id = c.id
              ORDER BY s.latest_at IS NULL, s.latest_at DESC, s.id LIMIT 1),
            (SELECT substr(m.content, 1, 2000) FROM ros_messages m
              WHERE m.conversation_id = c.id AND m.role = 'user'
              ORDER BY m.created_at, m.id LIMIT 1))
        END
     FROM ros_conversations c WHERE c.id = ?1";

pub(super) fn session_titles<'a>(
    conn: &Connection,
    conversations: impl IntoIterator<Item = &'a str>,
) -> Result<BTreeMap<String, String>, SqliteError> {
    let wanted: BTreeSet<&str> = conversations.into_iter().collect();
    let mut statement = conn
        .prepare_cached(TITLE_SQL)
        .map_err(SqliteError::query("search titles"))?;
    let mut titles = BTreeMap::new();
    for id in wanted {
        let row: Option<(Option<String>, Option<String>)> = statement
            .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(SqliteError::query("search titles"))?;
        let title = row.and_then(|(title, spoken)| {
            title
                .filter(|title| names_session(title))
                .or_else(|| spoken.as_deref().and_then(summary_title))
        });
        if let Some(title) = title {
            titles.insert(id.to_owned(), title);
        }
    }
    Ok(titles)
}
