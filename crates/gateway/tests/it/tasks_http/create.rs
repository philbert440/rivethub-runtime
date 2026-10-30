use chrono::Utc;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use uuid::Uuid;
use wire::gateway::GatewayError;
use wire::harness::HarnessId;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::ContextRefKind;
use wire::task::CriterionKind;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPromptMode;
use wire::task::TaskResponse;
use wire::task::TaskStatus;
use wire::task::TaskWaitTimeoutResponse;
use wire::task::TaskWire;

use super::Setup;
use super::call;
use super::claim;
use super::finished;
use super::goal;
use super::ignored_fields_body;
use super::json as parse;
use super::message;
use super::mode_of;
use super::null_requested_by;
use super::only_id;
use super::record;
use super::render;
use super::spec_for;
use super::stamp;
use super::task_post;

async fn send(setup: &Setup, target: &str, body: &str) -> (u16, Vec<u8>) {
    call(&setup.router(), "POST", target, Some(body)).await
}

async fn created(body: &str) -> TaskWire {
    let setup = Setup::new();
    let (status, bytes) = send(&setup, "/api/tasks", body).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    parse::<TaskResponse>(&bytes).task
}

async fn stored_count(setup: &Setup) -> usize {
    setup
        .memory
        .list(&TaskListFilter::default())
        .await
        .expect("list")
        .len()
}

#[tokio::test]
async fn create_applies_gateway_defaults() {
    let task = created(&ignored_fields_body()).await;
    let session = format!("task:{}", task.id);
    assert_eq!(task.goal, "ship");
    assert_eq!(task.agent_id, "alpha");
    assert_eq!(task.executor, TaskExecutorKind::ChatLoop);
    assert_eq!(task.requested_by.as_deref(), Some("gateway"));
    assert_eq!(task.origin, "api");
    assert_eq!(task.max_attempts, 1);
    assert_eq!(task.attempt, 0);
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(task.chain_depth, 0);
    assert_eq!(task.parent_task_id, None);
    assert_eq!(task.budget, TaskBudget::default());
    assert_eq!(task.session_key.as_deref(), Some(session.as_str()));
    assert_eq!(task.spec.task_id, task.id);
    assert_eq!(task.spec.session, session);
    assert_eq!(task.spec.prompt_mode, Some(TaskPromptMode::Task));
    assert_eq!(task.spec.agent_id, "alpha");
    let skew = Utc::now().signed_duration_since(task.created_at);
    assert!(skew.num_seconds().abs() < 5);
}

#[tokio::test]
async fn goal_text_is_stored_unchanged() {
    let text = "say \"hello\" $(date) .. café";
    let task = created(&goal(text)).await;
    assert_eq!(task.goal, text);
    assert_eq!(task.spec.goal, text);
}

#[tokio::test]
async fn requested_by_keeps_an_empty_string_and_fills_null() {
    let mut empty_body = task_post("ship");
    empty_body.requested_by = Some(String::new());
    let empty = created(&render(&empty_body)).await;
    assert_eq!(empty.requested_by.as_deref(), Some(""));
    let absent = created(&null_requested_by()).await;
    assert_eq!(absent.requested_by.as_deref(), Some("gateway"));
}

