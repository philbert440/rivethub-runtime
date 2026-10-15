#![cfg(test)]

mod wiki_support;

use kernel::queue::{FixedJob, JobName, JobQueue, WorkerRole};
use kernel::wiki::builder::{ExtractWiki, WIKI_PIPELINE_VERSION, WikiBuilderStore, WikiWork};
use memory_postgres::{PostgresMemory, PostgresQueue};
use sqlx::PgPool;
use testkit::fakes::FakeJobQueue;
use tokio_util::sync::CancellationToken;
use wiki_support::{TestResult, fixture, now, patch, provider, seed, seed_recording};
use wire::message::MessageContent;
use workers::wiki_builder::{
    WikiBuilderEnv, extract_wiki_job, handle_extract_wiki, handle_wiki_backfill,
};

const REPLAYED: &[&str] = &[
    "leaf once a full window exists",
    "leaves → branches → root with small batches",
    "truncated answer shrinks the leaf batch",
    "permanent failure → dead → rests an hour → revived",
    "below the floor until idle/stale",
];
const SKIPPED: &[(&str, &str)] = &[(
    "failing LLM fails the job; heartbeat never summarized",
    "the summarizer produces no summary in this scenario, so the wiki builder has no input",
)];

#[test]
fn wiki_summary_replay_accounts_for_every_scenario() {
    let fixture = fixture();
    assert_eq!(
        fixture.constants.wiki_pipeline_version,
        WIKI_PIPELINE_VERSION
    );
    for scenario in &fixture.scenarios {
        let replayed = REPLAYED
            .iter()
            .filter(|name| **name == scenario.name)
            .count();
        let skipped = SKIPPED
            .iter()
            .filter(|(name, reason)| *name == scenario.name && !reason.is_empty())
            .count();
        assert_eq!(replayed + skipped, 1, "{}", scenario.name);
    }
    assert_eq!(fixture.scenarios.len(), REPLAYED.len() + SKIPPED.len());
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_replays_every_recorded_summary_shape(pool: PgPool) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let cancel = CancellationToken::new();
    for scenario in fixture().scenarios {
        if SKIPPED.iter().any(|(name, _)| *name == scenario.name) {
            continue;
        }
        for row in scenario.tables.summaries.expect("replayed summaries") {
            seed_recording(&pool, &row).await?;
            let model = provider(&[patch()]);
            let queue = FakeJobQueue::default();
            let env = WikiBuilderEnv {
                store: &store,
                provider: &model,
                queue: &queue,
                model: "fake-chat",
                now: row.created_at,
                cancel: &cancel,
            };
            let job = ExtractWiki {
                summary_id: row.id,
                conversation_id: Some(row.conversation_id),
            };
            let work = store.wiki_work(&job).await?;
            let WikiWork::Pending(summary) = work else {
                panic!("fresh extraction");
            };
            assert_eq!(summary.id, row.id);
            assert_eq!(summary.conversation_id, Some(row.conversation_id));
            assert_eq!(summary.content, row.content);
            let topics = handle_extract_wiki(&env, &extract_wiki_job(&job)?.payload).await?;
            assert_eq!(topics, vec!["acmekit-deploys"]);
            let calls = model.chat_requests().await;
            assert_eq!(calls.len(), 1);
            let MessageContent::Text(user) = &calls
                .first()
                .expect("request")
                .messages
                .get(1)
                .expect("user")
                .content
            else {
                panic!("text");
            };
            assert_eq!(
                serde_json::from_str::<wire::JsonObject>(user)?,
                wire::JsonObject::from_serialize(&summary)?,
            );
            let state: (String, i32, Vec<String>) = sqlx::query_as("SELECT status, pipeline_version, topics_touched FROM rh_wiki_extractions WHERE summary_id = $1")
                .bind(row.id).fetch_one(&pool).await?;
            assert_eq!(state, ("done".to_owned(), WIKI_PIPELINE_VERSION, topics));
            sqlx::query("DELETE FROM rh_wiki_topics")
                .execute(&pool)
                .await?;
        }
        sqlx::query("DELETE FROM rh_summaries")
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM rh_conversations")
            .execute(&pool)
            .await?;
    }
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_claims_jobs_persists_sources_and_retries_embedding_delivery(
    pool: PgPool,
) -> TestResult {
    testkit::graphile_schema::install_graphile_worker(&pool).await?;
    let queue = PostgresQueue::connect(pool.clone()).await?;
    let store = PostgresMemory::new(pool.clone());
    let model = provider(&[patch()]);
    let cancel = CancellationToken::new();
    let job = seed(&pool).await?;
    let enqueued = extract_wiki_job(&job)?;
    assert_eq!(enqueued.key, Some(format!("wiki-ext-{}", job.summary_id)));
    queue.enqueue(enqueued).await?;
    let claimed = queue
        .claim(&kernel::queue::ClaimSelector::matching(
            queue.task_names(WorkerRole::WikiBuilder),
        ))
        .await?
        .expect("wiki job");
    assert_eq!(claimed.name, JobName::Fixed(FixedJob::ExtractWiki));
    let env = WikiBuilderEnv {
        store: &store,
        provider: &model,
        queue: &queue,
        model: "fake-chat",
        now: now(),
        cancel: &cancel,
    };
    let topics = handle_extract_wiki(&env, &claimed.payload).await?;
    assert_eq!(topics, vec!["acmekit-deploys"]);
    assert_eq!(handle_extract_wiki(&env, &claimed.payload).await?, topics);
    assert_eq!(model.chat_requests().await.len(), 1);
    queue.complete(claimed.id).await?;
    let embed = queue
        .claim(&kernel::queue::ClaimSelector::matching(
            queue.task_names(WorkerRole::Embedder),
        ))
        .await?
        .expect("embedding");
    assert_eq!(embed.name, JobName::Fixed(FixedJob::EmbedTarget));
    assert_eq!(
        workers::read_target(&embed.payload)?,
        workers::ParsedTarget::Ready(workers::EmbedTarget {
            table: workers::EmbedTable::WikiTopics,
            id: "acmekit-deploys".to_owned()
        })
    );
    assert!(
        queue
            .claim(&kernel::queue::ClaimSelector::matching(
                queue.task_names(WorkerRole::Embedder)
            ))
            .await?
            .is_none()
    );
    let source: (uuid::Uuid, uuid::Uuid) =
        sqlx::query_as("SELECT source_id, conversation_id FROM rh_wiki_provenance")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        source,
        (job.summary_id, job.conversation_id.expect("conversation"))
    );
    let citation: (uuid::Uuid, Option<String>) =
        sqlx::query_as("SELECT summary_id, note FROM rh_wiki_citations")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        citation,
        (
            job.summary_id,
            Some("Decided to deploy blue-green.".to_owned())
        )
    );
    let redirect: (String, String) =
        sqlx::query_as("SELECT from_slug, to_slug FROM rh_wiki_redirects")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        redirect,
        ("acmekit-rollout".to_owned(), "acmekit-deploys".to_owned())
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_backfill_requeues_old_done_and_failed_but_preserves_current_and_skipped(
    pool: PgPool,
) -> TestResult {
    testkit::graphile_schema::install_graphile_worker(&pool).await?;
    let queue = PostgresQueue::connect(pool.clone()).await?;
    let store = PostgresMemory::new(pool.clone());
    let model = provider(&[]);
    let cancel = CancellationToken::new();
    let mut expected = std::collections::BTreeSet::new();
    for (status, version, eligible) in [
        ("done", 3, true),
        ("done", 4, false),
        ("failed", 4, true),
        ("skipped", 3, false),
        ("done", 5, false),
    ] {
        let job = seed(&pool).await?;
        sqlx::query("INSERT INTO rh_wiki_extractions (summary_id, status, pipeline_version) VALUES ($1,$2,$3)")
            .bind(job.summary_id).bind(status).bind(version).execute(&pool).await?;
        if eligible {
            expected.insert(job.summary_id);
        }
    }
    expected.insert(seed(&pool).await?.summary_id);
    let env = WikiBuilderEnv {
        store: &store,
        provider: &model,
        queue: &queue,
        model: "fake-chat",
        now: now(),
        cancel: &cancel,
    };
    assert_eq!(handle_wiki_backfill(&env).await?, expected.len());
    assert_eq!(handle_wiki_backfill(&env).await?, expected.len());
    let mut actual = std::collections::BTreeSet::new();
    while let Some(job) = queue
        .claim(&kernel::queue::ClaimSelector::matching(
            queue.task_names(WorkerRole::WikiBuilder),
        ))
        .await?
    {
        let body: ExtractWiki = serde_json::from_value(serde_json::to_value(job.payload)?)?;
        assert!(actual.insert(body.summary_id));
        queue.complete(job.id).await?;
    }
    assert_eq!(actual, expected);
    assert_eq!(model.chat_requests().await.len(), 0);
    let crons = PostgresQueue::crontab(WorkerRole::WikiBuilder);
    assert!(crons.iter().any(
        |cron| cron.task == JobName::Fixed(FixedJob::EnqueueWikiBackfill)
            && cron.schedule == "*/10 * * * *"
    ));
    Ok(())
}
