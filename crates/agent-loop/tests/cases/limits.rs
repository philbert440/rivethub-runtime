use std::sync::Arc;
use std::time::Duration;

use agent_loop::{AgentLoop, Error};
use kernel::model::ModelChunk;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use wire::hooks::HookContextKind;
use wire::stream::LlmUsage;

use crate::support::provider::{Plan, Provider, chunks};
use crate::support::{Harness, agent, call, text, turn};

#[tokio::test]
async fn response_budget_and_disconnected_consumer_fail_the_turn() {
    for disconnected in [false, true] {
        let mut h = Harness::new().await;
        let provider = Provider::new(vec![chunks(vec![text("🦀"), ModelChunk::Done])]);
        let runner = agent(provider, Vec::new(), &h);
        let mut input = turn();
        input.limits.response_bytes = if disconnected { 1024 } else { 3 };
        let (sender, receiver) = mpsc::channel(8);
        if disconnected {
            drop(receiver);
        }
        let error = runner
            .run(input, sender, CancellationToken::new())
            .await
            .expect_err("fails");
        if disconnected {
            assert!(matches!(error, Error::Disconnected));
        } else {
            assert!(matches!(error, Error::Stream(_)));
        }
        h.close().await;
    }
}

#[tokio::test]
async fn invalid_turn_and_precancelled_turn_make_no_provider_requests() {
    for cancelled in [false, true] {
        let mut h = Harness::new().await;
        let provider = Provider::new(Vec::new());
        let runner = agent(provider.clone(), Vec::new(), &h);
        let mut input = turn();
        let token = CancellationToken::new();
        if cancelled {
            token.cancel();
        } else {
            input.request.messages.clear();
        }
        let (sender, _receiver) = mpsc::channel(8);
        let error = runner.run(input, sender, token).await.expect_err("fails");
        if cancelled {
            assert!(matches!(error, Error::Cancelled));
        } else {
            assert!(matches!(error, Error::MissingUserMessage));
        }
        assert!(provider.requests.lock().is_empty());
        h.close().await;
    }
}

#[tokio::test]
async fn usage_overflow_is_an_error_and_duplicate_catalog_names_are_refused() {
    let mut h = Harness::new().await;
    let usage = ModelChunk::Usage {
        usage: LlmUsage {
            prompt_tokens: u64::MAX,
            completion_tokens: 1,
            reasoning_tokens: None,
            cached_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        },
    };
    let first = [
        call(0, "unknown", "{}"),
        vec![usage.clone(), ModelChunk::Done],
    ]
    .concat();
    let provider = Provider::new(vec![chunks(first), chunks(vec![usage, ModelChunk::Done])]);
    let runner = agent(provider.clone(), Vec::new(), &h);
    let (sender, _receiver) = mpsc::channel(16);
    assert!(matches!(
        runner.run(turn(), sender, CancellationToken::new()).await,
        Err(Error::UsageOverflow)
    ));
    assert!(
        AgentLoop::new(
            "test".to_owned(),
            provider,
            vec![
                Arc::new(testkit::FakeTool::new("same").expect("echo tool schema")),
                Arc::new(testkit::FakeTool::new("same").expect("echo tool schema"))
            ],
            Arc::clone(&h.hooks)
        )
        .is_err()
    );
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waiting_turn_does_not_block_an_independent_turn() {
    let mut h = Harness::new().await;
    let entered = Arc::new(Notify::new());
    let dropped = CancellationToken::new();
    let provider = Provider::new(vec![
        Plan::Wait {
            entered: Arc::clone(&entered),
            dropped: dropped.clone(),
        },
        chunks(vec![text("independent"), ModelChunk::Done]),
    ]);
    let runner = Arc::new(agent(provider, Vec::new(), &h));
    let (sender, _receiver) = mpsc::channel(8);
    let token = CancellationToken::new();
    let first = tokio::spawn({
        let runner = Arc::clone(&runner);
        let token = token.clone();
        async move { runner.run(turn(), sender, token).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("first turn entered");
    let (sender, _receiver) = mpsc::channel(8);
    let mut second = turn();
    second.context.session_id = Some("second-session".to_owned());
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        runner.run(second, sender, CancellationToken::new()),
    )
    .await
    .expect("second turn proceeds")
    .expect("second turn completes");
    assert_eq!(result.response, "independent");
    token.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .expect("first cancels")
            .expect("joins"),
        Err(Error::Cancelled)
    ));
    assert!(dropped.is_cancelled());
    h.close().await;
    assert_eq!(
        h.records()
            .await
            .iter()
            .filter(|record| matches!(record.context.kind, HookContextKind::TurnAfter { .. }))
            .count(),
        2
    );
}
