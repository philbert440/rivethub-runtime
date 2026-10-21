use chrono::DateTime;
use chrono::Utc;
use memory_sqlite::SqliteError;
use memory_sqlite::SqliteStore;

pub(crate) const OR_SUPERSEDES_AND: &str = "the TypeScript ANDed every query token, so a multi-word query matched only rows holding all of them; the lexical arm now ORs the tokens and bm25 ranks the rows holding more of them first";

const REINFORCE_SQL: [&str; 2] = [
    "UPDATE ros_messages SET access_count = access_count + 1, last_accessed_at = ?1 WHERE id = ?2",
    "UPDATE ros_summaries SET access_count = access_count + 1, last_accessed_at = ?1 WHERE id = ?2",
];

pub(crate) struct Superseded {
    pub(crate) query: &'static str,
    pub(crate) hits: &'static [&'static str],
}

pub(crate) fn find(table: &'static [Superseded], text: &str) -> Option<&'static Superseded> {
    table.iter().find(|case| case.query == text)
}

pub(crate) async fn reinforce(
    store: &SqliteStore,
    ids: Vec<String>,
    clock: DateTime<Utc>,
) -> Result<(), SqliteError> {
    let stamp = clock.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    store
        .with_conn(move |conn| {
            for id in &ids {
                for sql in REINFORCE_SQL {
                    conn.execute(sql, rusqlite::params![stamp, id])
                        .map_err(|source| SqliteError::Query {
                            operation: "replay a recorded access bump",
                            source,
                        })?;
                }
            }
            Ok(())
        })
        .await
}
