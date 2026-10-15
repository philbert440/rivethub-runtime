use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("inspecting memory file {path} failed: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("connecting to postgres failed")]
    PostgresConnect(#[source] sqlx::Error),
    #[error("applying the postgres migrations failed")]
    PostgresMigrate(#[source] sqlx::migrate::MigrateError),
    #[error("reading the applied postgres schema version failed")]
    PostgresVersion(#[source] sqlx::Error),
    #[error("missing _rivetos_migrations rows: {missing}")]
    TrackingMissing { missing: String },
    #[error("the postgres migration set is empty")]
    PostgresEmpty,
    #[error("the memory file at {path} could not be opened")]
    SqliteOpen {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("a memory schema pragma failed")]
    SqlitePragma(#[source] rusqlite::Error),
    #[error("the memory schema could not be created")]
    SqliteCreate(#[source] rusqlite::Error),
    #[error("the memory schema upgrade to version {version} failed")]
    SqliteUpgrade {
        version: i64,
        #[source]
        source: rusqlite::Error,
    },
    #[error("the memory schema version {found} is not supported by this build")]
    Unsupported { found: i64 },
    #[error("copying {path} to {backup} failed")]
    Backup {
        path: PathBuf,
        backup: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("a backup already exists at {path} — refusing to replace it")]
    BackupExists { path: PathBuf },
}

pub(crate) fn relation_missing(source: &sqlx::Error) -> bool {
    source
        .as_database_error()
        .is_some_and(|error| error.code().as_deref() == Some("42P01"))
}
