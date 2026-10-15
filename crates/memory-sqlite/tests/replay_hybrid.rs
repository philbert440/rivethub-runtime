mod search_support;

use chrono::{DateTime, Utc};
use kernel::memory::{SearchMode, SearchOutcome, SearchQuery};
use search_support::{AccessRow, Case, Options, Recording, TestResult};
use std::sync::Arc;
use testkit::fakes::FakeProvider;

#[tokio::test]
async fn hybrid_vectors_replay() -> TestResult {
    let recording: Recording = serde_json::from_slice(&std::fs::read(testkit::recording(
        "memory-sqlite/search.json",
    ))?)?;
    let hybrid = recording.hybrid;
    let (_dir, store) = search_support::store(Some(Arc::new(FakeProvider::fnv_embeddings(8))))?;
    search_support::seed(&store, &hybrid).await?;
    for case in hybrid.memory {
        replay(&store, hybrid.clock, case).await?;
    }
    for case in hybrid.backend {
        if let Some(error) = case.error {
            let rejected = request(&case.query, case.options).expect_err("invalid tag");
            assert!(rejected.to_string().starts_with(&error.message));
        } else {
            let response = case.response.ok_or("missing response")?;
            replay(
                &store,
                hybrid.clock,
                Case {
                    query: case.query,
                    options: case.options,
                    hits: response.results,
                    degraded: response.degraded,
                },
            )
            .await?;
        }
    }
    for case in hybrid.literal_and_degraded {
        replay(&store, hybrid.clock, case).await?;
    }
    assert_eq!(access(&store, false).await?, hybrid.access_after);
    assert_eq!(access(&store, true).await?, hybrid.summary_access_after);
    Ok(())
}

fn request(text: &str, options: Options) -> TestResult<SearchQuery> {
    let mut query = search_support::query(text, SearchMode::Hybrid);
    if let Some(scope) = options.scope {
        query.scope = scope;
    }
    query.agent = options.agent;
    query.tag = options.tag.map(|tag| tag.parse()).transpose()?;
    if let Some(limit) = options.limit {
        query.limit = kernel::memory::Limit::clamped(Some(limit), 10, 1..=50);
    }
    Ok(query)
}

async fn replay(
    store: &memory_sqlite::SqliteStore,
    clock: DateTime<Utc>,
    case: Case,
) -> TestResult {
    let store = if case.degraded.is_some() {
        store
            .clone()
            .with_query_embed(memory_sqlite::QueryEmbed::new(
                Arc::new(FakeProvider::http_errors(500)),
                kernel::memory_core::DEFAULT_EMBED_QUERY_INSTRUCTION.to_owned(),
                std::time::Duration::from_secs(1),
                tokio_util::sync::CancellationToken::new(),
            ))
    } else {
        store.clone()
    };
    let actual = store
        .search_at(request(&case.query, case.options)?, clock, &Utc)
        .await?;
    assert_eq!(
        actual.degraded.is_some(),
        case.degraded.is_some(),
        "{}",
        case.query
    );
    compare(&case.query, &actual, case.hits);
    Ok(())
}

fn compare(text: &str, actual: &SearchOutcome, expected: Vec<search_support::Hit>) {
    assert_eq!(
        actual.results.iter().map(|hit| &hit.id).collect::<Vec<_>>(),
        expected.iter().map(|hit| &hit.id).collect::<Vec<_>>(),
        "{text}: {:?}",
        actual.results
    );
    for (actual, expected) in actual.results.iter().zip(expected) {
        assert_eq!(actual.source, expected.source);
        match expected.role {
            search_support::HitRole::Message(role) => assert_eq!(actual.role, Some(role)),
            search_support::HitRole::Summary(kind) => {
                assert!(matches!(
                    kind,
                    kernel::memory::SummaryKind::Leaf | kernel::memory::SummaryKind::Branch
                ));
                assert_eq!(actual.role, None);
            }
        }
        assert_eq!(actual.content, expected.content);
        assert_eq!(actual.agent.as_deref(), Some(expected.agent.as_str()));
        assert_eq!(actual.created_at, expected.created_at);
        assert!(
            (actual.relevance_score - expected.relevance_score).abs() < 1e-12,
            "{text} {}: {} != {}",
            actual.id,
            actual.relevance_score,
            expected.relevance_score
        );
    }
}

async fn access(store: &memory_sqlite::SqliteStore, summary: bool) -> TestResult<Vec<AccessRow>> {
    Ok(store
        .with_conn(move |conn| {
            let table = if summary {
                "ros_summaries"
            } else {
                "ros_messages"
            };
            let mut statement = conn
                .prepare(&format!(
                    "SELECT id,access_count,last_accessed_at FROM {table} ORDER BY rowid"
                ))
                .map_err(search_support::failed)?;
            statement
                .query_map([], |row| {
                    Ok(AccessRow {
                        id: row.get(0)?,
                        access_count: row.get(1)?,
                        last_accessed_at: row.get(2)?,
                    })
                })
                .map_err(search_support::failed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(search_support::failed)
        })
        .await?)
}
