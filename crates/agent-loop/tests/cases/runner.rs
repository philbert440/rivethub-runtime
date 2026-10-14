use std::sync::Arc;
use std::time::Duration;

use kernel::agent::{StopReason, TurnError, TurnRunner};
use kernel::model::ModelChunk;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use wire::hooks::HookContextKind;

use crate::support::provider::{Plan, Provider, chunks};
use crate::support::{Harness, agent, text, turn};

#[tokio::test]
async fn kernel_runner_streams_one_audited_headless_turn() {
    let mut harness = Harness::new().await;
    let expected = vec![text("hello 🦀"), ModelChunk::Done];
    let provider = Provider::new(vec![chunks(expected.clone())]);
    let runner: Arc<dyn TurnRunner> = Arc::new(agent(provider.clone(), Vec::new(), &harness));
    let mut input = turn();
    input.limits.steps = 1;
    let (sender, mut events) = mpsc::channel(4);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        runner.run(input, sender, CancellationToken::new()),
    )
    .await
    .expect("bounded turn")
    .expect("turn completes");
    assert_eq!(result.response, "hello 🦀");
    assert_eq!(result.stop, StopReason::Completed);
    assert_eq!(result.iterations, 1);
    assert_eq!(provider.requests.lock().len(), 1);
    for chunk in expected {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("event"),
            Some(chunk)
        );
    }
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("events close"),
        None
    );
    harness.close().await;
    let records = harness.records().await;
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record.context.kind, HookContextKind::ProviderBefore { .. }))
            .count(),
        1
    );
    assert!(records.iter().any(|record| matches!(
        record.context.kind,
        HookContextKind::TurnAfter {
            iterations: 1,
            aborted: false,
            ..
        }
    )));
}

#[tokio::test]
async fn kernel_runner_preserves_the_failure_source() {
    let mut harness = Harness::new().await;
    let runner: Arc<dyn TurnRunner> =
        Arc::new(agent(Provider::new(vec![Plan::Fail]), Vec::new(), &harness));
    let (sender, _events) = mpsc::channel(4);
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        runner.run(turn(), sender, CancellationToken::new()),
    )
    .await
    .expect("bounded turn")
    .expect_err("provider fails");
    let TurnError::Failed(source) = error else {
        panic!("typed failure source")
    };
    assert!(matches!(
        source.downcast_ref::<agent_loop::Error>(),
        Some(agent_loop::Error::Provider(_))
    ));
    harness.close().await;
}

#[tokio::test]
async fn kernel_runner_propagates_cancellation_and_timeout() {
    let mut harness = Harness::new().await;
    let entered = Arc::new(Notify::new());
    let dropped = CancellationToken::new();
    let provider = Provider::new(vec![Plan::Wait {
        entered: Arc::clone(&entered),
        dropped: dropped.clone(),
    }]);
    let runner: Arc<dyn TurnRunner> = Arc::new(agent(provider, Vec::new(), &harness));
    let cancel = CancellationToken::new();
    let (sender, _events) = mpsc::channel(4);
    let cancellation = async {
        entered.notified().await;
        cancel.cancel();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(runner.run(turn(), sender, cancel.clone()), cancellation)
    })
    .await
    .expect("cancellation bounded");
    assert!(matches!(result, Err(TurnError::Cancelled)));
    assert!(dropped.is_cancelled());
    let provider = Provider::new(vec![Plan::Open(Vec::new())]);
    let runner: Arc<dyn TurnRunner> = Arc::new(agent(provider, Vec::new(), &harness));
    let mut input = turn();
    input.limits.timeout = Duration::ZERO;
    let (sender, _events) = mpsc::channel(4);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        runner.run(input, sender, CancellationToken::new()),
    )
    .await
    .expect("timeout bounded");
    assert!(matches!(result, Err(TurnError::Timeout)));
    harness.close().await;
}
