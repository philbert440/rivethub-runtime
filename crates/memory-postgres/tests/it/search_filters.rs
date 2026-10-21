use std::collections::BTreeSet;

use kernel::memory::{Limit, MemoryStore, SearchMode, SearchQuery};
use memory_postgres::PostgresMemory;
use sqlx::PgPool;
use wire::gateway::memory::{MemoryHitSource, MemoryScope};
use wire::memory::MemoryEntry;
use wire::message::Role;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const TEXT: &str = "backup disk";
const SUMMARY: &str = "The nightly backup disk filled up until the old snapshots were pruned.";
const MODES: [SearchMode; 2] = [SearchMode::Fts, SearchMode::Regex];

fn entry(
    session: &str,
    agent: &str,
    role: Role,
    tool: Option<&str>,
    content: &str,
) -> TestResult<MemoryEntry> {
    Ok(MemoryEntry {
        id: None,
        session_id: session.into(),
        agent: agent.into(),
        channel: "test".into(),
        role,
        content: content.into(),
        tool_name: tool.map(str::to_owned),
        tool_args: None,
        tool_result: tool.map(|_| content.to_owned()),
        metadata: None,
        created_at: Some("2026-10-12T00:00:00Z".parse()?),
    })
}

async fn seed(pool: &PgPool) -> TestResult<PostgresMemory> {
    let memory = PostgresMemory::new(pool.clone());
    memory
        .append(vec![
            entry(
                "filters-ops",
                "rivet",
                Role::User,
                None,
                "the nightly backup disk is full and the job stopped",
            )?,
            entry(
                "filters-ops",
                "rivet",
                Role::Assistant,
                None,
                "prune the oldest snapshots on the backup disk before the next run",
            )?,
            entry(
                "filters-ops",
                "rivet",
                Role::Tool,
                Some("Bash"),
                "df -h reports the backup disk at one hundred percent used",
            )?,
            entry(
                "filters-ops",
                "rivet",
                Role::Tool,
                Some("Read"),
                "backup.yaml keeps thirty snapshots on the backup disk volume",
            )?,
            entry(
                "filters-side",
                "helper",
                Role::Tool,
                Some("Bash"),
                "du -sh lists the backup disk usage per snapshot folder",
            )?,
        ])
        .await?;
    let mut tx = testkit::capture_tx(pool).await?;
    sqlx::query("INSERT INTO rh_summaries (conversation_id, content, kind, depth, message_count, created_at) SELECT id, $1, 'leaf', 0, 4, '2026-10-12T00:00:00Z' FROM rh_conversations WHERE session_key = 'filters-ops'")
        .bind(SUMMARY)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(memory)
}

fn query(mode: SearchMode) -> SearchQuery {
    SearchQuery {
        text: TEXT.into(),
        mode,
        scope: MemoryScope::Both,
        agent: None,
        role: None,
        tool_name: None,
        tag: None,
        limit: Limit::clamped(Some(20), 10, 1..=50),
        since: None,
        before: None,
        window: None,
        expand: Some(false),
        synthesize: Some(false),
    }
}

async fn hits(memory: &PostgresMemory, query: SearchQuery) -> TestResult<BTreeSet<String>> {
    let outcome = memory.search(query).await?;
    Ok(outcome
        .results
        .into_iter()
        .map(|hit| match hit.source {
            MemoryHitSource::Summary => "summary".to_owned(),
            MemoryHitSource::Message => first_word(&hit.content),
        })
        .collect())
}

fn first_word(content: &str) -> String {
    content
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn set(expected: &[&str]) -> BTreeSet<String> {
    expected.iter().map(|hit| (*hit).to_owned()).collect()
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn role_and_tool_filters_narrow_message_hits_and_keep_summaries(pool: PgPool) -> TestResult {
    let memory = seed(&pool).await?;
    for mode in MODES {
        let mut search = query(mode);
        search.role = Some(Role::User);
        assert_eq!(
            hits(&memory, search).await?,
            set(&["the", "summary"]),
            "{mode}"
        );
        let mut search = query(mode);
        search.role = Some(Role::Tool);
        search.scope = MemoryScope::Messages;
        assert_eq!(
            hits(&memory, search).await?,
            set(&["df", "backup.yaml", "du"]),
            "{mode}"
        );
        let mut search = query(mode);
        search.tool_name = Some("Bash".into());
        assert_eq!(
            hits(&memory, search).await?,
            set(&["df", "du", "summary"]),
            "{mode}"
        );
        let mut search = query(mode);
        search.tool_name = Some("Bash".into());
        search.agent = Some("helper".into());
        search.scope = MemoryScope::Messages;
        assert_eq!(hits(&memory, search).await?, set(&["du"]), "{mode}");
    }
    Ok(())
}
