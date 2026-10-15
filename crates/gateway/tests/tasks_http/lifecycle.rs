use std::sync::Arc;
use std::sync::atomic::Ordering;

use kernel::tasks::ApprovalDecision;
use kernel::tasks::TaskJobs;
use kernel::tasks::TaskStore;
use uuid::Uuid;
use wire::task::TaskApprovalAccepted;
use wire::task::TaskApprovalWaitResponse;
use wire::task::TaskKillResponse;
use wire::task::TaskResponse;
use wire::task::TaskStatus;
use wire::task::TaskSteerAccepted;
use wire::task::TaskWaitTimeoutResponse;
use wire::task::TasksListResponse;

use super::Broker;
use super::Setup;
use super::approval;
use super::call;
use super::claim;
use super::goal;
use super::json as parse;
use super::message;
use super::record;
use super::stamp;

async fn send(setup: &Setup, method: &str, target: &str, body: Option<&str>) -> (u16, Vec<u8>) {
    call(&setup.router(), method, target, body).await
}

async fn created(setup: &Setup) -> TaskResponse {
    let (status, bytes) = send(setup, "POST", "/api/tasks", Some(&goal("ship"))).await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&bytes));
    parse(&bytes)
}

fn brokered() -> (Setup, Arc<Broker>) {
    let broker = Arc::new(Broker::default());
    let mut setup = Setup::new();
    setup.broker = Some(Arc::clone(&broker));
    (setup, broker)
}

async fn seed() -> (Setup, Uuid, Uuid, Uuid) {
    let setup = Setup::new();
    let early = Uuid::from_u128(1);
    let low = Uuid::from_u128(2);
    let high = Uuid::from_u128(3);
    let mut first = record(early, "alpha", stamp(14));
    first.status = TaskStatus::Completed;
    for task in [
        first,
        record(low, "beta", stamp(15)),
        record(high, "alpha", stamp(15)),
    ] {
        setup.memory.insert(&task).await.expect("insert");
    }
    (setup, early, low, high)
}

#[tokio::test]
async fn get_reads_one_task() {
    let (setup, early, _, _) = seed().await;
    let (status, bytes) = send(&setup, "GET", "/api/tasks/not-a-uuid", None).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Invalid task ID. Check the task link.");
    let missing = Uuid::from_u128(8);
    let (status, bytes) = send(&setup, "GET", &format!("/api/tasks/{missing}"), None).await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), format!("no task {missing}"));
    let (status, bytes) = send(&setup, "GET", &format!("/api/tasks/{early}"), None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let task = parse::<TaskResponse>(&bytes).task;
    assert_eq!(task.id, early.to_string());
    assert_eq!(task.goal, "goal");
    assert_eq!(task.pending_approvals, None);
}

