pub mod capture;
mod conversation;
pub mod embed;
pub mod error;
mod graphile;
pub mod project;
pub mod queue;
pub mod read;
pub mod search;
mod search_sql;
mod store_impl;
mod tags;

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
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

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

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
        MIGRATOR.run(pool).await.map_err(Error::Migrate)?;
        let names: Vec<String> = MIGRATOR
            .migrations
            .iter()
            .map(|migration| tracked_name(migration.version, &migration.description))
            .collect();
        let tracked = sqlx::query!(
            "select name from _rivetos_migrations where name = any($1)",
            &names
        )
        .fetch_all(pool)
        .await
        .map_err(|source| Error::operation("migration tracking", source))?;
        let missing: Vec<String> = names
            .iter()
            .filter(|name| !tracked.iter().any(|row| row.name == **name))
            .cloned()
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        Err(Error::Schema {
            message: format!("_rivetos_migrations is missing {}", missing.join(", ")),
        })
    }

    pub async fn migrate(pool: &PgPool) -> Result<(), Error> {
        MIGRATOR.run(pool).await.map_err(Error::Migrate)
    }

    #[must_use]
    pub fn migrator() -> &'static sqlx::migrate::Migrator {
        &MIGRATOR
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
        self.wiki_available.load(Ordering::Relaxed)
    }

    pub(crate) fn set_wiki_available(&self, available: bool) {
        self.wiki_available.store(available, Ordering::Relaxed);
    }
}

#[must_use]
fn tracked_name(version: i64, description: &str) -> String {
    format!("{version:04}_{}.sql", description.replace(' ', "_"))
}

#[cfg(test)]
mod tests {
    use super::tracked_name;

    #[test]
    fn tracked_names_are_the_sql_file_names() {
        assert_eq!(tracked_name(1, "baseline"), "0001_baseline.sql");
        assert_eq!(tracked_name(2, "ros tasks"), "0002_ros_tasks.sql");
        assert_eq!(tracked_name(21, "rh tables"), "0021_rh_tables.sql");
    }
}
