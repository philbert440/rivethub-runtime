#![cfg(test)]

#[expect(
    unreachable_pub,
    reason = "the harness is a private module of each test binary"
)]
#[expect(
    dead_code,
    reason = "each test binary uses a different subset of the harness"
)]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::Phase;
use common::Queue;
use common::Store;
use common::settings;
use common::target_payload;
use common::wait_at_least;
use common::wait_bool;
use common::width;
use kernel::model::ModelProvider;
use kernel::queue::FixedJob;
use kernel::queue::JobName;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use testkit::FakeProvider;
use testkit::Script;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use wire::JsonObject;
use workers::EMBED_CONCURRENCY;
use workers::EmbedTable;
use workers::POLL_INTERVAL;
use workers::ProcessRole;
use workers::Runner;
use workers::RunnerConfig;
use workers::WorkerError;
use workers::jobs_for;

#[test]
fn roles_match_the_kernel_task_lists() {
    assert_eq!(names(WorkerRole::Embedder), vec!["embed-target".to_owned()]);
    assert_eq!(
        names(WorkerRole::Summarizer),
        vec![
            "compact-conversation".to_owned(),
            "synthesize-tool-call".to_owned(),
            "enqueue-idle".to_owned(),
            "enqueue-stale-compaction".to_owned(),
            "reap-dead-jobs".to_owned(),
        ]
    );
    assert_eq!(
        names(WorkerRole::Categorizer),
        vec!["suggest-tags".to_owned()]
    );
    assert_eq!(
        names(WorkerRole::WikiBuilder),
        vec![
            "extract-wiki".to_owned(),
            "enqueue-wiki-backfill".to_owned(),
            "consolidate-wiki".to_owned(),
            "recompile-wiki".to_owned(),
            "enqueue-stale-wiki".to_owned(),
        ]
    );
    assert!(accepts(&fixed(FixedJob::EmbedTarget), WorkerRole::Embedder));
    assert!(!accepts(
        &fixed(FixedJob::CompactConversation),
        WorkerRole::Embedder
    ));
    assert!(accepts(
        &fixed(FixedJob::ReapDeadJobs),
        WorkerRole::Summarizer
    ));
    assert!(!accepts(
        &fixed(FixedJob::ExtractWiki),
        WorkerRole::Categorizer
    ));
}

#[test]
fn unknown_roles_list_the_enum_and_zero_concurrency_is_rejected() {
    assert_eq!(
        ProcessRole::parse("embedder").expect("role"),
        ProcessRole::Embedder
    );
    assert_eq!(
        ProcessRole::parse("wiki-builder").expect("role"),
        ProcessRole::WikiBuilder
    );
    assert_eq!(ProcessRole::parse("all").expect("role"), ProcessRole::All);
    let error = ProcessRole::parse("wiki_builder").expect_err("unknown");
    assert_eq!(
        error.to_string(),
        "unknown worker role: wiki_builder; expected embedder, summarizer, categorizer, wiki-builder, all"
    );
    assert!(matches!(
        RunnerConfig::new(ProcessRole::Embedder, 0),
        Err(WorkerError::Concurrency)
    ));
    assert_eq!(POLL_INTERVAL, Duration::from_secs(60));
    assert_eq!(EMBED_CONCURRENCY, 4);
    assert_eq!(RunnerConfig::embedder().concurrency, EMBED_CONCURRENCY);
    assert_eq!(RunnerConfig::embedder().poll_interval, POLL_INTERVAL);
}

