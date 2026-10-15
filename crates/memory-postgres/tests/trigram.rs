use kernel::memory::Limit;
use kernel::memory::MemoryStore as _;
use kernel::memory::SearchMode;
use kernel::memory::SearchQuery;
use memory_postgres::PostgresMemory;
use sqlx::PgPool;
use wire::gateway::memory::MemoryHitSource;
use wire::gateway::memory::MemoryScope;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const QUERY: &str = "herdr socket";
const CONTENT: &str = "weather stays mild overnight";
const TOOL_RESULT: &str =
    "checked the herdr socket path and the graphile queue depth before restarting the worker";
const SUMMARY: &str = "the operator checked the herdr socket path and the graphile queue depth before restarting the worker process";
const DECOY: &str = "calendar holds the on-call rotation";

fn search(text: &str, scope: MemoryScope) -> SearchQuery {
    SearchQuery {
        since: None,
        before: None,
        window: None,
        expand: None,
        synthesize: None,
        text: text.to_string(),
        mode: SearchMode::Trigram,
        scope,
        agent: None,
        tag: None,
        limit: Limit::clamped(Some(10), 20, 1..=50),
    }
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_short_query_inside_a_longer_tool_result_is_found(pool: PgPool) -> TestResult {
    assert!(TOOL_RESULT.contains(QUERY));
    assert!(TOOL_RESULT.chars().count() > QUERY.chars().count());
    assert!(!CONTENT.contains(QUERY));
    assert!(SUMMARY.contains(QUERY));
    assert!(SUMMARY.chars().count() > QUERY.chars().count());

    let mut tx = testkit::capture_tx(&pool).await?;
    sqlx::query(
        "INSERT INTO rh_conversations (session_key, agent, channel)
         VALUES ('trigram-tool', 'rivet', 'test')",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO rh_messages (conversation_id, agent, channel, role, content, tool_name, tool_result)
         SELECT id, 'rivet', 'test', 'tool', $1, 'shell', $2
           FROM rh_conversations WHERE session_key = 'trigram-tool'",
    )
    .bind(CONTENT)
    .bind(TOOL_RESULT)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO rh_messages (conversation_id, agent, channel, role, content, tool_result)
         SELECT id, 'rivet', 'test', 'assistant', $1, 'notebook pages stay blank'
           FROM rh_conversations WHERE session_key = 'trigram-tool'",
    )
    .bind(DECOY)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO rh_summaries (conversation_id, content, kind, depth, message_count)
         SELECT id, $1, 'leaf', 0, 1
           FROM rh_conversations WHERE session_key = 'trigram-tool'",
    )
    .bind(SUMMARY)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let memory = PostgresMemory::new(pool);
    let messages = memory.search(search(QUERY, MemoryScope::Messages)).await?;
    let message_text: Vec<&str> = messages
        .results
        .iter()
        .map(|hit| hit.content.as_str())
        .collect();
    assert_eq!(message_text, vec![CONTENT]);
    assert!(
        messages
            .results
            .iter()
            .all(|hit| hit.source == MemoryHitSource::Message)
    );

    let summaries = memory.search(search(QUERY, MemoryScope::Summaries)).await?;
    let summary_text: Vec<&str> = summaries
        .results
        .iter()
        .map(|hit| hit.content.as_str())
        .collect();
    assert_eq!(summary_text, vec![SUMMARY]);
    assert!(
        summaries
            .results
            .iter()
            .all(|hit| hit.source == MemoryHitSource::Summary)
    );
    Ok(())
}
