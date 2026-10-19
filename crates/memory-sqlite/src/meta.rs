use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;

use crate::error::SqliteError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum MetaKey {
    EmbedModel,
    EmbedDims,
}

pub(crate) fn read(conn: &Connection, key: MetaKey) -> Result<Option<String>, SqliteError> {
    conn.query_row(
        "SELECT value FROM ros_meta WHERE key = ?1",
        params![key.to_string()],
        |row| row.get(0),
    )
    .optional()
    .map_err(SqliteError::query("read store meta"))
}

pub(crate) fn record_absent(
    conn: &Connection,
    key: MetaKey,
    value: &str,
) -> Result<(), SqliteError> {
    conn.execute(
        "INSERT OR IGNORE INTO ros_meta (key, value) VALUES (?1, ?2)",
        params![key.to_string(), value],
    )
    .map_err(SqliteError::query("record store meta"))?;
    Ok(())
}

pub(crate) fn forget(conn: &Connection, key: MetaKey) -> Result<(), SqliteError> {
    conn.execute(
        "DELETE FROM ros_meta WHERE key = ?1",
        params![key.to_string()],
    )
    .map_err(SqliteError::query("forget store meta"))?;
    Ok(())
}
