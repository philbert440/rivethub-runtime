use std::num::ParseIntError;
use std::num::TryFromIntError;
use std::path::PathBuf;

use kernel::memory::MemoryStoreError;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub enum TimestampField {
    CreatedAt,
    UpdatedAt,
}

#[derive(Debug, Error)]
pub enum SqliteError {
    #[error("the memory file at {path} could not be opened")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "the memory file at {path} is not initialised — run rivethub-migrate sqlite --path {path}"
    )]
    NotInitialised { path: PathBuf },
    #[error(
        "the memory schema is behind: the file reports version {found}, this build requires {required} — run rivethub-migrate sqlite --path {path}"
    )]
    SchemaBehind {
        path: PathBuf,
        found: i64,
        required: i64,
    },
    #[error("a memory schema pragma failed")]
    Pragma(#[source] rusqlite::Error),
    #[error("the memory schema version {found} is not supported by this build")]
    UnsupportedSchema { found: i64 },
    #[error("the blocking memory database worker did not complete")]
    Join(#[source] tokio::task::JoinError),
    #[error("{field} is not a timestamp")]
    InvalidTimestamp {
        field: TimestampField,
        #[source]
        source: chrono::ParseError,
    },
    #[error("memory {operation} failed")]
    Query {
        operation: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("the {column} JSON could not be encoded")]
    Encode {
        column: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("stored {column} is not a JSON object")]
    StoredJson {
        column: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("stored {column} \"{value}\" is not a timestamp")]
    StoredTimestamp {
        column: &'static str,
        value: String,
        #[source]
        source: chrono::ParseError,
    },
    #[error("a stored count does not fit an unsigned count")]
    Count(#[source] TryFromIntError),
    #[error("job {id} does not exist")]
    MissingJob { id: String },
    #[error("the job clock overflowed")]
    Clock,
    #[error("no handler is registered for task {task}")]
    NoHandler { task: String },
    #[error("stored job state \"{value}\" is not queued, running, or dead")]
    StoredState { value: String },
    #[error("stored embed_dims \"{value}\" is not a width")]
    StoredDims {
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("embed job for {id} was already queued")]
    EmbedAlreadyQueued { id: String },
    #[error("job payload is not a JSON object")]
    PayloadNotObject,
    #[error("{message}")]
    ClaimRejected { message: String },
    #[error("job insert was rejected without a job_key")]
    InsertRejected,
    #[error("stored payload is not job JSON")]
    StoredPayload {
        #[source]
        source: serde_json::Error,
    },
    #[error("{message}")]
    Embed { message: String },
    #[error("embed target {table} {id} does not exist")]
    MissingEmbedTarget { table: String, id: String },
    #[error("stored message role \"{value}\" is not a role")]
    StoredRole { value: String },
}

impl SqliteError {
    pub(crate) fn query(operation: &'static str) -> impl FnOnce(rusqlite::Error) -> Self {
        move |source| Self::Query { operation, source }
    }
}

impl From<SqliteError> for MemoryStoreError {
    fn from(error: SqliteError) -> Self {
        match error {
            SqliteError::InvalidTimestamp { .. } => Self::InvalidRequest {
                message: error.to_string(),
            },
            SqliteError::Query { .. }
            | SqliteError::StoredJson { .. }
            | SqliteError::StoredTimestamp { .. }
            | SqliteError::Count(_)
            | SqliteError::Encode { .. }
            | SqliteError::MissingJob { .. }
            | SqliteError::Clock
            | SqliteError::NoHandler { .. }
            | SqliteError::StoredState { .. }
            | SqliteError::StoredDims { .. }
            | SqliteError::EmbedAlreadyQueued { .. }
            | SqliteError::PayloadNotObject
            | SqliteError::ClaimRejected { .. }
            | SqliteError::InsertRejected
            | SqliteError::StoredPayload { .. }
            | SqliteError::Embed { .. }
            | SqliteError::MissingEmbedTarget { .. }
            | SqliteError::StoredRole { .. } => Self::Query {
                message: error.to_string(),
                source: Box::new(error),
            },
            SqliteError::Open { .. }
            | SqliteError::NotInitialised { .. }
            | SqliteError::SchemaBehind { .. }
            | SqliteError::Pragma(_)
            | SqliteError::UnsupportedSchema { .. }
            | SqliteError::Join(_) => Self::Unavailable(Box::new(error)),
        }
    }
}
