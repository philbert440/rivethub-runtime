use kernel::memory::SearchMode;
use sqlx::PgPool;
use wire::gateway::memory::MemoryScope;

use crate::search::SearchOptions;
use crate::search_sql::explain_arm;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn options() -> SearchOptions {
    SearchOptions {
        since: None,
        before: None,
        text: "herdr socket".to_string(),
        mode: SearchMode::Trigram,
        scope: MemoryScope::Both,
        limit: 5,
        agent: None,
        tag: None,
    }
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn trigram_arms_use_bitmap_index_scans(pool: PgPool) -> TestResult {
    let mut tx = testkit::capture_tx(&pool).await?;
    sqlx::query(
        "INSERT INTO rh_conversations (session_key, agent, channel)
         VALUES ('trigram-plan', 'rivet', 'test')",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO rh_messages (conversation_id, agent, channel, role, content, tool_name, tool_result)
         SELECT id, 'rivet', 'test', 'tool', 'weather stays mild overnight', 'shell',
                'checked the herdr socket path and the graphile queue depth before restarting the worker'
           FROM rh_conversations WHERE session_key = 'trigram-plan'",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO rh_summaries (conversation_id, content, kind, depth, message_count)
         SELECT id, 'the operator checked the herdr socket path and the graphile queue depth before restarting the worker process',
                'leaf', 0, 1
           FROM rh_conversations WHERE session_key = 'trigram-plan'",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    sqlx::query("ANALYZE rh_messages").execute(&pool).await?;
    sqlx::query("ANALYZE rh_summaries").execute(&pool).await?;
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT indexname FROM pg_indexes WHERE tablename = 'rh_summaries' ORDER BY 1",
    )
    .fetch_all(&pool)
    .await?;
    assert!(
        names.iter().any(|name| name == "idx_ros_summaries_trgm"),
        "{names:?}"
    );

    let mut tx = pool.begin().await?;
    let messages = explain_arm(&mut tx, &options(), true).await?;
    let summaries = explain_arm(&mut tx, &options(), false).await?;
    assert!(
        messages.contains("Bitmap Index Scan on idx_ros_messages_trgm"),
        "{messages}"
    );
    assert!(
        messages.contains("Bitmap Index Scan on idx_ros_messages_tool_result_trgm"),
        "{messages}"
    );
    assert!(
        !messages.to_ascii_lowercase().contains("seq scan"),
        "{messages}"
    );
    assert!(
        summaries.contains("Bitmap Index Scan on idx_ros_summaries_trgm"),
        "{summaries}"
    );
    assert!(
        !summaries.to_ascii_lowercase().contains("seq scan"),
        "{summaries}"
    );
    Ok(())
}
