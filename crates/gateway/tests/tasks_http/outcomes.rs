use chrono::DateTime;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use uuid::Uuid;
use wire::gateway::catalog::OutcomeBucket;
use wire::gateway::catalog::OutcomesResponse;
use wire::task::AcceptanceCriterion;
use wire::task::CriterionKind;
use wire::task::EvalOutcome;
use wire::task::EvalVerdict;
use wire::task::TaskExecutorKind;
use wire::task::TaskResult;
use wire::task::TaskStatus;
use wire::task::TaskUsage;
use wire::task::TaskVerdict;

use super::Setup;
use super::call;
use super::json as parse;
use super::message;
use super::record;
use super::stamp;

fn criterion() -> AcceptanceCriterion {
    AcceptanceCriterion {
        id: "c".to_owned(),
        description: "done".to_owned(),
        kind: CriterionKind::Manual,
        check: None,
    }
}

fn usage(cost: f64) -> TaskUsage {
    TaskUsage {
        input_tokens: 1,
        output_tokens: 1,
        total_tokens: 2,
        cost_usd: Some(cost),
        turns: 1,
        wall_clock_ms: 10,
    }
}

fn result(verdict: TaskVerdict) -> TaskResult {
    TaskResult {
        verdict,
        summary: "s".to_owned(),
        output: None,
        artifacts: Vec::new(),
        criteria_self_report: None,
        usage: usage(0.0),
        error: None,
    }
}

fn verdict(kind: EvalVerdict) -> EvalOutcome {
    EvalOutcome {
        verdict: kind,
        attempts: 1,
        verifier_task_ids: vec!["verifier".to_owned()],
        criteria_report: Vec::new(),
        diverged: kind == EvalVerdict::Refuted,
        escalated_at: None,
    }
}

fn qualify(id: u128, agent: &str, day: u32) -> TaskRecord {
    let mut task = record(Uuid::from_u128(id), agent, stamp(day));
    task.status = TaskStatus::Completed;
    task.completed_at = Some(stamp(day));
    task.acceptance_criteria = vec![criterion()];
    task.result = Some(result(TaskVerdict::Completed));
    task
}

fn alpha() -> TaskRecord {
    let mut task = qualify(10, "alpha", 14);
    task.usage = Some(usage(1.5));
    task.duration_ms = Some(100);
    task.eval = Some(verdict(EvalVerdict::Refuted));
    task
}

fn beta() -> TaskRecord {
    let mut task = qualify(11, "beta", 15);
    task.executor = TaskExecutorKind::Mesh;
    task.usage = Some(usage(0.5));
    task.duration_ms = Some(300);
    task.eval = Some(verdict(EvalVerdict::Verified));
    task
}

fn share(
    verified: u64,
    refuted: u64,
    diverged: u64,
    rate: f64,
    cost: f64,
    avg: u64,
) -> OutcomeBucket {
    OutcomeBucket {
        tasks: 1,
        completed: 1,
        failed: 0,
        verified,
        refuted,
        escalated: 0,
        diverged,
        divergence_rate: rate,
        total_cost_usd: Some(cost),
        avg_duration_ms: Some(avg),
    }
}

fn alpha_share() -> OutcomeBucket {
    share(0, 1, 1, 1.0, 1.5, 100)
}

fn beta_share() -> OutcomeBucket {
    share(1, 0, 0, 0.0, 0.5, 300)
}

fn totals_bucket() -> OutcomeBucket {
    OutcomeBucket {
        tasks: 2,
        completed: 2,
        failed: 0,
        verified: 1,
        refuted: 1,
        escalated: 0,
        diverged: 1,
        divergence_rate: 0.5,
        total_cost_usd: Some(2.0),
        avg_duration_ms: Some(200),
    }
}

fn zero() -> OutcomeBucket {
    OutcomeBucket {
        tasks: 0,
        completed: 0,
        failed: 0,
        verified: 0,
        refuted: 0,
        escalated: 0,
        diverged: 0,
        divergence_rate: 0.0,
        total_cost_usd: None,
        avg_duration_ms: None,
    }
}

async fn seeded() -> Setup {
    let setup = Setup::new();
    setup.memory.insert(&alpha()).await.expect("alpha");
    setup.memory.insert(&beta()).await.expect("beta");
    setup
}