#[tokio::test]
async fn get_includes_pending_approvals() {
    let (setup, broker) = brokered();
    let id = Uuid::from_u128(6);
    setup
        .memory
        .insert(&record(id, "alpha", stamp(14)))
        .await
        .expect("insert");
    broker.pending.lock().push(approval(id, "req-1"));
    let (status, bytes) = send(&setup, "GET", &format!("/api/tasks/{id}"), None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let pending = parse::<TaskResponse>(&bytes)
        .task
        .pending_approvals
        .expect("pending");
    let request = pending.first().expect("request");
    assert_eq!(request.request_id, "req-1");
    assert_eq!(request.task_id, id.to_string());
}

#[tokio::test]
async fn list_filters_sorts_and_limits() {
    let (setup, early, low, high) = seed().await;
    let (status, bytes) = send(&setup, "GET", "/api/tasks", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let listed = parse::<TasksListResponse>(&bytes);
    let ids: Vec<_> = listed.tasks.iter().map(|task| task.id.clone()).collect();
    assert_eq!(
        ids,
        vec![high.to_string(), low.to_string(), early.to_string()]
    );
    let (status, bytes) = send(
        &setup,
        "GET",
        "/api/tasks?status=completed&agentId=alpha",
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let listed = parse::<TasksListResponse>(&bytes);
    assert_eq!(listed.tasks.len(), 1);
    assert_eq!(listed.tasks.first().expect("row").id, early.to_string());
    let (status, bytes) = send(&setup, "GET", "/api/tasks?limit=1", None).await;
    assert_eq!(status, 200);
    assert_eq!(
        parse::<TasksListResponse>(&bytes)
            .tasks
            .first()
            .expect("row")
            .id,
        high.to_string()
    );
    let (status, bytes) = send(&setup, "GET", "/api/tasks?limit=0", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(parse::<TasksListResponse>(&bytes).tasks, Vec::new());
}

#[tokio::test]
async fn list_rejects_unknown_status_and_bad_limits() {
    let (setup, _, _, _) = seed().await;
    let (status, bytes) = send(&setup, "GET", "/api/tasks?status=nope", None).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "unknown status \"nope\"");
    for limit in ["-1", "1.5", "%201", "4294967296"] {
        let (status, bytes) = send(&setup, "GET", &format!("/api/tasks?limit={limit}"), None).await;
        assert_eq!(status, 400, "{limit}: {}", String::from_utf8_lossy(&bytes));
        assert_eq!(message(&bytes), "invalid limit");
    }
    let (status, bytes) = send(&setup, "GET", "/api/tasks?limit=%2B1", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(parse::<TasksListResponse>(&bytes).tasks.len(), 1);
}

#[tokio::test]
async fn steer_requires_a_message_and_rejects_a_terminal_task() {
    let setup = Setup::new();
    let created = created(&setup).await.task;
    for body in [r"{}", r#"{"message":""}"#, r#"{"message":null}"#] {
        let (status, bytes) = send(
            &setup,
            "POST",
            &format!("/api/tasks/{}/steer", created.id),
            Some(body),
        )
        .await;
        assert_eq!(status, 400, "{body}: {}", String::from_utf8_lossy(&bytes));
        assert_eq!(message(&bytes), "message (string) is required");
    }
    let id = Uuid::from_u128(7);
    let mut done = record(id, "alpha", stamp(14));
    done.status = TaskStatus::Completed;
    setup.memory.insert(&done).await.expect("insert");
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/steer"),
        Some(r#"{"message":"again"}"#),
    )
    .await;
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "task is terminal (completed)");
    let missing = Uuid::from_u128(8);
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{missing}/steer"),
        Some(r#"{"message":"hi"}"#),
    )
    .await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), format!("no task {missing}"));
}

#[tokio::test]
async fn steer_sets_pending_and_enqueues_another_job() {
    let setup = Setup::new();
    let created = created(&setup).await.task;
    let first = claim(&setup.memory, "run-task").await.expect("first job");
    setup.memory.complete(first.id).await.expect("complete");
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{}/steer", created.id),
        Some(r#"{"message":"  turn"}"#),
    )
    .await;
    assert_eq!(status, 202, "{}", String::from_utf8_lossy(&bytes));
    assert!(parse::<TaskSteerAccepted>(&bytes).ok);
    let second = claim(&setup.memory, "run-task").await.expect("second job");
    assert_eq!(second.identifier, "run-task");
    let stored = setup
        .memory
        .load(Uuid::parse_str(&created.id).expect("id"))
        .await
        .expect("load");
    assert_eq!(stored.pending_message.as_deref(), Some("  turn"));
}

#[tokio::test]
async fn kill_returns_the_prior_status() {
    let setup = Setup::new();
    let created = created(&setup).await.task;
    let target = format!("/api/tasks/{}/kill", created.id);
    let (status, bytes) = send(&setup, "POST", &target, None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        parse::<TaskKillResponse>(&bytes),
        TaskKillResponse {
            ok: true,
            prior: Some(TaskStatus::Queued),
        }
    );
    let (status, bytes) = send(&setup, "POST", &target, None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        parse::<TaskKillResponse>(&bytes).prior,
        Some(TaskStatus::Killed)
    );
    let missing = Uuid::from_u128(8);
    let (status, bytes) = send(&setup, "POST", &format!("/api/tasks/{missing}/kill"), None).await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), format!("no task {missing}"));
}

