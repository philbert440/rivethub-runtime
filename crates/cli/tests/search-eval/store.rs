use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use memory_sqlite::SqliteError;
use memory_sqlite::SqliteStore;
use tempfile::TempDir;

use crate::Failure;
use crate::corpus::CONVERSATION_SQL;
use crate::corpus::Corpus;
use crate::corpus::MESSAGE_SQL;
use crate::corpus::SUMMARY_SQL;
use crate::read;

const STORE_FILE: &str = "memory.sqlite";
const WAL_SUFFIX: &str = "-wal";
const CONTENT_SQL: &str = "SELECT content FROM ros_messages WHERE id = ?1 \
     UNION ALL SELECT content FROM ros_summaries WHERE id = ?1";

pub(crate) struct Opened {
    _scratch: TempDir,
    pub(crate) path: PathBuf,
    pub(crate) store: SqliteStore,
}

pub(crate) async fn seeded(corpus: &Path) -> Result<Opened, Failure> {
    let corpus: Corpus = crate::set::parse(corpus)?;
    let scratch = tempfile::tempdir().map_err(Failure::Scratch)?;
    let path = scratch.path().join(STORE_FILE);
    migrate::sqlite(&path)?;
    let store = SqliteStore::open(&path)?;
    store
        .with_conn(move |conn| {
            let seed_failed = |source| SqliteError::Query {
                operation: "seed the eval corpus",
                source,
            };
            for row in &corpus.conversations {
                conn.execute(CONVERSATION_SQL, (&row.id, &row.agent, &row.created_at))
                    .map_err(seed_failed)?;
            }
            for row in &corpus.messages {
                let role = row.role.to_string();
                conn.execute(
                    MESSAGE_SQL,
                    (
                        &row.id,
                        &row.conversation,
                        &role,
                        &row.content,
                        &row.created_at,
                    ),
                )
                .map_err(seed_failed)?;
            }
            for row in &corpus.summaries {
                let kind = row.kind.to_string();
                conn.execute(
                    SUMMARY_SQL,
                    (
                        &row.id,
                        &row.conversation,
                        &row.content,
                        &kind,
                        &row.created_at,
                    ),
                )
                .map_err(seed_failed)?;
            }
            Ok(())
        })
        .await?;
    Ok(Opened {
        _scratch: scratch,
        path,
        store,
    })
}

pub(crate) fn pinned(source: &Path) -> Result<Opened, Failure> {
    let scratch = tempfile::tempdir().map_err(Failure::Scratch)?;
    let path = scratch.path().join(STORE_FILE);
    copy(source, &path)?;
    let wal = sidecar(source);
    if wal.try_exists().map_err(read(&wal))? {
        copy(&wal, &sidecar(&path))?;
    }
    let store = SqliteStore::open(&path)?;
    Ok(Opened {
        _scratch: scratch,
        path,
        store,
    })
}

pub(crate) async fn contents(
    store: &SqliteStore,
    ids: Vec<String>,
) -> Result<BTreeMap<String, String>, Failure> {
    Ok(store
        .with_conn(move |conn| {
            let failed = |source| SqliteError::Query {
                operation: "read the relevant rows",
                source,
            };
            let mut statement = conn.prepare(CONTENT_SQL).map_err(failed)?;
            let mut contents = BTreeMap::new();
            for id in ids {
                let found: Vec<String> = statement
                    .query_map([&id], |row| row.get(0))
                    .map_err(failed)?
                    .collect::<Result<_, _>>()
                    .map_err(failed)?;
                if let Some(content) = found.into_iter().next() {
                    contents.insert(id, content);
                }
            }
            Ok(contents)
        })
        .await?)
}

fn copy(from: &Path, to: &Path) -> Result<(), Failure> {
    std::fs::copy(from, to)
        .map(drop)
        .map_err(|source| Failure::Copy {
            path: from.to_path_buf(),
            source,
        })
}

fn sidecar(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(WAL_SUFFIX);
    PathBuf::from(name)
}