async fn outcomes(setup: &Setup, query: &str) -> OutcomesResponse {
    let (status, bytes) = call(
        &setup.router(),
        "GET",
        &format!("/api/outcomes{query}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    parse(&bytes)
}

#[tokio::test]
async fn outcomes_roll_up_terminal_work() {
    let setup = seeded().await;
    let body = outcomes(&setup, "").await;
    assert_eq!(body.filter.since, None);
    assert_eq!(body.filter.until, None);
    assert_eq!(body.filter.agent_id, None);
    assert_eq!(body.filter.origin, None);
    assert_eq!(body.totals, totals_bucket());
    assert_eq!(body.by_agent.get("alpha").expect("alpha"), &alpha_share());
    assert_eq!(body.by_agent.get("beta").expect("beta"), &beta_share());
    assert_eq!(
        body.by_executor.get("chat-loop").expect("chat"),
        &alpha_share()
    );
    assert_eq!(body.by_executor.get("mesh").expect("mesh"), &beta_share());
    assert_eq!(
        body.by_day.get("2026-10-14").expect("day 14"),
        &alpha_share()
    );
    assert_eq!(
        body.by_day.get("2026-10-15").expect("day 15"),
        &beta_share()
    );
    assert_eq!(body.totals.escalated, 0);
}

#[tokio::test]
async fn outcomes_filters_are_exact() {
    let setup = seeded().await;
    let by_agent = outcomes(&setup, "?agentId=alpha").await;
    assert_eq!(by_agent.filter.agent_id.as_deref(), Some("alpha"));
    assert_eq!(by_agent.totals.tasks, 1);
    assert!(by_agent.by_agent.contains_key("alpha"));
    assert!(!by_agent.by_agent.contains_key("beta"));
    let by_origin = outcomes(&setup, "?origin=api").await;
    assert_eq!(by_origin.filter.origin.as_deref(), Some("api"));
    assert_eq!(by_origin.totals.tasks, 2);
    let none = outcomes(&setup, "?origin=chat").await;
    assert_eq!(none.totals, zero());
    let (status, bytes) = call(&setup.router(), "GET", "/api/outcomes?origin=nope", None).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "unknown origin \"nope\"");
}

#[tokio::test]
async fn excluded_rows_do_not_count() {
    let setup = Setup::new();
    let mut eval = alpha();
    eval.id = Uuid::from_u128(20);
    eval.origin = TaskOrigin::Eval;
    let mut empty = alpha();
    empty.id = Uuid::from_u128(21);
    empty.acceptance_criteria.clear();
    let mut queued = alpha();
    queued.id = Uuid::from_u128(22);
    queued.status = TaskStatus::Queued;
    queued.completed_at = None;
    for task in [&eval, &empty, &queued] {
        setup.memory.insert(task).await.expect("insert");
    }
    let body = outcomes(&setup, "").await;
    assert_eq!(body.totals, zero());
    assert_eq!(body.by_day.len(), 0);
}

#[tokio::test]
async fn audit_only_rows_stay_out_of_the_rollup() {
    let setup = Setup::new();
    let mut hidden = alpha();
    hidden.id = Uuid::from_u128(40);
    hidden.spec.audit_only = Some(true);
    let mut visible = alpha();
    visible.id = Uuid::from_u128(41);
    visible.spec.audit_only = Some(false);
    setup.memory.insert(&hidden).await.expect("hidden");
    setup.memory.insert(&visible).await.expect("visible");
    let body = outcomes(&setup, "").await;
    assert_eq!(body.totals.tasks, 1);
}

#[tokio::test]
async fn a_killed_task_counts_without_completed_or_failed() {
    let setup = Setup::new();
    let mut task = qualify(30, "alpha", 14);
    task.status = TaskStatus::Killed;
    task.result = None;
    task.eval = None;
    task.usage = None;
    task.duration_ms = None;
    setup.memory.insert(&task).await.expect("insert");
    let body = outcomes(&setup, "").await;
    let mut expected = zero();
    expected.tasks = 1;
    assert_eq!(body.totals, expected);
    assert_eq!(body.totals.completed, 0);
    assert_eq!(body.totals.failed, 0);
    assert_eq!(body.by_day.get("2026-10-14").expect("day").tasks, 1);
}

#[tokio::test]
async fn since_and_until_are_inclusive() {
    let setup = seeded().await;
    let mut before = alpha();
    before.id = Uuid::from_u128(12);
    before.agent_id = "gamma".to_owned();
    before.completed_at = Some(stamp(14) - chrono::Duration::milliseconds(1));
    let mut after = beta();
    after.id = Uuid::from_u128(13);
    after.agent_id = "delta".to_owned();
    after.completed_at = Some(stamp(15) + chrono::Duration::milliseconds(1));
    setup.memory.insert(&before).await.expect("before");
    setup.memory.insert(&after).await.expect("after");
    let body = outcomes(
        &setup,
        "?since=2026-10-14T00:00:00Z&until=2026-10-15T00:00:00Z",
    )
    .await;
    assert_eq!(body.filter.since, Some(stamp(14)));
    assert_eq!(body.filter.until, Some(stamp(15)));
    assert_eq!(body.totals.tasks, 2);
    assert!(body.by_agent.contains_key("alpha"));
    assert!(body.by_agent.contains_key("beta"));
    assert!(!body.by_agent.contains_key("gamma"));
    assert!(!body.by_agent.contains_key("delta"));
}

#[tokio::test]
async fn dates_accept_epoch_millis_and_calendar_days() {
    let setup = seeded().await;
    let millis = stamp(15).timestamp_millis();
    let body = outcomes(&setup, &format!("?since={millis}")).await;
    assert_eq!(body.filter.since, DateTime::from_timestamp_millis(millis));
    assert_eq!(body.totals.tasks, 1);
    assert!(body.by_agent.contains_key("beta"));
    let body = outcomes(&setup, "?since=2026-10-15").await;
    assert_eq!(body.filter.since, Some(stamp(15)));
    assert_eq!(body.totals.tasks, 1);
    let (status, bytes) = call(
        &setup.router(),
        "GET",
        "/api/outcomes?since=10%2F14%2F2026",
        None,
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "invalid since");
    let (status, bytes) = call(&setup.router(), "GET", "/api/outcomes?until=nope", None).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "invalid until");
}

