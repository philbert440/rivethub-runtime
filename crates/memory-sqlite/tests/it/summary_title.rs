use crate::common;

use common::TestResult;
use common::fresh_store;
use common::instant;
use kernel::memory::NewSummary;
use kernel::memory::SummaryKind;
use kernel::memory::SummaryStore;
use kernel::memory::SummaryWrite;
use memory_sqlite::SqliteStore;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use uuid::Uuid;

const FIRST_LEAF: &str =
    "Investigated the billing timeout on the payment host. Root cause was pool exhaustion.";
const SECOND_LEAF: &str = "A second leaf about weekend gardening plans.";

#[tokio::test]
async fn the_first_leaf_titles_an_untitled_conversation_once() -> TestResult {
    let fresh = fresh_store(None)?;
    let conversation = Uuid::new_v4();
    seed_conversation(&fresh.store, conversation, None).await?;
    let first = fresh
        .store
        .write_summary(leaf(conversation, FIRST_LEAF, "2026-10-10T12:00:00Z")?)
        .await?;
    assert_eq!(first, SummaryWrite::FirstLeaf);
    assert_eq!(
        title(&fresh.store, conversation).await?.as_deref(),
        Some("Investigated the billing timeout on the payment host.")
    );
    let second = fresh
        .store
        .write_summary(leaf(conversation, SECOND_LEAF, "2026-10-10T13:00:00Z")?)
        .await?;
    assert_eq!(second, SummaryWrite::Added);
    assert_eq!(
        title(&fresh.store, conversation).await?.as_deref(),
        Some("Investigated the billing timeout on the payment host.")
    );
    Ok(())
}

#[tokio::test]
async fn a_harness_title_is_never_replaced_by_a_summary() -> TestResult {
    let fresh = fresh_store(None)?;
    let conversation = Uuid::new_v4();
    seed_conversation(&fresh.store, conversation, Some("Named by the hook")).await?;
    fresh
        .store
        .write_summary(leaf(conversation, FIRST_LEAF, "2026-10-10T12:00:00Z")?)
        .await?;
    assert_eq!(
        title(&fresh.store, conversation).await?.as_deref(),
        Some("Named by the hook")
    );
    Ok(())
}

#[tokio::test]
async fn a_markup_only_leaf_leaves_the_title_for_a_later_leaf() -> TestResult {
    let fresh = fresh_store(None)?;
    let conversation = Uuid::new_v4();
    seed_conversation(&fresh.store, conversation, Some("  ")).await?;
    fresh
        .store
        .write_summary(leaf(conversation, "### ", "2026-10-10T12:00:00Z")?)
        .await?;
    assert_eq!(
        title(&fresh.store, conversation).await?.as_deref(),
        Some("  ")
    );
    fresh
        .store
        .write_summary(leaf(conversation, SECOND_LEAF, "2026-10-10T13:00:00Z")?)
        .await?;
    assert_eq!(
        title(&fresh.store, conversation).await?.as_deref(),
        Some("A second leaf about weekend gardening plans.")
    );
    Ok(())
}

#[tokio::test]
async fn the_backfill_titles_summarized_conversations_from_their_earliest_leaf() -> TestResult {
    let fresh = fresh_store(None)?;
    let untitled = Uuid::new_v4();
    let named = Uuid::new_v4();
    let bare = Uuid::new_v4();
    seed_conversation(&fresh.store, untitled, None).await?;
    seed_conversation(&fresh.store, named, Some("Kept")).await?;
    seed_conversation(&fresh.store, bare, None).await?;
    raw_leaf(
        &fresh.store,
        untitled,
        SECOND_LEAF,
        "2026-10-10T13:00:00.000Z",
    )
    .await?;
    raw_leaf(
        &fresh.store,
        untitled,
        FIRST_LEAF,
        "2026-10-10T12:00:00.000Z",
    )
    .await?;
    raw_leaf(&fresh.store, named, FIRST_LEAF, "2026-10-10T12:00:00.000Z").await?;
    assert_eq!(fresh.store.backfill_titles(true).await?, 1);
    assert_eq!(title(&fresh.store, untitled).await?, None);
    assert_eq!(fresh.store.backfill_titles(false).await?, 1);
    assert_eq!(
        title(&fresh.store, untitled).await?.as_deref(),
        Some("Investigated the billing timeout on the payment host.")
    );
    assert_eq!(title(&fresh.store, named).await?.as_deref(), Some("Kept"));
    assert_eq!(title(&fresh.store, bare).await?, None);
    assert_eq!(fresh.store.backfill_titles(false).await?, 0);
    Ok(())
}

fn leaf(conversation: Uuid, content: &str, at: &str) -> TestResult<NewSummary> {
    let at = instant(at)?;
    Ok(NewSummary {
        id: Uuid::new_v4(),
        conversation_id: conversation,
        kind: SummaryKind::Leaf,
        depth: SummaryKind::Leaf.depth(),
        content: content.to_owned(),
        message_count: 0,
        earliest_at: at,
        latest_at: at,
        model: "fake-chat".to_owned(),
        pipeline_version: 5,
        created_at: at,
        sources: Vec::new(),
        child_ids: Vec::new(),
    })
}

async fn seed_conversation(
    store: &SqliteStore,
    id: Uuid,
    title: Option<&'static str>,
) -> TestResult {
    store
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO ros_conversations \
                    (id, session_key, agent, channel, title, settings, active, created_at, updated_at) \
                 VALUES (?1, ?1, 'rivet', 'cli', ?2, '{}', 1, ?3, ?3)",
                params![id.to_string(), title, common::RECORDED_START],
            )
            .map_err(common::seed)?;
            Ok(())
        })
        .await?;
    Ok(())
}

async fn raw_leaf(store: &SqliteStore, conversation: Uuid, content: &str, at: &str) -> TestResult {
    let content = content.to_owned();
    let at = at.to_owned();
    store
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO ros_summaries \
                    (id, conversation_id, depth, content, kind, message_count, earliest_at, \
                     latest_at, model, created_at, pipeline_version) \
                 VALUES (?1, ?2, 0, ?3, 'leaf', 0, ?4, ?4, 'fake-chat', ?4, 5)",
                params![
                    Uuid::new_v4().to_string(),
                    conversation.to_string(),
                    content,
                    at
                ],
            )
            .map_err(common::seed)?;
            Ok(())
        })
        .await?;
    Ok(())
}

async fn title(store: &SqliteStore, id: Uuid) -> TestResult<Option<String>> {
    let found = store
        .with_conn(move |conn| {
            conn.query_row(
                "SELECT title FROM ros_conversations WHERE id = ?1",
                params![id.to_string()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(common::seed)
        })
        .await?;
    Ok(found.ok_or("conversation")?)
}
