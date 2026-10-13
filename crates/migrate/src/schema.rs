use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use rusqlite::Connection;

use crate::Status;
use crate::error::Error;
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

const CONNECTION_PRAGMAS: &str = "PRAGMA journal_mode = wal;\nPRAGMA foreign_keys = ON;";

pub fn sqlite(path: &Path) -> Result<Status, Error> {
    sqlite_at(path, Utc::now())
}

pub fn sqlite_at(path: &Path, now: DateTime<Utc>) -> Result<Status, Error> {
    if !path.exists() {
        let mut conn = open_writing(path)?;
        create_fresh(&mut conn)?;
        return Ok(current());
    }
    let found = user_version_read_only(path)?;
    if found == 0 {
        let mut conn = open_writing(path)?;
        create_fresh(&mut conn)?;
        return Ok(current());
    }
    match SchemaVersion::try_from(found)? {
        SchemaVersion::V7 => Ok(current()),
        from @ (SchemaVersion::V1
        | SchemaVersion::V2
        | SchemaVersion::V3
        | SchemaVersion::V4
        | SchemaVersion::V5
        | SchemaVersion::V6) => {
            copy_original(path, from)?;
            let mut conn = open_writing(path)?;
            bring_through(&mut conn, from, now)?;
            Ok(current())
        }
    }
}

pub fn sqlite_status(path: &Path) -> Result<Status, Error> {
    let required = SchemaVersion::V7.number();
    if !path.exists() {
        return Ok(Status {
            current: 0,
            required,
            pending: steps(0),
        });
    }
    match user_version_read_only(path)? {
        0 => Ok(Status {
            current: 0,
            required,
            pending: steps(0),
        }),
        found => {
            let from = SchemaVersion::try_from(found)?;
            Ok(Status {
                current: from.number(),
                required,
                pending: steps(from.number()),
            })
        }
    }
}

pub fn sqlite_connection(conn: &mut Connection, now: DateTime<Utc>) -> Result<(), Error> {
    match user_version(conn)? {
        0 => create_fresh(conn),
        7 => Ok(()),
        found => bring_through(conn, SchemaVersion::try_from(found)?, now),
    }
}

fn current() -> Status {
    Status {
        current: SchemaVersion::V7.number(),
        required: SchemaVersion::V7.number(),
        pending: 0,
    }
}

fn steps(from: i64) -> usize {
    let mut count = 0usize;
    let mut current = from;
    while current < SchemaVersion::V7.number() {
        count += 1;
        current += 1;
    }
    count
}

fn open_writing(path: &Path) -> Result<Connection, Error> {
    let conn = Connection::open(path).map_err(|source| Error::SqliteOpen {
        path: path.to_path_buf(),
        source,
    })?;
    conn.execute_batch(CONNECTION_PRAGMAS)
        .map_err(Error::SqlitePragma)?;
    Ok(conn)
}

fn user_version_read_only(path: &Path) -> Result<i64, Error> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|source| Error::SqliteOpen {
            path: path.to_path_buf(),
            source,
        })?;
    user_version(&conn)
}

fn copy_original(path: &Path, from: SchemaVersion) -> Result<(), Error> {
    let backup = backup_path(path, from);
    if backup.exists() {
        return Err(Error::BackupExists { path: backup });
    }
    std::fs::copy(path, &backup).map_err(|source| Error::Backup {
        path: path.to_path_buf(),
        backup,
        source,
    })?;
    Ok(())
}

fn backup_path(path: &Path, from: SchemaVersion) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".v{}.bak", from.number()));
    PathBuf::from(name)
}

fn create_fresh(conn: &mut Connection) -> Result<(), Error> {
    let tx = begin(conn)?;
    apply_schema(&tx)?;
    apply_tail_indexes(&tx)?;
    tx.execute_batch(CURRENT_VERSION_PRAGMA)
        .map_err(Error::SqliteCreate)?;
    tx.commit().map_err(Error::SqliteCreate)
}

fn bring_through(
    conn: &mut Connection,
    from: SchemaVersion,
    now: DateTime<Utc>,
) -> Result<(), Error> {
    let tx = begin(conn)?;
    apply_schema(&tx)?;
    tx.commit().map_err(Error::SqliteCreate)?;
    upgrade::ladder(conn, from, now)?;
    let tx = begin(conn)?;
    apply_tail_indexes(&tx)?;
    tx.execute_batch(CURRENT_VERSION_PRAGMA)
        .map_err(Error::SqliteCreate)?;
    tx.commit().map_err(Error::SqliteCreate)
}

fn user_version(conn: &Connection) -> Result<i64, Error> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(Error::SqlitePragma)
}

fn begin(conn: &mut Connection) -> Result<rusqlite::Transaction<'_>, Error> {
    conn.transaction().map_err(Error::SqliteCreate)
}

fn apply_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), Error> {
    tx.execute_batch(SCHEMA_SQL).map_err(Error::SqliteCreate)
}

fn apply_tail_indexes(tx: &rusqlite::Transaction<'_>) -> Result<(), Error> {
    tx.execute_batch(TAIL_INDEXES_SQL)
        .map_err(Error::SqliteCreate)
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