#[tokio::test]
async fn goal_and_agent_are_required_strings() {
    let cases = [
        ("{}", "goal (string) is required"),
        (
            r#"{"goal":"","agentId":"alpha"}"#,
            "goal (string) is required",
        ),
        (
            r#"{"goal":null,"agentId":"alpha"}"#,
            "goal (string) is required",
        ),
        (r#"{"goal":"ship"}"#, "agentId (string) is required"),
        (
            r#"{"goal":"ship","agentId":""}"#,
            "agentId (string) is required",
        ),
        (
            r#"{"goal":"ship","agentId":null}"#,
            "agentId (string) is required",
        ),
    ];
    for (body, expected) in cases {
        let setup = Setup::new();
        let (status, bytes) = send(&setup, "/api/tasks", body).await;
        assert_eq!(
            status,
            400,
            "{expected}: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(message(&bytes), expected);
        assert_eq!(stored_count(&setup).await, 0);
    }
}

#[tokio::test]
async fn a_non_string_goal_is_a_serde_error() {
    let setup = Setup::new();
    let (status, bytes) = send(&setup, "/api/tasks", r#"{"goal":1,"agentId":"alpha"}"#).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    let error = message(&bytes);
    assert_ne!(error, "goal (string) is required");
    assert!(error.contains("invalid type"), "{error}");
    assert_eq!(stored_count(&setup).await, 0);
}

#[tokio::test]
async fn unknown_executor_is_quoted() {
    let raw = "no\"pe";
    let setup = Setup::new();
    let mut body = task_post("ship");
    body.executor = Some(raw.to_owned());
    let body = render(&body);
    let (status, bytes) = send(&setup, "/api/tasks", &body).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    let quoted = serde_json::to_string(raw).expect("quote");
    assert_eq!(message(&bytes), format!("unknown executor {quoted}"));
}

#[tokio::test]
async fn unknown_agent_names_mesh_only_when_it_is_on() {
    let local = Setup::new();
    let (status, bytes) = send(&local, "/api/tasks", &ghost_body()).await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "agent \"ghost\" not found locally");
    let mut mesh = Setup::new();
    mesh.mesh = true;
    let (status, bytes) = send(&mesh, "/api/tasks", &ghost_body()).await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "agent \"ghost\" not found locally or on the mesh"
    );
}

#[tokio::test]
async fn harness_allow_list_applies_only_to_a_harness_session() {
    let allowed = harness_post(
        Some(vec![HarnessId::ClaudeCode]),
        "harness-session",
        "claude-code",
    )
    .await;
    assert_eq!(allowed.0, 201, "{}", String::from_utf8_lossy(&allowed.1));
    let task = parse::<TaskResponse>(&allowed.1).task;
    assert_eq!(task.executor, TaskExecutorKind::HarnessSession);
    assert_eq!(task.executor_target.as_deref(), Some("claude-code"));

    let denied = harness_post(Some(Vec::new()), "harness-session", "claude-code").await;
    assert_eq!(denied.0, 403, "{}", String::from_utf8_lossy(&denied.1));
    let mut expected = GatewayError::new(
        "harness \"claude-code\" is not allowed on this node (gateway.allowed_harnesses)",
    );
    expected = expected.code("harness_not_allowed");
    expected.harness_id = Some(HarnessId::ClaudeCode);
    assert_eq!(parse::<GatewayError>(&denied.1), expected);

    let open = harness_post(None, "harness-session", "claude-code").await;
    assert_eq!(open.0, 201, "{}", String::from_utf8_lossy(&open.1));
    let chat = harness_post(Some(Vec::new()), "chat-loop", "claude-code").await;
    assert_eq!(chat.0, 201, "{}", String::from_utf8_lossy(&chat.1));
    let custom = harness_post(Some(Vec::new()), "harness-session", "custom-bin").await;
    assert_eq!(custom.0, 201, "{}", String::from_utf8_lossy(&custom.1));
    let mesh = harness_post(Some(Vec::new()), "mesh", "claude-code").await;
    assert_eq!(mesh.0, 201, "{}", String::from_utf8_lossy(&mesh.1));
}

async fn harness_post(
    allowed: Option<Vec<HarnessId>>,
    executor: &str,
    target: &str,
) -> (u16, Vec<u8>) {
    let mut setup = Setup::new();
    setup.allowed = allowed;
    let mut body = task_post("ship");
    body.executor = Some(executor.to_owned());
    body.executor_target = Some(target.to_owned());
    send(&setup, "/api/tasks", &render(&body)).await
}

fn ghost_body() -> String {
    let mut body = task_post("ship");
    body.agent_id = "ghost";
    render(&body)
}

#[tokio::test]
async fn bodies_that_are_not_objects_are_rejected() {
    let cases = [
        ("", "invalid JSON"),
        ("   ", "invalid JSON"),
        ("{", "invalid JSON"),
        ("null", "body must be a JSON object"),
        ("\"x\"", "body must be a JSON object"),
        ("[]", "body must be a JSON object"),
    ];
    for (body, expected) in cases {
        let setup = Setup::new();
        let (status, bytes) = send(&setup, "/api/tasks", body).await;
        assert_eq!(
            status,
            400,
            "{expected}: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(message(&bytes), expected);
    }
}

#[tokio::test]
async fn oversized_body_is_rejected_before_insert() {
    let setup = Setup::new();
    let body = format!(
        "{{\"goal\":\"{}\",\"agentId\":\"alpha\"}}",
        "x".repeat(256 * 1024)
    );
    let (status, bytes) = send(&setup, "/api/tasks", &body).await;
    assert_eq!(status, 413, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "body too large");
    assert_eq!(stored_count(&setup).await, 0);
}

#[tokio::test]
async fn chain_rules_reject_a_broken_parent() {
    let missing = Uuid::now_v7();
    let setup = Setup::new();
    let (status, bytes) = send(
        &setup,
        "/api/tasks",
        &child_body(Some(missing.to_string()), Some(1)),
    )
    .await;
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "parent task is missing");
    assert_eq!(stored_count(&setup).await, 0);

    let (status, bytes) = send(
        &setup,
        "/api/tasks",
        &child_body(Some("nope".to_owned()), Some(1)),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "parent task id is not a uuid");

    let (status, bytes) = send(&setup, "/api/tasks", &child_body(None, Some(2))).await;
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "chain depth 2 does not follow parent depth 0"
    );

    let (status, bytes) = send(
        &setup,
        "/api/tasks",
        &child_body(Some(missing.to_string()), None),
    )
    .await;
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "chain depth is required when a parent task is set"
    );
}

