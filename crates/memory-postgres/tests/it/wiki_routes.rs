use kernel::wiki::builder::ExtractWiki;
use memory_postgres::{PostgresMemory, PostgresQueue};
use sqlx::PgPool;
use testkit::topic_pages::PageStores;
use testkit::wiki_routes;
use testkit::wiki_routes::RouteStores;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Seeded {
    _directory: tempfile::TempDir,
    memory: PostgresMemory,
    queue: PostgresQueue,
    job: ExtractWiki,
}

impl Seeded {
    async fn new(pool: PgPool) -> TestResult<Self> {
        testkit::install_graphile_worker(&pool).await?;
        let directory = tempfile::tempdir()?;
        let conversation = Uuid::new_v4();
        let summary = Uuid::new_v4();
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL rivet.defer_embed_enqueue = 'on'")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO rh_conversations (id, session_key, agent) VALUES ($1, $2, 'rivet')",
        )
        .bind(conversation)
        .bind(wiki_routes::SESSION_KEY)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO rh_summaries (id, conversation_id, content, kind) VALUES ($1, $2, 'AcmeKit notes.', 'leaf')",
        )
        .bind(summary)
        .bind(conversation)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        let memory = PostgresMemory::new(pool).with_wiki_directory(directory.path().join("wiki"));
        let queue = memory.queue().await?;
        Ok(Self {
            _directory: directory,
            memory,
            queue,
            job: ExtractWiki {
                summary_id: summary,
                conversation_id: Some(conversation),
            },
        })
    }

    fn stores(&self) -> RouteStores<'_> {
        RouteStores {
            pages: PageStores {
                topics: &self.memory,
                pages: &self.memory,
                wiki: &self.memory,
                queue: &self.queue,
            },
            wiki: &self.memory,
        }
    }
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_placed_session_writes_its_topic_page_and_no_other_topics_page(
    pool: PgPool,
) -> TestResult {
    let seeded = Seeded::new(pool).await?;
    wiki_routes::a_placed_session_writes_its_topic_page_and_no_other_topics_page(
        seeded.stores(),
        &seeded.job,
    )
    .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn the_topic_page_is_cited_without_a_proposal(pool: PgPool) -> TestResult {
    let seeded = Seeded::new(pool).await?;
    wiki_routes::the_topic_page_is_cited_without_a_proposal(seeded.stores(), &seeded.job).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_host_is_filed_as_a_section_under_hosts(pool: PgPool) -> TestResult {
    let seeded = Seeded::new(pool).await?;
    wiki_routes::a_host_is_filed_as_a_section_under_hosts(seeded.stores(), &seeded.job).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_sub_topic_summary_lands_in_its_section(pool: PgPool) -> TestResult {
    let seeded = Seeded::new(pool).await?;
    wiki_routes::a_sub_topic_summary_lands_in_its_section(seeded.stores(), &seeded.job).await
}
