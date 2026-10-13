use std::collections::BTreeSet;
use std::sync::Arc;

use kernel::memory::Limit;
use kernel::memory::SearchMode;
use kernel::memory::SearchQuery;
use kernel::model::Embedding;
use kernel::model::ModelProvider as _;
use memory_postgres::PostgresMemory;
use memory_postgres::QueryEmbed;
use pgvector::HalfVector;
use sqlx::PgPool;
use testkit::FakeProvider;
use testkit::Script;
use wire::capture::CaptureBatchRequest;
use wire::gateway::memory::MemoryScope;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WIDTH: usize = 1024;
const OLD: &str = "2026-01-01T10:00:00Z";

const BANANA_ONCE: &str = "banana cultivation notes from the AcmeKit greenhouse rotation";
const BANANA_THRICE: &str = "banana banana banana, the whole crate is bananas for smoothies";
const ROLLBACK: &str = "rollback drills monthly so the staging deploys always recover";
const DEPLOY: &str = "deploy scripts run nightly before the staging health check gate";

fn unwired(pool: &PgPool) -> PostgresMemory {
    PostgresMemory::new(pool.clone())
}

fn wired(pool: &PgPool, provider: FakeProvider) -> (PostgresMemory, Arc<FakeProvider>) {
    let provider = Arc::new(provider);
    let memory = PostgresMemory::new(pool.clone()).with_query_embed(QueryEmbed::new(
        provider.clone(),
        None,
        None,
        None,
    ));
    (memory, provider)
}

fn query(text: &str, mode: SearchMode) -> SearchQuery {
    SearchQuery {
        text: text.to_owned(),
        mode,
        scope: MemoryScope::Both,
        agent: None,
        tag: None,
        limit: Limit::clamped(Some(10), 20, 1..=50),
    }
}

fn contents(hits: &[memory_postgres::search::SearchHit]) -> Vec<String> {
    hits.iter().map(|hit| hit.content.clone()).collect()
}

async fn capture(pool: &PgPool, event_id: &str, content: &str) -> TestResult {
    let batch: CaptureBatchRequest = serde_json::from_value(serde_json::json!({
        "session_key": "claude-code:vector",
        "agent": "rivet",
        "channel": "claude-code",
        "messages": [{
            "event_id": event_id,
            "role": "user",
            "content": content,
            "created_at": OLD,
        }],
    }))?;
    unwired(pool)
        .capture_batch(&batch, &memory_postgres::CaptureOptions::default())
        .await?;
    Ok(())
}

async fn seed_four(pool: &PgPool) -> TestResult {
    for (event, content) in [
        ("banana-once", BANANA_ONCE),
        ("banana-thrice", BANANA_THRICE),
        ("rollback", ROLLBACK),
        ("deploy", DEPLOY),
    ] {
        capture(pool, event, content).await?;
    }
    Ok(())
}

fn event_of(content: &str) -> &'static str {
    match content {
        BANANA_ONCE => "banana-once",
        BANANA_THRICE => "banana-thrice",
        ROLLBACK => "rollback",
        _ => "deploy",
    }
}

