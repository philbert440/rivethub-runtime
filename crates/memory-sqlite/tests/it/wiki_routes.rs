use kernel::wiki::builder::ExtractWiki;
use memory_sqlite::SqliteStore;
use tempfile::TempDir;
use testkit::topic_pages::PageStores;
use testkit::wiki_routes;
use testkit::wiki_routes::RouteStores;
use uuid::Uuid;

use crate::common::TestResult;
use crate::common::execute;

const CONVERSATION: &str = "00000000-0000-4000-8000-0000000000c1";
const SUMMARY: &str = "00000000-0000-4000-8000-0000000000d1";

async fn seeded() -> TestResult<(TempDir, SqliteStore, ExtractWiki)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("memory.sqlite");
    migrate::sqlite(&path)?;
    let store = SqliteStore::open(&path)?;
    execute(
        &store,
        &format!(
            "INSERT INTO ros_conversations (id, session_key, agent, created_at, updated_at)
             VALUES ('{CONVERSATION}', '{}', 'rivet', '2026-10-10T12:00:00.000Z', '2026-10-10T12:00:00.000Z');
             INSERT INTO ros_summaries (id, conversation_id, content, created_at)
             VALUES ('{SUMMARY}', '{CONVERSATION}', 'AcmeKit notes.', '2026-10-10T12:00:00.000Z');",
            wiki_routes::SESSION_KEY
        ),
    )
    .await?;
    let job = ExtractWiki {
        summary_id: Uuid::parse_str(SUMMARY)?,
        conversation_id: Some(Uuid::parse_str(CONVERSATION)?),
    };
    Ok((directory, store, job))
}

fn stores(store: &SqliteStore) -> RouteStores<'_> {
    RouteStores {
        pages: PageStores {
            topics: store,
            pages: store,
            wiki: store,
            queue: store,
        },
        wiki: store,
    }
}

#[tokio::test]
async fn a_placed_session_writes_its_topic_page_and_no_other_topics_page() -> TestResult {
    let (_directory, store, job) = seeded().await?;
    wiki_routes::a_placed_session_writes_its_topic_page_and_no_other_topics_page(
        stores(&store),
        &job,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn the_topic_page_is_cited_without_a_proposal() -> TestResult {
    let (_directory, store, job) = seeded().await?;
    wiki_routes::the_topic_page_is_cited_without_a_proposal(stores(&store), &job).await?;
    Ok(())
}

#[tokio::test]
async fn a_host_is_filed_as_a_section_under_hosts() -> TestResult {
    let (_directory, store, job) = seeded().await?;
    wiki_routes::a_host_is_filed_as_a_section_under_hosts(stores(&store), &job).await?;
    Ok(())
}

#[tokio::test]
async fn a_sub_topic_summary_lands_in_its_section() -> TestResult {
    let (_directory, store, job) = seeded().await?;
    wiki_routes::a_sub_topic_summary_lands_in_its_section(stores(&store), &job).await?;
    Ok(())
}
