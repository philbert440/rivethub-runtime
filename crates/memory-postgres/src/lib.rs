mod access;
pub mod admin;
pub mod capture;
mod conversation;
pub mod dump;
pub mod embed;
mod embedding_store;
pub mod error;
mod graphile;
mod graphile_sql;
mod listen;
mod loss;
mod memory_jobs;
pub mod project;
pub mod queue;
pub mod read;
pub mod search;
mod search_complete;
mod search_details;
mod search_sql;
pub use search_complete::SearchSynthesis;
mod store_impl;
mod summary_store;
mod tags;
mod task_db;
mod task_jobs;
mod task_query;
mod task_rows;
mod task_watch;
mod wiki;
mod wiki_builder;

#[cfg(test)]
mod resilience;

pub use task_db::PostgresTasks;

use std::cmp::Ordering;
use std::time::Duration;

pub use capture::CaptureOptions;
pub use capture::CaptureWriter;
pub use capture::ConversationEnsure;
pub use capture::ConversationRecord;
pub use capture::ConversationUpdate;
pub use capture::ConversationUpsert;
pub use capture::MessageRow;
pub use embed::QueryEmbed;
pub use error::Error;
use graphile::GraphileQueue;
pub use queue::PostgresQueue;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub const SCHEMA_VERSION: i64 = 23;
pub(crate) const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub struct PostgresMemory {
    wiki_directory: Option<std::path::PathBuf>,
    pool: PgPool,
    embed: Option<embed::EmbedArm>,
    synthesis: Option<SearchSynthesis>,
    access: std::sync::Arc<access::AccessLog>,
}

impl PostgresMemory {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            wiki_directory: None,
            pool,
            embed: None,
            synthesis: None,
            access: std::sync::Arc::default(),
        }
    }

    #[must_use]
    pub fn with_wiki_directory(mut self, directory: std::path::PathBuf) -> Self {
        self.wiki_directory = Some(directory);
        self
    }

    #[must_use]
    pub fn with_query_embed(mut self, embed: QueryEmbed) -> Self {
        self.embed = Some(embed::EmbedArm::new(embed));
        self
    }

    #[must_use]
    pub fn with_search_synthesis(mut self, synthesis: SearchSynthesis) -> Self {
        self.synthesis = Some(synthesis);
        self
    }

    pub async fn connect(url: &str) -> Result<Self, Error> {
        let pool = open_pool(url, 5, Some(Duration::from_secs(30)))
            .await
            .map_err(Error::Connection)?;
        Self::prepare(&pool).await?;
        Ok(Self::new(pool))
    }

    pub async fn prepare(pool: &PgPool) -> Result<(), Error> {
        let found = newest_applied(pool).await?;
        match found.cmp(&SCHEMA_VERSION) {
            Ordering::Equal => Ok(()),
            Ordering::Greater => {
                tracing::warn!(
                    "the memory schema reports version {found}, this build requires {SCHEMA_VERSION} — rolling updates migrate first; continuing"
                );
                Ok(())
            }
            Ordering::Less => Err(Error::SchemaBehind {
                found,
                required: SCHEMA_VERSION,
            }),
        }
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub(crate) async fn ping(&self) -> Result<(), Error> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|source| Error::operation("probe", source))
    }

    pub async fn queue(&self) -> Result<PostgresQueue, Error> {
        PostgresQueue::connect(self.pool.clone()).await
    }

    pub async fn queue_available(&self) -> Result<bool, Error> {
        GraphileQueue::schema_present(&self.pool).await
    }
}

pub(crate) async fn open_pool(
    url: &str,
    max_connections: u32,
    idle_timeout: Option<Duration>,
) -> Result<PgPool, sqlx::Error> {
    const ATTEMPTS: u8 = 3;
    let mut attempt = 1;
    loop {
        let options = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .test_before_acquire(true);
        let options = match idle_timeout {
            Some(idle) => options.idle_timeout(idle),
            None => options,
        };
        match options.connect(url).await {
            Ok(pool) => return Ok(pool),
            Err(sqlx::Error::PoolTimedOut) if attempt < ATTEMPTS => {
                tracing::warn!(
                    "memory database connection timed out; retrying (attempt {attempt} of {ATTEMPTS})"
                );
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn newest_applied(pool: &PgPool) -> Result<i64, Error> {
    match sqlx::query!(
        "select coalesce(max(version), 0) as \"applied!\" from _sqlx_migrations where success"
    )
    .fetch_one(pool)
    .await
    {
        Ok(row) => Ok(row.applied),
        Err(source) if Error::relation_missing(&source) => Err(Error::NotAdopted),
        Err(source) => Err(Error::operation("schema version check", source)),
    }
}
