pub mod capture;
mod conversation;
pub mod embed;
mod embedding_store;
pub mod error;
mod graphile;
pub mod project;
pub mod queue;
pub mod read;
pub mod search;
mod search_sql;
mod store_impl;
mod tags;
mod wiki;

use std::cmp::Ordering;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering as AtomicOrdering;
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
pub use graphile::GraphileQueue;
pub use queue::PostgresQueue;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub const SCHEMA_VERSION: i64 = 21;

#[derive(Debug)]
pub struct PostgresMemory {
    pool: PgPool,
    wiki_available: AtomicBool,
    embed: Option<embed::EmbedArm>,
}

impl PostgresMemory {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            wiki_available: AtomicBool::new(true),
            embed: None,
        }
    }

    #[must_use]
    pub fn with_query_embed(mut self, embed: QueryEmbed) -> Self {
        self.embed = Some(embed::EmbedArm::new(embed));
        self
    }

    pub async fn connect(url: &str) -> Result<Self, Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(30))
            .connect(url)
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

    pub async fn queue(&self) -> Result<PostgresQueue, Error> {
        PostgresQueue::connect(self.pool.clone()).await
    }

    pub async fn queue_available(&self) -> Result<bool, Error> {
        Ok(self.queue().await?.available())
    }

    #[must_use]
    pub fn wiki_available(&self) -> bool {
        self.wiki_available.load(AtomicOrdering::Relaxed)
    }

    pub(crate) fn set_wiki_available(&self, available: bool) {
        self.wiki_available
            .store(available, AtomicOrdering::Relaxed);
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
