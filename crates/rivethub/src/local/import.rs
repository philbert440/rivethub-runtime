use std::path::PathBuf;

use config::Config;
use memory_sqlite::SqliteStore;
use memory_sqlite::import::{ImportError, ImportTable};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    #[error(transparent)]
    Import(#[from] ImportError),
    #[error("local import worker failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error(
        "one-time PGlite import is pending; set memory.import_pg_dump to a dump file; run: {command}"
    )]
    MissingDump { command: String },
    #[error("one-time PGlite import dump {path} is absent; run: {command}")]
    AbsentDump { path: PathBuf, command: String },
    #[error("cannot inspect import dump {path}: {source}")]
    Inspect {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub(super) async fn pending(
    store: &SqliteStore,
    config: &Config,
    shutdown: &CancellationToken,
) -> Result<(), Error> {
    if !config.pglite_import_pending() || store.pg_import_report().await?.is_some() {
        return Ok(());
    }
    let dump = config
        .memory
        .as_ref()
        .and_then(|memory| memory.import_pg_dump.clone());
    let path = tokio::task::spawn_blocking(move || require_dump(dump)).await??;
    let report = store.import_pg_dump(path, shutdown.clone()).await?;
    tracing::info!(?report, "one-time PGlite import completed");
    Ok(())
}

fn require_dump(path: Option<PathBuf>) -> Result<PathBuf, Error> {
    let command = format!(
        "pg_dump {} --file=<dump-file> <PGLITE_URL>",
        ImportTable::pg_dump_arguments().join(" ")
    );
    let path = path.ok_or_else(|| Error::MissingDump {
        command: command.clone(),
    })?;
    match path.try_exists() {
        Ok(true) => Ok(path),
        Ok(false) => Err(Error::AbsentDump { path, command }),
        Err(source) => Err(Error::Inspect { path, source }),
    }
}
