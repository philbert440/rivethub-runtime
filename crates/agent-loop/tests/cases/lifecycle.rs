use std::sync::Arc;
use std::time::Duration;

use agent_loop::Error;
use async_trait::async_trait;
use kernel::ToolContext;
use kernel::model::ModelChunk;
use kernel::tool::{Tool, ToolError};
use tokio::sync::{Barrier, Notify, mpsc};
use tokio_util::sync::CancellationToken;
use wire::hooks::HookContextKind;

use crate::support::provider::{Plan, Provider, chunks};
use crate::support::{Harness, agent, call, names, text, turn};

#[tokio::test]
async fn cancellation_drops_provider_start_and_emits_aborted_turn() {
    let mut h = Harness::new().await;
    let entered = Arc::new(Notify::new());
    let dropped = CancellationToken::new();
    let provider = Provider::new(vec![Plan::Wait {
        entered: Arc::clone(&entered),
        dropped: dropped.clone(),
    }]);
    let agent = agent(provider, Vec::new(), &h);
    let cancel = CancellationToken::new();
    let (sender, _receiver) = mpsc::channel(8);
    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { agent.run(turn(), sender, cancel).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("provider entered");
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("turn ends")
            .expect("joins"),
        Err(Error::Cancelled)
    ));
    assert!(dropped.is_cancelled());
    h.close().await;
    assert!(matches!(
        h.records()
            .await
            .last()
            .expect("terminal event")
            .context
            .kind,
        HookContextKind::TurnAfter { aborted: true, .. }
    ));
}

#[tokio::test]
async fn turn_timeout_closes_an_open_stream() {
    let mut h = Harness::new().await;
    let provider = Provider::new(vec![Plan::Open(vec![Ok(text("partial"))])]);
    let agent = agent(provider.clone(), Vec::new(), &h);
    let (sender, mut receiver) = mpsc::channel(8);
    let task =
        tokio::spawn(async move { agent.run(turn(), sender, CancellationToken::new()).await });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("first delta"),
        Some(text("partial"))
    );
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(21)).await;
    tokio::time::resume();
    assert!(matches!(task.await.expect("joins"), Err(Error::Timeout)));
    assert!(provider.held.lock().iter().all(mpsc::Sender::is_closed));
    h.close().await;
}

#[tokio::test]
async fn cancellation_releases_output_backpressure_and_audit_failure_stops_provider() {
    let mut h = Harness::new().await;
    let provider = Provider::new(vec![Plan::Open(vec![
        Ok(text("one")),
        Ok(text("two")),
        Ok(text("three")),
    ])]);
    let agent = agent(provider.clone(), Vec::new(), &h);
    let (sender, mut receiver) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { agent.run(turn(), sender, cancel).await }
    });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("first delta"),
        Some(text("one"))
    );
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("bounded cancellation")
            .expect("joins"),
        Err(Error::Cancelled)
    ));
    assert!(provider.held.lock().iter().all(mpsc::Sender::is_closed));
    h.close().await;
    let provider = Provider::new(vec![chunks(vec![ModelChunk::Done])]);
    let runner = crate::support::agent(provider.clone(), Vec::new(), &h);
    let (sender, _receiver) = mpsc::channel(1);
    assert!(matches!(
        runner.run(turn(), sender, CancellationToken::new()).await,
        Err(Error::Hook(_))
    ));
    assert!(provider.requests.lock().is_empty());
}

struct RendezvousTool {
    definition: rmcp::model::Tool,
    barrier: Arc<Barrier>,
    dropped: CancellationToken,
    hang: bool,
}

#[async_trait]
impl Tool for RendezvousTool {
    fn definition(&self) -> rmcp::model::Tool {
        self.definition.clone()
    }
    async fn call(
        &self,
        _arguments: Option<rmcp::model::JsonObject>,
        _ctx: ToolContext,
    ) -> Result<rmcp::model::CallToolResult, ToolError> {
        let _guard = self.dropped.clone().drop_guard();
        self.barrier.wait().await;
        if self.hang {
            std::future::pending::<()>().await;
        }
        Ok(rmcp::model::CallToolResult::success(vec![
            rmcp::model::ContentBlock::text("parallel result"),
        ]))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_tools_run_concurrently_and_results_remain_in_model_order() {
    let mut h = Harness::new().await;
    let mut response = call(1, "parallel", "{}");
    response.extend(call(0, "parallel", "{}"));
    response.push(ModelChunk::Done);
    let provider = Provider::new(vec![
        chunks(response),
        chunks(vec![text("done"), ModelChunk::Done]),
    ]);
    let tool = Arc::new(RendezvousTool {
        definition: testkit::FakeTool::new("parallel")
            .expect("echo tool schema")
            .definition(),
        barrier: Arc::new(Barrier::new(2)),
        dropped: CancellationToken::new(),
        hang: false,
    });
    let agent = agent(provider, vec![tool], &h);
    let mut input = turn();
    input.authorized_tools = names(&["parallel"]);
    let (sender, _receiver) = mpsc::channel(32);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        agent.run(input, sender, CancellationToken::new()),
    )
    .await
    .expect("both tools reach barrier")
    .expect("turn");
    let ids = result
        .messages
        .iter()
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["call-0", "call-1"]);
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_joins_tool_tasks_before_returning() {
    let mut h = Harness::new().await;
    let provider = Provider::new(vec![chunks(
        [call(0, "parallel", "{}"), vec![ModelChunk::Done]].concat(),
    )]);
    let barrier = Arc::new(Barrier::new(2));
    let dropped = CancellationToken::new();
    let tool = Arc::new(RendezvousTool {
        definition: testkit::FakeTool::new("parallel")
            .expect("echo tool schema")
            .definition(),
        barrier: Arc::clone(&barrier),
        dropped: dropped.clone(),
        hang: true,
    });
    let agent = agent(provider, vec![tool], &h);
    let mut input = turn();
    input.authorized_tools = names(&["parallel"]);
    let (sender, _receiver) = mpsc::channel(16);
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { agent.run(input, sender, cancel).await }
    });
    tokio::time::timeout(Duration::from_secs(5), barrier.wait())
        .await
        .expect("tool entered");
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("turn ends")
            .expect("joins"),
        Err(Error::Cancelled)
    ));
    assert!(
        dropped.is_cancelled(),
        "tool future was dropped before turn returned"
    );
    h.close().await;
}
