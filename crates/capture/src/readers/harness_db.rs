use std::path::Path;

use kernel::transcript::TranscriptReaderError;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::Row;
use thiserror::Error;
use wire::message::Role;

const HERMES_SESSIONS: &str = "SELECT DISTINCT session_id FROM messages ORDER BY session_id";

const HERMES_MESSAGES: &str = "\
SELECT rowid, session_id, role, content, timestamp \
FROM messages \
WHERE session_id = ?1 AND rowid > ?2 \
ORDER BY timestamp ASC, rowid ASC";

const OPENCODE_SESSIONS: &str = "SELECT DISTINCT session_id FROM message ORDER BY session_id";

const OPENCODE_PARTS: &str = "\
SELECT part.rowid AS rowid, \
       part.session_id AS session_id, \
       part.message_id AS message_id, \
       message.role AS role, \
       part.type AS part_type, \
       part.text AS text, \
       part.tool_name AS tool_name, \
       part.tool_arguments AS tool_arguments, \
       part.tool_result AS tool_result, \
       part.created_at AS created_at \
FROM part \
LEFT JOIN message ON message.id = part.message_id \
WHERE part.session_id = ?1 AND part.rowid > ?2 \
ORDER BY part.created_at ASC, part.rowid ASC";

#[derive(Debug, Error)]
pub(crate) enum HarnessDbError {
    #[error("harness database query failed: {message}")]
    Query {
        message: String,
        #[source]
        source: kernel::BoxError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DbRows<T> {
    Ready(Vec<T>),
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Speaker {
    Known(Role),
    Unrecognized { text: String, parse_error: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HermesMessage {
    pub(crate) row_id: i64,
    pub(crate) session_id: String,
    pub(crate) speaker: Speaker,
    pub(crate) content: String,
    pub(crate) at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartKind {
    Text,
    Tool,
    Unrecognized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpencodePart {
    pub(crate) row_id: i64,
    pub(crate) session_id: String,
    pub(crate) message_id: String,
    pub(crate) speaker: Speaker,
    pub(crate) kind: PartKind,
    pub(crate) text: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) tool_arguments: Option<String>,
    pub(crate) tool_result: Option<String>,
    pub(crate) at: String,
}

#[derive(Debug)]
pub(crate) struct HarnessDb;

impl HarnessDb {
    pub(crate) async fn hermes_sessions(database: &Path) -> Result<DbRows<String>, HarnessDbError> {
        let database = database.to_path_buf();
        spawn(move || read_ids(&database, HERMES_SESSIONS)).await
    }

    pub(crate) async fn hermes_messages(
        database: &Path,
        session_id: &str,
        after_row: i64,
    ) -> Result<DbRows<HermesMessage>, HarnessDbError> {
        let database = database.to_path_buf();
        let session_id = session_id.to_owned();
        spawn(move || read_hermes(&database, &session_id, after_row)).await
    }

    pub(crate) async fn opencode_sessions(
        database: &Path,
    ) -> Result<DbRows<String>, HarnessDbError> {
        let database = database.to_path_buf();
        spawn(move || read_ids(&database, OPENCODE_SESSIONS)).await
    }

    pub(crate) async fn opencode_parts(
        database: &Path,
        session_id: &str,
        after_row: i64,
    ) -> Result<DbRows<OpencodePart>, HarnessDbError> {
        let database = database.to_path_buf();
        let session_id = session_id.to_owned();
        spawn(move || read_opencode(&database, &session_id, after_row)).await
    }
}

pub(crate) fn read_error(error: HarnessDbError) -> TranscriptReaderError {
    TranscriptReaderError::Read {
        message: error.to_string(),
        source: Box::new(error),
    }
}

enum Opened {
    Ready(Connection),
    Unreadable,
}

fn read_only() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

fn open_read(path: &Path) -> Opened {
    match Connection::open_with_flags(path, read_only()) {
        Ok(connection) => Opened::Ready(connection),
        Err(error) => {
            tracing::warn!(
                %error,
                path = %path.display(),
                "harness database could not be opened"
            );
            Opened::Unreadable
        }
    }
}

async fn spawn<T: Send + 'static>(
    work: impl FnOnce() -> Result<DbRows<T>, HarnessDbError> + Send + 'static,
) -> Result<DbRows<T>, HarnessDbError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(query_error(
            "harness database worker stopped",
            Box::new(error),
        )),
    }
}

fn read_ids(path: &Path, sql: &str) -> Result<DbRows<String>, HarnessDbError> {
    let Opened::Ready(connection) = open_read(path) else {
        return Ok(DbRows::Unreadable);
    };
    let queried = connection.prepare(sql).and_then(|mut statement| {
        statement
            .query_map([], |row| row.get::<_, String>("session_id"))?
            .collect::<Result<Vec<_>, _>>()
    });
    rows_or_unreadable(queried)
}

fn read_hermes(
    path: &Path,
    session_id: &str,
    after_row: i64,
) -> Result<DbRows<HermesMessage>, HarnessDbError> {
    let Opened::Ready(connection) = open_read(path) else {
        return Ok(DbRows::Unreadable);
    };
    let queried = connection
        .prepare(HERMES_MESSAGES)
        .and_then(|mut statement| {
            statement
                .query_map(rusqlite::params![session_id, after_row], hermes_row)?
                .collect::<Result<Vec<_>, _>>()
        });
    rows_or_unreadable(queried)
}

fn hermes_row(row: &Row<'_>) -> rusqlite::Result<HermesMessage> {
    let role: String = row.get("role")?;
    Ok(HermesMessage {
        row_id: row.get("rowid")?,
        session_id: row.get("session_id")?,
        speaker: speaker_from_text(role),
        content: row.get("content")?,
        at: row.get("timestamp")?,
    })
}

fn read_opencode(
    path: &Path,
    session_id: &str,
    after_row: i64,
) -> Result<DbRows<OpencodePart>, HarnessDbError> {
    let Opened::Ready(connection) = open_read(path) else {
        return Ok(DbRows::Unreadable);
    };
    let queried = connection
        .prepare(OPENCODE_PARTS)
        .and_then(|mut statement| {
            statement
                .query_map(rusqlite::params![session_id, after_row], opencode_row)?
                .collect::<Result<Vec<_>, _>>()
        });
    rows_or_unreadable(queried)
}

fn opencode_row(row: &Row<'_>) -> rusqlite::Result<OpencodePart> {
    let role: Option<String> = row.get("role")?;
    let kind: String = row.get("part_type")?;
    Ok(OpencodePart {
        row_id: row.get("rowid")?,
        session_id: row.get("session_id")?,
        message_id: row.get("message_id")?,
        speaker: speaker(role),
        kind: part_kind(&kind),
        text: row.get("text")?,
        tool_name: row.get("tool_name")?,
        tool_arguments: row.get("tool_arguments")?,
        tool_result: row.get("tool_result")?,
        at: row.get("created_at")?,
    })
}

fn speaker_from_text(text: String) -> Speaker {
    match text.parse::<Role>() {
        Ok(role) => Speaker::Known(role),
        Err(error) => Speaker::Unrecognized {
            text,
            parse_error: error.to_string(),
        },
    }
}

fn part_kind(text: &str) -> PartKind {
    match text {
        "text" => PartKind::Text,
        "tool" => PartKind::Tool,
        _ => PartKind::Unrecognized,
    }
}

fn speaker(role: Option<String>) -> Speaker {
    match role {
        Some(text) => speaker_from_text(text),
        None => Speaker::Unrecognized {
            text: String::new(),
            parse_error: "part has no message row".to_owned(),
        },
    }
}

fn rows_or_unreadable<T>(
    result: Result<Vec<T>, rusqlite::Error>,
) -> Result<DbRows<T>, HarnessDbError> {
    match result {
        Ok(rows) => Ok(DbRows::Ready(rows)),
        Err(error) if not_a_database(&error) => {
            tracing::warn!(%error, "harness database is not a sqlite file");
            Ok(DbRows::Unreadable)
        }
        Err(error) => Err(query_error(&error.to_string(), Box::new(error))),
    }
}

fn not_a_database(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::NotADatabase
    )
}

fn query_error(message: &str, source: kernel::BoxError) -> HarnessDbError {
    HarnessDbError::Query {
        message: message.to_owned(),
        source,
    }
}
