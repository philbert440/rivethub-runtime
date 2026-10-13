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
    #[error("a memory schema pragma failed")]
    Pragma(#[source] rusqlite::Error),
    #[error("the memory schema could not be brought to version 7")]
    Create(#[source] rusqlite::Error),
    #[error("the memory schema upgrade to version {version} failed")]
    Upgrade {
        version: i64,
        #[source]
        source: rusqlite::Error,
    },
    #[error("the memory schema version {found} is not supported by this build")]
    UnsupportedSchema { found: i64 },
    #[error("the blocking memory database worker did not complete")]
    Join(#[source] tokio::task::JoinError),
}