#[tokio::test]
async fn chain_depth_must_be_one_past_the_parent() {
    let setup = Setup::new();
    let parent = created_on(&setup, &goal("parent")).await;
    let (status, bytes) = send(
        &setup,
        "/api/tasks",
        &child_body(Some(parent.id.clone()), Some(2)),
    )
    .await;
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        "chain depth 2 does not follow parent depth 0"
    );
    let (status, bytes) = send(
        &setup,
        "/api/tasks",
        &child_body(Some(parent.id.clone()), Some(1)),
    )
    .await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    let child = parse::<TaskResponse>(&bytes).task;
    assert_eq!(child.parent_task_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(child.chain_depth, 1);

    let deep_id = Uuid::from_u128(4);
    let mut deep = record(deep_id, "alpha", stamp(14));
    deep.chain_depth = 4;
    setup.memory.insert(&deep).await.expect("parent");
    let (status, bytes) = send(
        &setup,
        "/api/tasks",
        &named_child("next", Some(deep_id.to_string()), Some(5)),
    )
    .await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(parse::<TaskResponse>(&bytes).task.chain_depth, 5);
}

async fn created_on(setup: &Setup, body: &str) -> TaskWire {
    let (status, bytes) = send(setup, "/api/tasks", body).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    parse::<TaskResponse>(&bytes).task
}

#[tokio::test]
async fn chain_depth_overflow_is_a_conflict() {
    let setup = Setup::new();
    let parent_id = Uuid::from_u128(9);
    let mut parent = record(parent_id, "alpha", stamp(14));
    parent.chain_depth = u32::MAX;
    setup.memory.insert(&parent).await.expect("parent");
    let (status, bytes) = send(
        &setup,
        "/api/tasks",
        &child_body(Some(parent_id.to_string()), Some(0)),
    )
    .await;
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        message(&bytes),
        format!("chain depth 0 does not follow parent depth {}", u32::MAX)
    );
    assert_eq!(stored_count(&setup).await, 1);
}