#[tokio::test]
async fn a_task_without_completed_at_needs_open_bounds() {
    let setup = Setup::new();
    let mut task = qualify(40, "alpha", 14);
    task.completed_at = None;
    task.duration_ms = Some(10);
    task.usage = None;
    task.eval = None;
    setup.memory.insert(&task).await.expect("insert");
    let open = outcomes(&setup, "").await;
    assert_eq!(open.totals.tasks, 1);
    assert_eq!(open.by_day.len(), 0);
    assert_eq!(open.totals.avg_duration_ms, Some(10));
    let bounded = outcomes(&setup, "?since=2026-10-01").await;
    assert_eq!(bounded.totals, zero());
}

#[tokio::test]
async fn outcome_query_hides_store_errors() {
    let setup = Setup::new();
    setup.memory.fail_next_list();
    let (status, bytes) = call(&setup.router(), "GET", "/api/outcomes", None).await;
    assert_eq!(status, 500, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "internal error");
    setup.memory.fail_next_list();
    let (status, bytes) = call(&setup.router(), "GET", "/api/tasks", None).await;
    assert_eq!(status, 500, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "graphile worker queue is absent");
    let tasks: Vec<TaskRecord> = setup
        .memory
        .list(&TaskListFilter::default())
        .await
        .expect("list still works");
    assert_eq!(tasks.len(), 0);
}

#[tokio::test]
async fn a_negative_duration_fails_the_rollup() {
    let setup = Setup::new();
    let mut task = qualify(41, "alpha", 14);
    task.duration_ms = Some(-5);
    setup.memory.insert(&task).await.expect("insert");
    let (status, bytes) = call(&setup.router(), "GET", "/api/outcomes", None).await;
    assert_eq!(status, 500, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "stored duration is negative");
}

#[tokio::test]
async fn outcomes_reject_other_methods() {
    let setup = Setup::new();
    let (status, bytes) = call(&setup.router(), "POST", "/api/outcomes", Some("{}")).await;
    assert_eq!(status, 405, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "method not allowed");
}
