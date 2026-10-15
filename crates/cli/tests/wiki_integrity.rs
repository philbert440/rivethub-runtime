#![cfg(test)]

#[expect(
    dead_code,
    reason = "each wiki test target uses a subset of shared test helpers"
)]
mod wiki_support;

use kernel::wiki::builder::{WikiAction, WikiBuilderStore, WikiStoreError};
use memory_postgres::PostgresMemory;
use sqlx::PgPool;
use testkit::fakes::{FakeJobQueue, FakeProvider};
use tokio_util::sync::CancellationToken;
use wiki_support::{TestResult, now, patch, provider, raw_provider, seed, slug};
use wire::stream::openai::FinishReason;
use workers::WorkerError;
use workers::wiki_builder::{WikiBuilderEnv, extract_wiki_job, handle_extract_wiki};

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_alias_updates_and_unchanged_patches_keep_provenance_without_reembedding(
    pool: PgPool,
) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let first = seed(&pool).await?;
    assert_eq!(
        store.apply_wiki(&first, vec![patch()], now()).await?,
        vec!["acmekit-deploys"]
    );
    let second = seed(&pool).await?;
    assert_eq!(
        store.apply_wiki(&second, vec![patch()], now()).await?,
        Vec::<String>::new()
    );
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM rh_wiki_provenance")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count.0, 2);
    let third = seed(&pool).await?;
    let mut update = patch();
    update.action = WikiAction::Update;
    update.slug = slug("acmekit-rollout");
    update.summary = "Rollback switches traffic to the previous colour. 🦀".to_owned();
    update.article = None;
    assert_eq!(
        store
            .apply_wiki(&third, vec![update.clone()], now())
            .await?,
        vec!["acmekit-deploys"]
    );
    let row: (String, String, i32, Option<String>) = sqlx::query_as(
        "SELECT current_state, article, history_count, embed_status FROM rh_wiki_topics",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(row.0, update.summary);
    assert_eq!(row.1, patch().article.expect("article"));
    assert_eq!(row.2, 2);
    assert_eq!(row.3, None);
    let aliases: (Vec<String>,) = sqlx::query_as("SELECT aliases FROM rh_wiki_topics")
        .fetch_one(&pool)
        .await?;
    assert_eq!(aliases.0, vec!["acmekit-rollout"]);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_conflicting_alias_rolls_back_every_patch(pool: PgPool) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let first = seed(&pool).await?;
    store.apply_wiki(&first, vec![patch()], now()).await?;
    let second = seed(&pool).await?;
    let mut other = patch();
    other.slug = slug("other");
    other.aliases = vec![slug("other-alias")];
    store.apply_wiki(&second, vec![other], now()).await?;
    let third = seed(&pool).await?;
    let mut conflict = patch();
    conflict.aliases.push(slug("other-alias"));
    let mut fresh = patch();
    fresh.slug = slug("a-fresh-topic");
    fresh.aliases.clear();
    assert!(matches!(
        store.apply_wiki(&third, vec![fresh, conflict], now()).await,
        Err(WikiStoreError::SlugConflict(_))
    ));
    let topics: Vec<(String,)> = sqlx::query_as("SELECT slug FROM rh_wiki_topics ORDER BY slug")
        .fetch_all(&pool)
        .await?;
    assert_eq!(
        topics,
        vec![("acmekit-deploys".to_owned(),), ("other".to_owned(),)]
    );
    let extraction: (i64,) =
        sqlx::query_as("SELECT count(*) FROM rh_wiki_extractions WHERE summary_id = $1")
            .bind(third.summary_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(extraction.0, 0);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_rejects_wrong_conversation_before_model_or_writes(pool: PgPool) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let mut job = seed(&pool).await?;
    job.conversation_id = Some(uuid::Uuid::new_v4());
    let model = provider(&[patch()]);
    let queue = FakeJobQueue::default();
    let cancel = CancellationToken::new();
    let env = WikiBuilderEnv {
        store: &store,
        provider: &model,
        queue: &queue,
        model: "fake-chat",
        now: now(),
        cancel: &cancel,
    };
    assert!(matches!(
        handle_extract_wiki(&env, &extract_wiki_job(&job)?.payload).await,
        Err(WorkerError::WikiStore(
            WikiStoreError::ConversationMismatch(_)
        ))
    ));
    assert_eq!(model.chat_requests().await.len(), 0);
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM rh_wiki_extractions")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count.0, 0);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_bad_model_output_is_failed_and_retryable(pool: PgPool) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let queue = FakeJobQueue::default();
    let cancel = CancellationToken::new();
    for (text, finish) in [
        ("not JSON", FinishReason::Stop),
        ("[{}]", FinishReason::Stop),
        ("[]", FinishReason::Length),
    ] {
        let job = seed(&pool).await?;
        let model = raw_provider(text, finish);
        let env = WikiBuilderEnv {
            store: &store,
            provider: &model,
            queue: &queue,
            model: "fake-chat",
            now: now(),
            cancel: &cancel,
        };
        assert!(
            handle_extract_wiki(&env, &extract_wiki_job(&job)?.payload)
                .await
                .is_err()
        );
        let row: (String, Option<String>) =
            sqlx::query_as("SELECT status, error FROM rh_wiki_extractions WHERE summary_id = $1")
                .bind(job.summary_id)
                .fetch_one(&pool)
                .await?;
        assert_eq!(row.0, "failed");
        assert!(row.1.is_some());
        assert!(store.wiki_backfill(100).await?.contains(&job));
        let model = provider(&[]);
        let env = WikiBuilderEnv {
            provider: &model,
            ..env
        };
        assert_eq!(
            handle_extract_wiki(&env, &extract_wiki_job(&job)?.payload).await?,
            Vec::<String>::new()
        );
        assert!(!store.wiki_backfill(100).await?.contains(&job));
    }
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_duplicate_concurrent_commits_do_not_duplicate_history(pool: PgPool) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let job = seed(&pool).await?;
    let (left, right) = tokio::try_join!(
        store.apply_wiki(&job, vec![patch()], now()),
        store.apply_wiki(&job, vec![patch()], now())
    )?;
    assert_eq!(left, right);
    let row: (i32,) = sqlx::query_as("SELECT history_count FROM rh_wiki_topics")
        .fetch_one(&pool)
        .await?;
    assert_eq!(row.0, 1);
    store
        .fail_wiki(job.summary_id, "late model failure", now())
        .await?;
    let row: (String,) = sqlx::query_as("SELECT status FROM rh_wiki_extractions")
        .fetch_one(&pool)
        .await?;
    assert_eq!(row.0, "done");
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_cancellation_leaves_no_extraction(pool: PgPool) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let job = seed(&pool).await?;
    let model = FakeProvider::default();
    let queue = FakeJobQueue::default();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let env = WikiBuilderEnv {
        store: &store,
        provider: &model,
        queue: &queue,
        model: "fake-chat",
        now: now(),
        cancel: &cancel,
    };
    assert!(matches!(
        handle_extract_wiki(&env, &extract_wiki_job(&job)?.payload).await,
        Err(WorkerError::Cancelled)
    ));
    let row: (i64,) = sqlx::query_as("SELECT count(*) FROM rh_wiki_extractions")
        .fetch_one(&pool)
        .await?;
    assert_eq!(row.0, 0);
    assert_eq!(model.chat_requests().await.len(), 0);
    Ok(())
}

#[test]
fn wiki_model_types_reject_bad_slugs_and_wrong_types() -> TestResult {
    for slug in ["../escape", "/absolute", "a/b", "a\\b", ""] {
        assert!(wire::gateway::wiki::WikiSlug::try_from(slug.to_owned()).is_err());
    }
    for slug in ["acmekit-rollout", "developer's-notes", "release-v2"] {
        assert!(wire::gateway::wiki::WikiSlug::try_from(slug.to_owned()).is_ok());
    }
    assert!(
        serde_json::from_str::<kernel::wiki::builder::ExtractWiki>(r#"{"summaryId":3}"#).is_err()
    );
    let encoded = serde_json::to_string(&vec![patch()])?;
    assert_eq!(
        serde_json::from_str::<Vec<kernel::wiki::builder::WikiPatch>>(&encoded)?,
        vec![patch()]
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn wiki_summary_without_conversation_preserves_absence(pool: PgPool) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let mut job = seed(&pool).await?;
    sqlx::query("UPDATE rh_summaries SET conversation_id = NULL WHERE id = $1")
        .bind(job.summary_id)
        .execute(&pool)
        .await?;
    job.conversation_id = None;
    let kernel::wiki::builder::WikiWork::Pending(summary) = store.wiki_work(&job).await? else {
        panic!("new work");
    };
    assert_eq!(summary.conversation_id, None);
    store.apply_wiki(&job, vec![patch()], now()).await?;
    let row: (Option<uuid::Uuid>,) =
        sqlx::query_as("SELECT conversation_id FROM rh_wiki_provenance")
            .fetch_one(&pool)
            .await?;
    assert_eq!(row.0, None);
    Ok(())
}