#[tokio::test]
async fn enqueue_failure_finishes_the_task_failed() {
    let setup = Setup::new();
    setup.memory.fail_next_enqueue();
    let (status, bytes) = send(&setup, "/api/tasks", &goal("ship")).await;
    assert_eq!(status, 500, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "graphile worker queue is absent");
    let stored = setup
        .memory
        .load(only_id(&setup.memory).await)
        .await
        .expect("load");
    assert_eq!(stored.status, TaskStatus::Failed);
    assert_eq!(
        stored.error.as_deref(),
        Some("graphile worker queue is absent")
    );
}

#[tokio::test]
async fn heartbeat_session_is_kept_only_when_it_is_present() {
    let setup = Setup::new();
    let (status, bytes) = send(&setup, "/api/tasks", &spec_body("", "heartbeat")).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "spec session is empty");
    assert_eq!(stored_count(&setup).await, 0);

    let (status, bytes) = send(&setup, "/api/tasks", &spec_body("keep-me", "heartbeat")).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    let task = parse::<TaskResponse>(&bytes).task;
    assert_eq!(task.spec.session, "keep-me");
    assert_eq!(task.session_key.as_deref(), Some("keep-me"));
    assert_eq!(task.spec.prompt_mode, Some(TaskPromptMode::Heartbeat));
}

#[tokio::test]
async fn a_client_spec_wins_over_request_context_and_keeps_its_agent() {
    let setup = Setup::new();
    let mut body = task_post("ship");
    body.context_refs = Some(vec![file_ref("request.md")]);
    body.acceptance_criteria = Some(vec![criterion("r", "req")]);
    body.budget = Some(TaskBudget {
        max_turns: Some(3),
        ..TaskBudget::default()
    });
    body.spec = Some(spec_for(
        "kept-agent",
        "s",
        TaskPromptMode::Task,
        Some("../etc/passwd"),
        vec![file_ref("spec.md")],
        vec![criterion("s", "spec")],
    ));
    let body = render(&body);
    let (status, bytes) = send(&setup, "/api/tasks", &body).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    let task = parse::<TaskResponse>(&bytes).task;
    assert_eq!(task.goal, "ship");
    assert_eq!(task.spec.goal, "spec-goal");
    assert_eq!(task.spec.agent_id, "kept-agent");
    assert_eq!(task.spec.task_id, task.id);
    assert_eq!(task.spec.session, format!("task:{}", task.id));
    assert_eq!(task.spec.working_dir.as_deref(), Some("../etc/passwd"));
    assert_eq!(task.budget.max_turns, Some(3));
    assert_eq!(task.spec.budget.max_turns, Some(3));
    let context = task.context_refs.first().expect("context");
    assert_eq!(context.r#ref, "spec.md");
    let criterion = task.acceptance_criteria.first().expect("criterion");
    assert_eq!(criterion.id, "s");
}

#[tokio::test]
async fn spec_budget_is_used_when_the_request_has_none() {
    let task = created(&spec_body("s", "task")).await;
    assert_eq!(task.budget.max_turns, Some(9));
    assert_eq!(task.spec.budget.max_turns, Some(9));
}

fn spec_body(session: &str, mode: &str) -> String {
    let mut body = task_post("ship");
    body.spec = Some(spec_for(
        "alpha",
        session,
        mode_of(mode),
        None,
        Vec::new(),
        Vec::new(),
    ));
    render(&body)
}

fn child_body(parent: Option<String>, depth: Option<u32>) -> String {
    named_child("child", parent, depth)
}

fn named_child(goal_text: &str, parent: Option<String>, depth: Option<u32>) -> String {
    let mut body = task_post(goal_text);
    body.parent_task_id = parent;
    body.chain_depth = depth;
    render(&body)
}

fn file_ref(path: &str) -> ContextRef {
    ContextRef {
        kind: ContextRefKind::File,
        r#ref: path.to_owned(),
        note: None,
    }
}

fn criterion(id: &str, description: &str) -> AcceptanceCriterion {
    AcceptanceCriterion {
        id: id.to_owned(),
        description: description.to_owned(),
        kind: CriterionKind::Manual,
        check: None,
    }
}

#[tokio::test]
async fn node_affinity_picks_the_job_name() {
    assert_eq!(job_name(None).await, "run-task:local");
    assert_eq!(job_name(Some("")).await, "run-task:local");
    assert_eq!(job_name(Some("local")).await, "run-task:local");
    assert_eq!(job_name(Some("east")).await, "run-task:east");
}

async fn job_name(affinity: Option<&str>) -> String {
    let setup = Setup::new();
    let mut body = task_post("ship");
    body.node_affinity = affinity.map(str::to_owned);
    let (status, bytes) = send(&setup, "/api/tasks", &render(&body)).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    claim(&setup.memory, "run-task:local")
        .await
        .or(claim(&setup.memory, "run-task:east").await)
        .expect("job")
        .name
        .to_string()
}

#[tokio::test]
async fn wait_query_is_exact_and_timeout_is_clamped() {
    let setup = Setup::new();
    let (status, bytes) = send(&setup, "/api/tasks?wait=yes", &goal("ship")).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        parse::<TaskResponse>(&bytes).task.status,
        TaskStatus::Queued
    );
    let (status, bytes) = send(&setup, "/api/tasks?wait=TRUE", &goal("ship")).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));

    let (status, bytes) = send(&setup, "/api/tasks?timeoutMs=abc", &goal("ship")).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "invalid timeoutMs");
    assert_eq!(stored_count(&setup).await, 2);

    let (status, bytes) = send(&setup, "/api/tasks?wait=1&timeoutMs=0", &goal("ship")).await;
    assert_eq!(status, 504, "{}", String::from_utf8_lossy(&bytes));
    let timed: TaskWaitTimeoutResponse = parse(&bytes);
    assert_eq!(timed.error, "wait deadline exceeded");
    assert_eq!(timed.task.expect("task").status, TaskStatus::Queued);
}

