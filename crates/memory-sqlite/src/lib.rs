mod append;
pub mod capture;
mod conversation;
pub mod error;
mod insert;
mod message;
mod read;
mod stats;
mod time;

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::Connection;

pub use capture::MAX_CONTENT_CHARS;
pub use error::SqliteError;
pub use error::TimestampField;

pub const SCHEMA_VERSION: i64 = 7;

const FILE_PRAGMAS: &str = "PRAGMA journal_mode = wal;\nPRAGMA foreign_keys = ON;";

#[cfg(test)]
const MEMORY_PRAGMAS: &str = "PRAGMA foreign_keys = ON;";

#[derive(Debug)]
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
    owner: Option<String>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SqliteError> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(SqliteError::NotInitialised {
                path: path.to_path_buf(),
            });
        }
        let found = read_user_version(path)?;
        verify(path, found)?;
        let conn = Connection::open(path).map_err(|source| SqliteError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        conn.execute_batch(FILE_PRAGMAS)
            .map_err(SqliteError::Pragma)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            owner: None,
        })
    }

    #[must_use]
    pub fn with_owner(mut self, owner: &str) -> Self {
        let owner = owner.trim();
        self.owner = (!owner.is_empty()).then(|| owner.to_owned());
        self
    }

    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub async fn with_conn<T, F>(&self, f: F) -> Result<T, SqliteError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, SqliteError> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        match tokio::task::spawn_blocking(move || f(&mut conn.lock())).await {
            Ok(result) => result,
            Err(source) => Err(SqliteError::Join(source)),
        }
    }
}

fn read_user_version(path: &Path) -> Result<i64, SqliteError> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|source| SqliteError::Open {
            path: path.to_path_buf(),
            source,
        })?;
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(SqliteError::Pragma)
}

fn verify(path: &Path, found: i64) -> Result<(), SqliteError> {
    match found {
        0 => Err(SqliteError::NotInitialised {
            path: path.to_path_buf(),
        }),
        SCHEMA_VERSION => Ok(()),
        unsupported if !(0..=SCHEMA_VERSION).contains(&unsupported) => {
            Err(SqliteError::UnsupportedSchema { found: unsupported })
        }
        behind => Err(SqliteError::SchemaBehind {
            path: path.to_path_buf(),
            found: behind,
            required: SCHEMA_VERSION,
        }),
    }
}

#[cfg(test)]
use std::path::PathBuf;

#[cfg(test)]
impl SqliteStore {
    fn open_in_memory() -> Result<Self, migrate::Error> {
        let mut conn =
            Connection::open_in_memory().map_err(|source| migrate::Error::SqliteOpen {
                path: PathBuf::from(":memory:"),
                source,
            })?;
        conn.execute_batch(MEMORY_PRAGMAS)
            .map_err(migrate::Error::SqlitePragma)?;
        migrate::sqlite_connection(&mut conn, chrono::Utc::now())?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            owner: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::SqliteStore;
    use crate::error::SqliteError;

    #[tokio::test]
    async fn with_conn_runs_off_the_async_worker() {
        let store = SqliteStore::open_in_memory().unwrap();
        let awaited_on = std::thread::current().id();
        let ran_on = store
            .with_conn(move |_conn| {
                assert!(tokio::runtime::Handle::try_current().is_ok());
                Ok(std::thread::current().id())
            })
            .await
            .unwrap();
        assert_ne!(ran_on, awaited_on);
    }

    #[tokio::test]
    async fn in_memory_store_applies_the_current_schema() {
        let store = SqliteStore::open_in_memory().unwrap();
        let state = store
            .with_conn(move |conn| {
                let version: i64 = conn
                    .query_row("PRAGMA user_version", [], |row| row.get(0))
                    .unwrap();
                let tables: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                Ok((version, tables))
            })
            .await
            .unwrap();
        assert_eq!(state, (7, 32));
    }

    #[tokio::test]
    async fn with_conn_propagates_the_closure_error() {
        let store = SqliteStore::open_in_memory().unwrap();
        let outcome = store
            .with_conn(move |_conn| Err::<i64, _>(SqliteError::UnsupportedSchema { found: 9 }))
            .await;
        match outcome {
            Err(SqliteError::UnsupportedSchema { found }) => assert_eq!(found, 9),
            other => panic!("expected UnsupportedSchema, got {other:?}"),
        }
    }
}