#[tokio::test]
async fn approvals_follow_the_decision_order() {
    let (setup, broker) = brokered();
    let created = created(&setup).await.task;
    let id = Uuid::parse_str(&created.id).expect("id");
    broker.pending.lock().push(approval(id, "req-allow"));
    broker.pending.lock().push(approval(id, "req-deny"));
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/req-allow"),
        Some(r#"{"decision":"allow"}"#),
    )
    .await;
    assert_eq!(status, 202, "{}", String::from_utf8_lossy(&bytes));
    assert!(parse::<TaskApprovalAccepted>(&bytes).ok);
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/req-deny"),
        Some(r#"{"decision":"deny"}"#),
    )
    .await;
    assert_eq!(status, 202, "{}", String::from_utf8_lossy(&bytes));
    assert!(
        broker
            .pending
            .lock()
            .iter()
            .all(|item| item.task_id != id.to_string())
    );
    let resolved = broker.resolved.lock().clone();
    assert_eq!(resolved.first().expect("allow").1, ApprovalDecision::Allow);
    assert_eq!(resolved.get(1).expect("deny").1, ApprovalDecision::Deny);

    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/missing"),
        Some(r#"{"decision":"allow"}"#),
    )
    .await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "unknown approval");
}

#[tokio::test]
async fn approval_decision_must_be_allow_or_deny() {
    let (setup, _) = brokered();
    let created = created(&setup).await.task;
    let id = &created.id;
    for body in [
        r#"{"decision":"allow-session"}"#,
        "{}",
        r#"{"decision":null}"#,
        r#"{"decision":"Allow"}"#,
    ] {
        let (status, bytes) = send(
            &setup,
            "POST",
            &format!("/api/tasks/{id}/approvals/req"),
            Some(body),
        )
        .await;
        assert_eq!(status, 400, "{body}: {}", String::from_utf8_lossy(&bytes));
        assert_eq!(message(&bytes), "decision must be \"allow\" or \"deny\"");
    }
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/req"),
        Some(r#"{"decision":1}"#),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert!(message(&bytes).contains("invalid type"));
}

#[tokio::test]
async fn approval_without_a_broker_is_unknown() {
    let setup = Setup::new();
    let id = Uuid::now_v7();
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/req"),
        Some(r#"{"decision":"nope"}"#),
    )
    .await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "unknown approval");
}

#[tokio::test]
async fn approval_of_a_missing_task_is_not_found_after_a_valid_decision() {
    let (setup, _) = brokered();
    let id = Uuid::now_v7();
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/req"),
        Some(r#"{"decision":"allow-session"}"#),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "decision must be \"allow\" or \"deny\"");
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/req"),
        Some(r#"{"decision":"allow"}"#),
    )
    .await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), format!("no task {id}"));
}

#[tokio::test]
async fn a_terminal_task_denies_pending_approvals() {
    let (setup, broker) = brokered();
    let created = created(&setup).await.task;
    let id = &created.id;
    let (status, _) = send(&setup, "POST", &format!("/api/tasks/{id}/kill"), None).await;
    assert_eq!(status, 200);
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/req"),
        Some(r#"{"decision":"allow"}"#),
    )
    .await;
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "task is terminal (killed)");
    let denied = broker.denied.lock().clone();
    let (_, reason) = denied.first().expect("denied");
    assert_eq!(reason, "task is terminal");
}

#[tokio::test]
async fn deny_pending_failure_is_internal() {
    let (setup, broker) = brokered();
    let created = created(&setup).await.task;
    let id = &created.id;
    let (status, _) = send(&setup, "POST", &format!("/api/tasks/{id}/kill"), None).await;
    assert_eq!(status, 200);
    broker.deny_fails.store(true, Ordering::SeqCst);
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals/req"),
        Some(r#"{"decision":"deny"}"#),
    )
    .await;
    assert_eq!(status, 500, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "unknown approval");
    assert_eq!(broker.denied.lock().len(), 0);
}

