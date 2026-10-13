use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::error::SqliteError;
use crate::upgrade;
use crate::version::SchemaVersion;

const SCHEMA_SQL: &str = include_str!("schema/schema_v7.sql");

const TAIL_INDEXES_SQL: &str = "
CREATE INDEX IF NOT EXISTS idx_ros_messages_unembedded ON ros_messages (created_at)
          WHERE embedding IS NULL AND embed_status IS NULL;
CREATE INDEX IF NOT EXISTS idx_ros_conversations_owner ON ros_conversations (owner_user_id)
          WHERE owner_user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_ros_tags_removal_state ON ros_tags (removal_state)
          WHERE removal_state = 'suggested';
";

const CURRENT_VERSION_PRAGMA: &str = "PRAGMA user_version = 7;";

pub(super) fn prepare(conn: &mut Connection, now: DateTime<Utc>) -> Result<(), SqliteError> {
    let found = user_version(conn)?;
    let from = match found {
        0 => return create_fresh(conn),
        other => SchemaVersion::try_from(other)?,
    };
    match from {
        SchemaVersion::V7 => Ok(()),
        older @ (SchemaVersion::V1
        | SchemaVersion::V2
        | SchemaVersion::V3
        | SchemaVersion::V4
        | SchemaVersion::V5
        | SchemaVersion::V6) => bring_through(conn, older, now),
    }
}

fn create_fresh(conn: &mut Connection) -> Result<(), SqliteError> {
    let tx = begin_create(conn)?;
    apply_current_schema(&tx)?;
    apply_tail_indexes(&tx)?;
    tx.execute_batch(CURRENT_VERSION_PRAGMA)
        .map_err(SqliteError::Create)?;
    tx.commit().map_err(SqliteError::Create)
}

fn bring_through(
    conn: &mut Connection,
    from: SchemaVersion,
    now: DateTime<Utc>,
) -> Result<(), SqliteError> {
    let tx = begin_create(conn)?;
    apply_current_schema(&tx)?;
    tx.commit().map_err(SqliteError::Create)?;
    upgrade::ladder(conn, from, now)?;
    let tx = begin_create(conn)?;
    apply_tail_indexes(&tx)?;
    tx.execute_batch(CURRENT_VERSION_PRAGMA)
        .map_err(SqliteError::Create)?;
    tx.commit().map_err(SqliteError::Create)
}

fn user_version(conn: &Connection) -> Result<i64, SqliteError> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(SqliteError::Pragma)
}

fn begin_create(conn: &mut Connection) -> Result<rusqlite::Transaction<'_>, SqliteError> {
    conn.transaction().map_err(SqliteError::Create)
}

fn apply_current_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), SqliteError> {
    tx.execute_batch(SCHEMA_SQL).map_err(SqliteError::Create)
}

fn apply_tail_indexes(tx: &rusqlite::Transaction<'_>) -> Result<(), SqliteError> {
    tx.execute_batch(TAIL_INDEXES_SQL)
        .map_err(SqliteError::Create)
}

#[cfg(test)]
mod tests {
    use super::TAIL_INDEXES_SQL;

    #[test]
    fn the_tail_indexes_are_absent_from_the_schema_text() {
        let schema = include_str!("schema/schema_v7.sql");
        for marker in [
            "idx_ros_messages_unembedded",
            "idx_ros_conversations_owner",
            "idx_ros_tags_removal_state",
        ] {
            assert!(
                !schema.contains(marker),
                "{marker} must be applied after the schema text, not inside it"
            );
            assert!(
                TAIL_INDEXES_SQL.contains(marker),
                "{marker} must be created by the tail batch"
            );
        }
    }
}