async fn embed_row(pool: &PgPool, event_id: &str, vector: &[f32]) -> TestResult {
    sqlx::query("UPDATE rh_messages SET embedding = $1 WHERE metadata->>'event_id' = $2")
        .bind(HalfVector::from_f32_slice(vector))
        .bind(event_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn seed_fnv_embeddings(
    pool: &PgPool,
    provider: &FakeProvider,
    seeded: &[&str],
) -> TestResult {
    for content in seeded {
        let mut embeddings = provider.embed(vec![(*content).to_owned()]).await?;
        let vector = embeddings
            .pop()
            .ok_or("the fake returned no embedding")?
            .vector;
        embed_row(pool, event_of(content), &vector).await?;
    }
    Ok(())
}

async fn seed_all_four(pool: &PgPool, provider: &FakeProvider) -> TestResult {
    seed_fnv_embeddings(
        pool,
        provider,
        &[BANANA_ONCE, BANANA_THRICE, ROLLBACK, DEPLOY],
    )
    .await
}

#[sqlx::test]
async fn vector_mode_ranks_seeded_embeddings(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let seeder = FakeProvider::fnv_embeddings(WIDTH);
    seed_all_four(&pool, &seeder).await?;
    let (memory, _) = wired(&pool, FakeProvider::fnv_embeddings(WIDTH));

    let outcome = memory
        .search_outcome_for(&query("banana", SearchMode::Vector))
        .await?;
    let ranked: Vec<&str> = outcome
        .hits
        .iter()
        .map(|hit| hit.content.as_str())
        .collect();
    assert_eq!(
        ranked.first().copied(),
        Some(BANANA_THRICE),
        "the strongest banana vector ranks first: {ranked:?}"
    );
    assert_eq!(
        ranked.get(1).copied(),
        Some(BANANA_ONCE),
        "the single banana mention ranks second: {ranked:?}"
    );
    assert_eq!(ranked.len(), 4, "every embedded row surfaces: {ranked:?}");
    assert!(
        outcome
            .hits
            .first()
            .is_some_and(|hit| hit.score > 0.0 && hit.score < 1.0)
    );
    assert_eq!(outcome.degraded, None);
    assert_eq!(memory.vector_arm_dropped(), 0);
    Ok(())
}

#[sqlx::test]
async fn vector_mode_without_a_provider_searches_fts_silently(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let memory = unwired(&pool);
    let outcome = memory
        .search_outcome_for(&query("deploy", SearchMode::Vector))
        .await?;
    assert!(
        contents(&outcome.hits).contains(&DEPLOY.to_string()),
        "the fts fallback answers: {:?}",
        contents(&outcome.hits)
    );
    assert_eq!(outcome.degraded, None);
    assert_eq!(memory.vector_arm_dropped(), 0);
    Ok(())
}

#[sqlx::test]
async fn hybrid_without_a_provider_carries_no_degradation(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let memory = unwired(&pool);
    let outcome = memory
        .search_outcome_for(&query("deploy", SearchMode::Hybrid))
        .await?;
    assert!(
        contents(&outcome.hits).contains(&DEPLOY.to_string()),
        "hybrid ranks the seeded data: {:?}",
        contents(&outcome.hits)
    );
    assert_eq!(outcome.degraded, None);
    assert_eq!(memory.vector_arm_dropped(), 0);
    Ok(())
}

#[sqlx::test]
async fn hybrid_with_a_failing_provider_degrades_and_counts(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let (memory, _) = wired(
        &pool,
        FakeProvider::new(vec![
            Script::EmbedHttpStatus(503),
            Script::EmbedHttpStatus(503),
        ]),
    );
    let outcome = memory
        .search_outcome_for(&query("deploy", SearchMode::Hybrid))
        .await?;
    assert!(
        contents(&outcome.hits).contains(&DEPLOY.to_string()),
        "the text arms still answer: {:?}",
        contents(&outcome.hits)
    );
    assert_eq!(outcome.degraded.as_deref(), Some("http 503"));
    assert_eq!(memory.vector_arm_dropped(), 1);
    let again = memory
        .search_outcome_for(&query("rollback", SearchMode::Hybrid))
        .await?;
    assert_eq!(again.degraded.as_deref(), Some("http 503"));
    assert_eq!(memory.vector_arm_dropped(), 2);
    Ok(())
}

#[sqlx::test]
async fn hybrid_fuses_the_vector_arm_with_the_text_arms(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let bare = unwired(&pool)
        .search_outcome_for(&query("xqzzk banana", SearchMode::Hybrid))
        .await?;
    assert!(
        bare.hits.is_empty(),
        "without the vector arm the query matches nothing: {:?}",
        contents(&bare.hits)
    );

    let seeder = FakeProvider::fnv_embeddings(WIDTH);
    seed_all_four(&pool, &seeder).await?;
    let (memory, _) = wired(&pool, FakeProvider::fnv_embeddings(WIDTH));
    let fused = memory
        .search_outcome_for(&query("xqzzk banana", SearchMode::Hybrid))
        .await?;
    let ranked: Vec<&str> = fused.hits.iter().map(|hit| hit.content.as_str()).collect();
    assert!(
        !ranked.is_empty(),
        "the vector arm recovers the banana rows"
    );
    assert_eq!(
        ranked.first().copied(),
        Some(BANANA_THRICE),
        "the vector rank drives the fused order: {ranked:?}"
    );
    assert!(ranked.contains(&BANANA_ONCE));
    assert_eq!(fused.degraded, None);
    assert_eq!(memory.vector_arm_dropped(), 0);
    Ok(())
}

#[sqlx::test]
async fn vector_mode_fallbacks_record_every_degraded_reason(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let (memory, _) = wired(
        &pool,
        FakeProvider::new(vec![
            Script::Embed(vec![]),
            Script::Embed(vec![Embedding {
                vector: vec![0.0; WIDTH],
            }]),
            Script::EmbedNetwork,
        ]),
    );
    for reason in ["bad response", "empty vector", "network"] {
        let outcome = memory
            .search_outcome_for(&query("deploy", SearchMode::Vector))
            .await?;
        assert_eq!(outcome.degraded, Some(reason.to_owned()));
        assert!(
            contents(&outcome.hits).contains(&DEPLOY.to_string()),
            "the fts fallback answers for {reason}"
        );
    }
    assert_eq!(memory.vector_arm_dropped(), 3);
    Ok(())
}

#[sqlx::test]
async fn explicit_fts_embeds_for_semantics_and_degrades_quietly(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let seeder = FakeProvider::fnv_embeddings(WIDTH);
    seed_fnv_embeddings(&pool, &seeder, &[DEPLOY, BANANA_ONCE]).await?;

    let (scoring, _) = wired(&pool, FakeProvider::fnv_embeddings(WIDTH));
    let scored = scoring
        .search_outcome_for(&query("deploy", SearchMode::Fts))
        .await?;
    assert_eq!(scored.degraded, None);
    assert_eq!(scoring.vector_arm_dropped(), 0);

    let (failing, _) = wired(&pool, FakeProvider::new(vec![Script::EmbedHttpStatus(500)]));
    let degraded = failing
        .search_outcome_for(&query("deploy", SearchMode::Fts))
        .await?;
    assert_eq!(degraded.degraded.as_deref(), Some("http 500"));
    assert_eq!(failing.vector_arm_dropped(), 0, "explicit fts warns only");

    let with_embed = scored
        .hits
        .iter()
        .find(|hit| hit.content == DEPLOY)
        .ok_or("the deploy hit is missing when the embed works")?;
    let without_embed = degraded
        .hits
        .iter()
        .find(|hit| hit.content == DEPLOY)
        .ok_or("the deploy hit is missing when the embed fails")?;
    assert!(
        with_embed.score > without_embed.score,
        "the embedding lifts the semantic term: {} vs {}",
        with_embed.score,
        without_embed.score
    );
    Ok(())
}

#[sqlx::test]
async fn the_query_input_is_the_instruction_plus_normalized_text(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let (memory, provider) = wired(&pool, FakeProvider::fnv_embeddings(WIDTH));
    memory
        .search_outcome_for(&query("Deploy   the  RELEASE ", SearchMode::Hybrid))
        .await?;
    let want = format!(
        "{}Deploy the RELEASE",
        memory_postgres::embed::DEFAULT_EMBED_QUERY_INSTRUCTION
    );
    assert_eq!(provider.embed_requests().await, vec![vec![want]]);
    Ok(())
}

fn basis(dim: usize, value: f32) -> Vec<f32> {
    (0..WIDTH)
        .map(|index| if index == dim { value } else { 0.0 })
        .collect()
}

#[sqlx::test]
async fn the_chunk_arm_surfaces_and_merges_parents(pool: PgPool) -> TestResult {
    seed_four(&pool).await?;
    let query_vector = basis(0, 1.0);
    let (memory, _) = wired(
        &pool,
        FakeProvider::new(vec![Script::Embed(vec![Embedding {
            vector: query_vector.clone(),
        }])]),
    );

    let weak = basis(0, 0.1);
    embed_row(&pool, "rollback", &weak).await?;
    embed_row(&pool, "deploy", &basis(1, 1.0)).await?;

    for (event, scale) in [("banana-once", 2.0_f32), ("rollback", 1.0)] {
        let id: sqlx::types::Uuid =
            sqlx::query_scalar("SELECT id FROM rh_messages WHERE metadata->>'event_id' = $1")
                .bind(event)
                .fetch_one(&pool)
                .await?;
        let chunk: Vec<f32> = query_vector.iter().map(|value| value * scale).collect();
        sqlx::query(
            "INSERT INTO rh_message_chunks (message_id, idx, char_start, char_end, content, embedding)
             VALUES ($1, 0, 0, 16, 'seed chunk', $2)",
        )
        .bind(id)
        .bind(HalfVector::from_f32_slice(&chunk))
        .execute(&pool)
        .await?;
    }

    let outcome = memory
        .search_outcome_for(&query("banana", SearchMode::Vector))
        .await?;
    let ranked: Vec<&str> = outcome
        .hits
        .iter()
        .map(|hit| hit.content.as_str())
        .collect();
    assert_eq!(
        ranked.iter().take(2).copied().collect::<BTreeSet<&str>>(),
        BTreeSet::from([BANANA_ONCE, ROLLBACK]),
        "the chunk arm surfaces the chunk-only parent and the strong chunk merges over the weak parent: {ranked:?}"
    );
    assert_eq!(
        ranked.last().copied(),
        Some(DEPLOY),
        "the unrelated parent stays last: {ranked:?}"
    );
    let rollback_hit = outcome
        .hits
        .iter()
        .find(|hit| hit.content == ROLLBACK)
        .ok_or("the rollback parent is missing")?;
    assert!(
        rollback_hit.score > 0.5,
        "the merged row carries the chunk score, not the weak parent score: {}",
        rollback_hit.score
    );
    Ok(())
}