#[tokio::test]
async fn approval_routes_require_a_request_id() {
    let setup = Setup::new();
    let id = Uuid::now_v7();
    let (status, bytes) = send(
        &setup,
        "POST",
        &format!("/api/tasks/{id}/approvals"),
        Some(r#"{"decision":"allow"}"#),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "requestId is required");
    let (status, bytes) = send(
        &setup,
        "POST",
        "/api/tasks/not-a-uuid/approvals",
        Some("{}"),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Invalid task ID. Check the task link.");
    let (status, bytes) = send(
        &setup,
        "POST",
        "/api/tasks/not-a-uuid/approvals/req",
        Some(r#"{"decision":"allow"}"#),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "Invalid task ID. Check the task link.");
}

#[tokio::test]
async fn get_wait_returns_a_terminal_task_or_times_out() {
    let (setup, early, low, _) = seed().await;
    let (status, bytes) = send(
        &setup,
        "GET",
        &format!("/api/tasks/{early}/wait?timeoutMs=1000"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(
        parse::<TaskResponse>(&bytes).task.status,
        TaskStatus::Completed
    );
    let (status, bytes) = send(
        &setup,
        "GET",
        &format!("/api/tasks/{low}/wait?timeoutMs=1"),
        None,
    )
    .await;
    assert_eq!(status, 504, "{}", String::from_utf8_lossy(&bytes));
    let timed: TaskWaitTimeoutResponse = parse(&bytes);
    assert_eq!(timed.error, "wait deadline exceeded");
    assert_eq!(timed.task, None);
    let stored = setup.memory.load(low).await.expect("load");
    assert_eq!(stored.status, TaskStatus::Queued);
    let missing = Uuid::from_u128(8);
    let (status, bytes) = send(
        &setup,
        "GET",
        &format!("/api/tasks/{missing}/wait?timeoutMs=1"),
        None,
    )
    .await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), format!("no task {missing}"));
}

#[tokio::test]
async fn get_wait_returns_an_approval_only_for_the_exact_flag() {
    let (setup, broker) = brokered();
    let id = Uuid::from_u128(6);
    setup
        .memory
        .insert(&record(id, "alpha", stamp(14)))
        .await
        .expect("insert");
    broker.pending.lock().push(approval(id, "req-1"));
    let (status, bytes) = send(
        &setup,
        "GET",
        &format!("/api/tasks/{id}/wait?onApproval=return&timeoutMs=1000"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let body: TaskApprovalWaitResponse = parse(&bytes);
    assert_eq!(body.approval.request_id, "req-1");
    let (status, bytes) = send(
        &setup,
        "GET",
        &format!("/api/tasks/{id}/wait?onApproval=yes&timeoutMs=1"),
        None,
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "onApproval must be \"return\"");
    let (status, bytes) = send(
        &setup,
        "GET",
        &format!("/api/tasks/{id}/wait?timeoutMs=1"),
        None,
    )
    .await;
    assert_eq!(status, 504, "{}", String::from_utf8_lossy(&bytes));
    let timed: TaskWaitTimeoutResponse = parse(&bytes);
    assert_eq!(timed.error, "wait deadline exceeded");
    assert_eq!(timed.task, None);
    assert_eq!(
        setup.memory.load(id).await.expect("load").status,
        TaskStatus::Queued
    );
}

#[tokio::test]
async fn other_methods_are_not_allowed() {
    let setup = Setup::new();
    let id = Uuid::now_v7();
    let cases = [
        ("DELETE", "/api/tasks"),
        ("POST", "/api/catalog"),
        ("POST", "/api/outcomes"),
        ("PUT", &format!("/api/tasks/{id}")),
    ];
    for (method, target) in cases {
        let (status, bytes) = send(&setup, method, target, None).await;
        assert_eq!(
            status,
            405,
            "{method} {target}: {}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(message(&bytes), "method not allowed");
    }
}