#[tokio::test(flavor = "current_thread")]
async fn roles_without_a_handler_refuse_to_start() {
    for role in [
        ProcessRole::Summarizer,
        ProcessRole::Categorizer,
        ProcessRole::WikiBuilder,
    ] {
        let rig = rig(role, 1, fnv_provider());
        let error = rig
            .runner
            .run(CancellationToken::new())
            .await
            .expect_err("unavailable");
        assert!(matches!(error, WorkerError::RoleUnavailable { role: got } if got == role));
        assert_eq!(rig.queue.claims(), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn all_claims_only_the_embedder() {
    let rig = rig(ProcessRole::All, 1, fnv_provider());
    rig.store
        .insert(EmbedTable::Messages, "m", "hello", None)
        .await
        .expect("seed");
    let embed = rig
        .queue
        .push(
            fixed(FixedJob::EmbedTarget),
            target_payload("ros_messages", "m"),
            5,
        )
        .await;
    let other = rig
        .queue
        .push(
            fixed(FixedJob::CompactConversation),
            JsonObject::default(),
            5,
        )
        .await;
    let (cancel, run) = spawn(&rig);
    rig.queue.wait_phase(embed, Phase::Done).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(rig.queue.claims(), 1);
    assert_eq!(rig.queue.view(other).await.phase, Phase::Ready);
    finish(cancel, run).await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_unhandled_claim_is_failed() {
    let rig = rig(ProcessRole::Embedder, 1, fnv_provider());
    let id = rig
        .queue
        .push_any(
            fixed(FixedJob::CompactConversation),
            JsonObject::default(),
            1,
        )
        .await;
    let (cancel, run) = spawn(&rig);
    rig.queue.wait_phase(id, Phase::Dead).await;
    let job = rig.queue.view(id).await;
    assert_eq!(
        job.error.as_deref(),
        Some("no handler for job compact-conversation")
    );
    assert_ne!(job.phase, Phase::Done);
    finish(cancel, run).await;
}

#[tokio::test(flavor = "current_thread")]
async fn concurrency_stays_at_the_configured_limit() {
    let queue = Arc::new(Queue::new());
    let store = Arc::new(Store::holding());
    let runner = Arc::new(Runner::new(
        Arc::clone(&queue),
        Arc::clone(&store),
        fnv_provider(),
        RunnerConfig::new(ProcessRole::Embedder, 2).expect("concurrency"),
        settings(),
    ));
    let mut ids = Vec::new();
    for name in ["a", "b", "c"] {
        store
            .insert(EmbedTable::Messages, name, "hello", None)
            .await
            .expect("seed");
        ids.push(
            queue
                .push(
                    fixed(FixedJob::EmbedTarget),
                    target_payload("ros_messages", name),
                    5,
                )
                .await,
        );
    }
    let mut loads = store.loads();
    let cancel = CancellationToken::new();
    let worker = Arc::clone(&runner);
    let token = cancel.clone();
    let run = tokio::spawn(async move { worker.run(token).await });
    wait_at_least(&mut loads, 2).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(store.load_count(), 2);
    store.release();
    for id in ids {
        queue.wait_phase(id, Phase::Done).await;
    }
    assert_eq!(store.load_count(), 3);
    assert_eq!(store.writes().await, 3);
    finish(cancel, run).await;
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_fails_every_in_flight_embed() {
    let queue = Arc::new(Queue::new());
    let store = Arc::new(Store::new());
    let provider = Arc::new(common::HangEmbed::new());
    for name in ["m", "n"] {
        store
            .insert(EmbedTable::Messages, name, "hello", None)
            .await
            .expect("seed");
    }
    let runner = Arc::new(Runner::new(
        Arc::clone(&queue),
        Arc::clone(&store),
        Arc::clone(&provider) as Arc<dyn ModelProvider>,
        RunnerConfig::new(ProcessRole::Embedder, 2).expect("concurrency"),
        settings(),
    ));
    let mut ids = Vec::new();
    for name in ["m", "n"] {
        ids.push(
            queue
                .push(
                    fixed(FixedJob::EmbedTarget),
                    target_payload("ros_messages", name),
                    5,
                )
                .await,
        );
    }
    let cancel = CancellationToken::new();
    let worker = Arc::clone(&runner);
    let token = cancel.clone();
    let run = tokio::spawn(async move { worker.run(token).await });
    queue.wait_claims(2).await;
    cancel.cancel();
    run.await.expect("join").expect("runner");
    for id in ids {
        let job = queue.view(id).await;
        assert_eq!(job.phase, Phase::Backoff);
        assert_eq!(job.attempt, 1);
        assert_eq!(job.error.as_deref(), Some("the worker was cancelled"));
    }
    assert_eq!(store.writes().await, 0);
    assert_eq!(store.failures(EmbedTable::Messages, "m").await, 0);
    assert_eq!(store.failures(EmbedTable::Messages, "n").await, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_claim_error_stops_the_runner() {
    let rig = rig(ProcessRole::Embedder, 1, fnv_provider());
    rig.queue.fail_next_claim().await;
    let error = rig
        .runner
        .run(CancellationToken::new())
        .await
        .expect_err("claim");
    assert!(matches!(
        error,
        WorkerError::Queue(JobQueueError::Unavailable(_))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn a_failure_backs_off_without_another_claim() {
    let provider =
        Arc::new(FakeProvider::new(vec![Script::EmbedHttpStatus(500)])) as Arc<dyn ModelProvider>;
    let rig = rig(ProcessRole::Embedder, 1, provider);
    rig.store
        .insert(EmbedTable::Messages, "m", "hello", None)
        .await
        .expect("seed");
    let backed = rig
        .queue
        .push(
            fixed(FixedJob::EmbedTarget),
            target_payload("ros_messages", "m"),
            5,
        )
        .await;
    let (cancel, run) = spawn(&rig);
    rig.queue.wait_phase(backed, Phase::Backoff).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    let job = rig.queue.view(backed).await;
    assert_eq!(job.attempt, 1);
    assert_eq!(rig.queue.claims(), 1);
    assert!(job.error.as_deref().expect("error").contains("500"));
    finish(cancel, run).await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_last_attempt_is_dead_and_not_completed() {
    let provider =
        Arc::new(FakeProvider::new(vec![Script::EmbedHttpStatus(500)])) as Arc<dyn ModelProvider>;
    let rig = rig(ProcessRole::Embedder, 1, provider);
    rig.store
        .insert(EmbedTable::Messages, "m", "hello", None)
        .await
        .expect("seed");
    let dead = rig
        .queue
        .push(
            fixed(FixedJob::EmbedTarget),
            target_payload("ros_messages", "m"),
            1,
        )
        .await;
    let (cancel, run) = spawn(&rig);
    rig.queue.wait_phase(dead, Phase::Dead).await;
    let job = rig.queue.view(dead).await;
    assert_eq!(job.attempt, 1);
    assert_ne!(job.phase, Phase::Done);
    finish(cancel, run).await;
}

#[tokio::test(flavor = "current_thread")]
async fn wake_claims_without_waiting_out_the_poll() {
    let rig = rig(ProcessRole::Embedder, 1, fnv_provider());
    let mut idle = rig.runner.idle();
    let (cancel, run) = spawn(&rig);
    wait_bool(&mut idle, true).await;
    let id = rig
        .queue
        .push(
            fixed(FixedJob::EmbedTarget),
            target_payload("ros_messages", "m"),
            5,
        )
        .await;
    rig.runner.wake();
    rig.queue.wait_phase(id, Phase::Done).await;
    assert_eq!(rig.queue.claims(), 1);
    finish(cancel, run).await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_parked_runner_waits_out_the_poll_interval() {
    let rig = rig(ProcessRole::Embedder, 1, fnv_provider());
    let mut idle = rig.runner.idle();
    let (cancel, run) = spawn(&rig);
    wait_bool(&mut idle, true).await;
    let id = rig
        .queue
        .push(
            fixed(FixedJob::EmbedTarget),
            target_payload("ros_messages", "m"),
            5,
        )
        .await;
    tokio::time::advance(Duration::from_secs(59)).await;
    tokio::task::yield_now().await;
    assert_eq!(rig.queue.claims(), 0);
    assert_eq!(rig.queue.view(id).await.phase, Phase::Ready);
    tokio::time::advance(Duration::from_secs(1)).await;
    rig.queue.wait_phase(id, Phase::Done).await;
    finish(cancel, run).await;
}

struct Rig {
    queue: Arc<Queue>,
    store: Arc<Store>,
    runner: Arc<Runner<Queue, Store>>,
}

fn rig(role: ProcessRole, concurrency: u32, provider: Arc<dyn ModelProvider>) -> Rig {
    let queue = Arc::new(Queue::new());
    let store = Arc::new(Store::new());
    let runner = Arc::new(Runner::new(
        Arc::clone(&queue),
        Arc::clone(&store),
        provider,
        RunnerConfig::new(role, concurrency).expect("concurrency"),
        settings(),
    ));
    Rig {
        queue,
        store,
        runner,
    }
}

fn fnv_provider() -> Arc<dyn ModelProvider> {
    Arc::new(FakeProvider::fnv_embeddings(width()))
}

fn fixed(job: FixedJob) -> JobName {
    JobName::Fixed(job)
}

fn names(role: WorkerRole) -> Vec<String> {
    jobs_for(role).iter().map(ToString::to_string).collect()
}

fn accepts(name: &JobName, role: WorkerRole) -> bool {
    workers::accepts(role, name)
}

fn spawn(rig: &Rig) -> (CancellationToken, JoinHandle<Result<(), WorkerError>>) {
    let cancel = CancellationToken::new();
    let worker = Arc::clone(&rig.runner);
    let token = cancel.clone();
    let run = tokio::spawn(async move { worker.run(token).await });
    (cancel, run)
}

async fn finish(cancel: CancellationToken, run: JoinHandle<Result<(), WorkerError>>) {
    cancel.cancel();
    run.await.expect("join").expect("runner");
}
