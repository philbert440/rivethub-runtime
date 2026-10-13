use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::Transaction;
use rusqlite::params;

use crate::error::SqliteError;
use crate::version::SchemaVersion;

pub(super) fn ladder(
    conn: &mut Connection,
    from: SchemaVersion,
    now: DateTime<Utc>,
) -> Result<(), SqliteError> {
    let mut current = from;
    while let Some(next) = current.next() {
        step(conn, next, now)?;
        current = next;
    }
    Ok(())
}

fn step(conn: &mut Connection, to: SchemaVersion, now: DateTime<Utc>) -> Result<(), SqliteError> {
    match to {
        SchemaVersion::V1 | SchemaVersion::V2 | SchemaVersion::V4 | SchemaVersion::V5 => Ok(()),
        SchemaVersion::V3 => in_transaction(conn, to, |tx| {
            add_message_embedding_columns(tx)?;
            move_embed_queue_into_jobs(tx, now)
        }),
        SchemaVersion::V6 => in_transaction(conn, to, add_owner_columns),
        SchemaVersion::V7 => in_transaction(conn, to, add_tag_removal_columns),
    }
}

fn in_transaction(
    conn: &mut Connection,
    to: SchemaVersion,
    run: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<()>,
) -> Result<(), SqliteError> {
    let failed = |source: rusqlite::Error| SqliteError::Upgrade {
        version: to.number(),
        source,
    };
    let tx = conn.transaction().map_err(failed)?;
    run(&tx).map_err(failed)?;
    tx.commit().map_err(failed)
}

fn add_message_embedding_columns(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    if column_present(tx, "ros_messages", "embedding")? {
        return Ok(());
    }
    tx.execute_batch(
        "ALTER TABLE ros_messages ADD COLUMN embedding BLOB;
         ALTER TABLE ros_messages ADD COLUMN embed_error TEXT;
         ALTER TABLE ros_messages ADD COLUMN embed_failures INTEGER NOT NULL DEFAULT 0;",
    )
}

fn add_owner_columns(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    if !column_present(tx, "ros_conversations", "owner_user_id")? {
        tx.execute(
            "ALTER TABLE ros_conversations ADD COLUMN owner_user_id TEXT",
            [],
        )?;
    }
    if !column_present(tx, "ros_messages", "owner_user_id")? {
        tx.execute("ALTER TABLE ros_messages ADD COLUMN owner_user_id TEXT", [])?;
    }
    Ok(())
}

fn add_tag_removal_columns(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    if column_present(tx, "ros_tags", "removal_state")? {
        return Ok(());
    }
    tx.execute_batch(
        "ALTER TABLE ros_tags ADD COLUMN removal_state TEXT CHECK (removal_state IS NULL OR removal_state IN ('suggested', 'rejected'));
         ALTER TABLE ros_tags ADD COLUMN removal_reason TEXT NOT NULL DEFAULT '';",
    )
}

fn move_embed_queue_into_jobs(tx: &Transaction<'_>, now: DateTime<Utc>) -> rusqlite::Result<()> {
    let stamp = timestamp(now);
    tx.execute(
        r"INSERT INTO ros_jobs (id, task, job_key, payload, run_at, attempts, max_attempts, state, last_error, locked_at, created_at, updated_at)
           SELECT id, 'embed-target', 'embed-ros_messages-' || message_id,
                  json_object('targetTable', 'ros_messages', 'targetId', message_id),
                  enqueued_at, 0, 5, 'queued', NULL, NULL, ?1, ?1
           FROM ros_embed_queue
           ORDER BY rowid",
        params![stamp],
    )?;
    tx.execute("DELETE FROM ros_embed_queue", [])?;
    Ok(())
}

fn column_present(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2",
        params![table, column],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|found| found.is_some())
}

#[must_use]
fn timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::timestamp;
    use chrono::TimeZone;
    use chrono::Utc;

    #[test]
    fn timestamps_render_as_millisecond_instant_text() {
        let now = Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap();
        assert_eq!(timestamp(now), "2026-10-10T12:00:00.000Z");
    }
}