#[tokio::test]
async fn post_wait_leaves_the_task_running_when_the_deadline_passes() {
    let setup = Setup::new();
    let (status, bytes) = send(&setup, "/api/tasks?wait=1&timeoutMs=1", &goal("ship")).await;
    assert_eq!(status, 504, "{}", String::from_utf8_lossy(&bytes));
    let timed: TaskWaitTimeoutResponse = parse(&bytes);
    assert_eq!(timed.error, "wait deadline exceeded");
    let task = timed.task.expect("task");
    assert_eq!(task.status, TaskStatus::Queued);
    let stored = setup
        .memory
        .load(only_id(&setup.memory).await)
        .await
        .expect("load");
    assert_eq!(stored.status, TaskStatus::Queued);
    assert_eq!(stored.error, None);
    assert_eq!(stored.id.to_string(), task.id);
}

#[tokio::test]
async fn post_wait_returns_the_task_when_it_finishes() {
    let setup = Setup::new();
    let mut receiver = setup.memory.subscribe();
    let memory = setup.memory.clone();
    let app = setup.router();
    let body = goal("ship");
    let posted = tokio::spawn(async move {
        call(
            &app,
            "POST",
            "/api/tasks?wait=true&timeoutMs=1500",
            Some(&body),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(1_000), receiver.changed())
        .await
        .expect("insert bump")
        .expect("watch open");
    let id = only_id(&memory).await;
    memory
        .finish(id, &finished(TaskStatus::Completed))
        .await
        .expect("finish");
    let (status, bytes) = posted.await.expect("join");
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let task = parse::<TaskResponse>(&bytes).task;
    assert_eq!(task.id, id.to_string());
    assert_eq!(task.status, TaskStatus::Completed);
}
