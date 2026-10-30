#[expect(
    dead_code,
    reason = "each test binary includes the shared harness and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "a shared harness reports setup failure by panicking; the message names the step"
)]
mod common;

use std::collections::BTreeMap;

use common::Script;
use common::engine_with;
use common::next_body;
use common::quiet;
use common::request;
use common::settings;
use common::task;
use tasks::Error;
use tasks::TaskOrigin;
use tasks::TokenRate;
use wire::task::TaskBudget;
use wire::task::TaskStatus;
use wire::task::TaskVerdict;

#[expect(
    clippy::expect_used,
    reason = "the rate literal is a fixed decimal the helper parses once for every budget case"
)]
fn priced() -> BTreeMap<String, TokenRate> {
    let mut rates = BTreeMap::new();
    rates.insert("demo".to_string(), TokenRate::new(3.0, 6.0).expect("rate"));
    rates
}

fn with_budget(goal: &str, budget: TaskBudget) -> tasks::NewTask {
    let mut created = request(goal);
    created.budget = Some(budget);
    tasks::NewTask {
        request: created,
        origin: TaskOrigin::Chat,
    }
}

#[tokio::test]
async fn a_priced_reply_records_the_exact_cost() {
    let (stub, mut rx) = common::chat_stub(Script {
        model: Some("demo"),
        prompt_tokens: Some(1_000_000),
        completion_tokens: Some(0),
        ..Script::reply("done")
    })
    .await;
    let base = common::base_url(stub.addr);
    let (engine, _) = engine_with(common::directory(base), settings(None, priced()));
    let id = engine.submit(task("ship it")).await.expect("submit");
    engine.run_one().await.expect("run");
    next_body(&mut rx);
    let usage = engine.load(id).await.expect("load").usage.expect("usage");
    assert_eq!(usage.cost_usd, Some(3.0));
    assert_eq!(usage.total_tokens, Some(1_000_000));
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn zero_turns_fail_before_the_gateway_is_called() {
    let (stub, mut rx) = common::chat_stub(Script::reply("done")).await;
    let engine = common::engine(common::base_url(stub.addr));
    let id = engine
        .submit(with_budget(
            "ship it",
            TaskBudget {
                max_turns: Some(0),
                ..TaskBudget::default()
            },
        ))
        .await
        .expect("submit");
    let error = engine.run_one().await.expect_err("budget");
    assert!(matches!(error, Error::Budget));
    quiet(&mut rx);
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Killed);
    assert_eq!(loaded.error.as_deref(), Some("maxTurns (0) reached"));
    let result = loaded.result.expect("result");
    assert_eq!(result.verdict, TaskVerdict::BudgetExceeded);
    assert_eq!(result.error.as_deref(), Some("maxTurns (0) reached"));
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn a_finished_reply_completes_whatever_its_token_turn_and_price_limits() {
    let (stub, _rx) = common::chat_stub(Script {
        prompt_tokens: Some(20),
        completion_tokens: Some(0),
        ..Script::reply("done")
    })
    .await;
    let engine = common::engine(common::base_url(stub.addr));
    for budget in [
        TaskBudget {
            max_tokens: Some(1),
            ..TaskBudget::default()
        },
        TaskBudget {
            max_usd: Some(1.0),
            ..TaskBudget::default()
        },
        TaskBudget {
            max_turns: Some(1),
            ..TaskBudget::default()
        },
    ] {
        let id = engine
            .submit(with_budget("ship it", budget))
            .await
            .expect("submit");
        engine.run_one().await.expect("run");
        let loaded = engine.load(id).await.expect("load");
        assert_eq!(loaded.status, TaskStatus::Completed, "{budget:?}");
        assert_eq!(loaded.error, None);
        assert_eq!(loaded.result.expect("result").summary, "done");
    }
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn a_named_model_without_a_rate_is_a_pricing_error() {
    let (stub, _rx) = common::chat_stub(Script {
        model: Some("missing"),
        prompt_tokens: Some(1),
        completion_tokens: Some(1),
        ..Script::reply("done")
    })
    .await;
    let engine = common::engine(common::base_url(stub.addr));
    let id = engine.submit(task("ship it")).await.expect("submit");
    assert!(matches!(
        engine.run_one().await.expect_err("price"),
        Error::Pricing
    ));
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Failed
    );
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn a_hung_turn_ends_as_a_timeout() {
    let (stub, _rx) = common::chat_stub(Script {
        hang: true,
        ..Script::reply("done")
    })
    .await;
    let engine = common::engine(common::base_url(stub.addr));
    let id = engine
        .submit(with_budget(
            "ship it",
            TaskBudget {
                max_wall_clock_ms: Some(200),
                ..TaskBudget::default()
            },
        ))
        .await
        .expect("submit");
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), engine.run_one())
        .await
        .expect("deadline")
        .expect_err("timeout");
    assert!(matches!(error, Error::Timeout));
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Timeout
    );
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn reported_usage_settles_on_the_task_and_its_result() {
    let (stub, mut rx) = common::chat_stub(Script {
        prompt_tokens: Some(23_248),
        completion_tokens: Some(4),
        ..Script::reply("done")
    })
    .await;
    let engine = common::engine(common::base_url(stub.addr));
    let id = engine.submit(task("ship it")).await.expect("submit");
    engine.run_one().await.expect("run");
    next_body(&mut rx);
    let loaded = engine.load(id).await.expect("load");
    let usage = loaded.usage.expect("usage");
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.total_tokens),
        (Some(23_248), Some(4), Some(23_252))
    );
    assert_eq!(loaded.result.expect("result").usage, usage);
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn unreported_usage_leaves_the_token_fields_out() {
    let (stub, mut rx) = common::chat_stub(Script::reply("done")).await;
    let engine = common::engine(common::base_url(stub.addr));
    let id = engine.submit(task("ship it")).await.expect("submit");
    engine.run_one().await.expect("run");
    next_body(&mut rx);
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Completed);
    let usage = loaded.usage.expect("usage");
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.total_tokens),
        (None, None, None)
    );
    assert_eq!(usage.turns, 1);
    let written = serde_json::to_value(usage).expect("usage serializes");
    for field in ["inputTokens", "outputTokens", "totalTokens"] {
        assert_eq!(written.get(field), None, "{field}");
    }
    stub.close().await.expect("stub shuts down");
}
