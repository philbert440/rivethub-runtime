pub mod error;
pub mod schema;
pub mod upgrade;
pub mod version;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::Connection;

pub use error::SqliteError;
pub use version::SchemaVersion;

const FILE_PRAGMAS: &str = "PRAGMA journal_mode = wal;\nPRAGMA foreign_keys = ON;";
const MEMORY_PRAGMAS: &str = "PRAGMA foreign_keys = ON;";

#[derive(Debug)]
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SqliteError> {
        Self::open_at(path, Utc::now())
    }

    pub fn open_at(path: impl AsRef<Path>, now: DateTime<Utc>) -> Result<Self, SqliteError> {
        let path = path.as_ref();
        let mut conn = Connection::open(path).map_err(|source| SqliteError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        conn.execute_batch(FILE_PRAGMAS)
            .map_err(SqliteError::Pragma)?;
        schema::prepare(&mut conn, now)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self, SqliteError> {
        let mut conn = Connection::open_in_memory().map_err(|source| SqliteError::Open {
            path: PathBuf::from(":memory:"),
            source,
        })?;
        conn.execute_batch(MEMORY_PRAGMAS)
            .map_err(SqliteError::Pragma)?;
        schema::prepare(&mut conn, Utc::now())?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
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
