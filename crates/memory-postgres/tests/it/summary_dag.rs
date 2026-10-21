#![cfg(test)]

use chrono::DateTime;
use chrono::Utc;
use kernel::memory::MemoryStore as _;
use memory_postgres::PostgresMemory;
use sqlx::PgPool;
use wire::gateway::memory::SummaryKind;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SEED: &[&str] = &[
    "insert into rh_conversations (id, session_key, agent, channel, created_at, updated_at) values
      ('00000000-0000-4000-8000-000000000001', 'cli:dag', 'rivet', 'cli', '2026-10-10T12:00:00Z', '2026-10-10T13:00:00Z'),
      ('00000000-0000-4000-8000-000000000002', 'cli:dag', 'helper', 'cli', '2026-10-10T11:00:00Z', '2026-10-10T11:30:00Z')",
    "insert into rh_messages (id, conversation_id, agent, channel, role, content, created_at) values
      ('00000000-0000-4000-8000-000000000101', '00000000-0000-4000-8000-000000000001', 'rivet', 'cli', 'user', 'one', '2026-10-10T12:00:00Z'),
      ('00000000-0000-4000-8000-000000000102', '00000000-0000-4000-8000-000000000001', 'rivet', 'cli', 'assistant', 'two', '2026-10-10T12:01:00Z')",
    "insert into rh_summaries (id, conversation_id, depth, content, kind, message_count, earliest_at, latest_at, created_at) values
      ('00000000-0000-4000-8000-000000000014', '00000000-0000-4000-8000-000000000001', 1, 'branch', 'branch', 2, '2026-10-10T12:00:00Z', '2026-10-10T12:01:00Z', '2026-10-10T12:30:00Z'),
      ('00000000-0000-4000-8000-000000000011', '00000000-0000-4000-8000-000000000001', 0, 'first', 'leaf', 1, '2026-10-10T12:00:00Z', '2026-10-10T12:00:00Z', '2026-10-10T12:30:00Z'),
      ('00000000-0000-4000-8000-000000000012', '00000000-0000-4000-8000-000000000001', 0, 'second', 'leaf', 1, '2026-10-10T12:01:00Z', '2026-10-10T12:01:00Z', '2026-10-10T12:30:00Z'),
      ('00000000-0000-4000-8000-000000000015', '00000000-0000-4000-8000-000000000002', 0, 'stale', 'leaf', 0, '2026-10-10T11:00:00Z', '2026-10-10T11:00:00Z', '2026-10-10T11:30:00Z')",
    "update rh_summaries set parent_id = '00000000-0000-4000-8000-000000000014'
      where id in ('00000000-0000-4000-8000-000000000011', '00000000-0000-4000-8000-000000000012')",
    "insert into rh_summary_sources (summary_id, message_id, ordinal) values
      ('00000000-0000-4000-8000-000000000011', '00000000-0000-4000-8000-000000000101', 0),
      ('00000000-0000-4000-8000-000000000012', '00000000-0000-4000-8000-000000000102', 0)",
];

async fn seed(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("select set_config('rivet.defer_embed_enqueue', 'on', true)")
        .execute(&mut *tx)
        .await?;
    for statement in SEED.iter().copied() {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("timestamp")
        .with_timezone(&Utc)
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn the_summary_dag_walks_from_the_session_leaves_to_the_root(pool: PgPool) -> TestResult {
    seed(&pool).await?;
    let memory = PostgresMemory::new(pool);
    let nodes = memory.summary_dag("cli:dag").await?;
    let shape: Vec<_> = nodes
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                node.parent_id.as_deref(),
                node.level,
                node.kind,
            )
        })
        .collect();
    let branch = "00000000-0000-4000-8000-000000000014";
    assert_eq!(
        shape,
        vec![
            (branch, None, 1, SummaryKind::Branch),
            (
                "00000000-0000-4000-8000-000000000011",
                Some(branch),
                0,
                SummaryKind::Leaf
            ),
            (
                "00000000-0000-4000-8000-000000000012",
                Some(branch),
                0,
                SummaryKind::Leaf
            ),
        ]
    );
    let first = nodes.get(1).expect("first leaf");
    assert_eq!(
        first.source_message_ids,
        vec!["00000000-0000-4000-8000-000000000101"]
    );
    assert_eq!(first.message_count, 1);
    assert_eq!(first.content, "first");
    assert_eq!(first.earliest_at, Some(at("2026-10-10T12:00:00Z")));
    assert_eq!(first.created_at, at("2026-10-10T12:30:00Z"));
    assert_eq!(nodes.first().expect("branch").source_message_ids.len(), 0);
    assert_eq!(memory.summary_dag("cli:missing").await?.len(), 0);
    Ok(())
}
