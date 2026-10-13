use std::path::PathBuf;

use thiserror::Error;

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
}
