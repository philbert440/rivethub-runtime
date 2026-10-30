#![cfg(test)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use chrono::DateTime;
use gateway::Gateway;
use gateway::tasks::Tasks;
use gateway::tenancy::{Tenancy, UserId};
use kernel::memory::MemoryStore;
use kernel::queue::ClaimSelector;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::tasks::TaskStore;
use testkit::FakeMemoryStore;
use testkit::Request;
use testkit::TaskMemory;
use uuid::Uuid;
use wire::gateway::GatewayError;
use wire::gateway::agents::AgentPreset;
use wire::gateway::agents::AgentsFile;
use wire::harness::HarnessId;
use wire::task::AcceptanceCriterion;
use wire::task::CriterionKind;
use wire::task::TaskBudget;
use wire::task::TaskCreateRequest;
use wire::task::TaskKillResponse;
use wire::task::TaskResponse;
use wire::task::TaskStatus as WireStatus;
use wire::task::TaskWaitTimeoutResponse;

use crate::common;

struct Node {
    app: Router,
    memory: TaskMemory,
    _root: tempfile::TempDir,
}

fn node() -> Node {
    let root = tempfile::tempdir().expect("sandbox");
    seed_presets(root.path());
    let memory = TaskMemory::new();
    let mut settings = common::settings();
    settings.state_dir = root.path().join("state");
    settings.directory_root = root.path().join("agents");
    let store: Arc<dyn MemoryStore> = Arc::new(FakeMemoryStore::default());
    let tasks = Tasks::new(
        Arc::new(memory.clone()),
        Arc::new(memory.clone()),
        Arc::new(memory.clone()),
        settings.node.clone(),
    );
    let gateway = Arc::new(
        Gateway::new(store, settings)
            .with_tenancy(Tenancy::local(
                UserId::new("owner".to_owned()).expect("owner"),
            ))
            .with_tasks(Arc::new(tasks)),
    );
    Node {
        app: gateway.router(false),
        memory,
        _root: root,
    }
}

fn seed_presets(root: &Path) {
    let preset = AgentPreset {
        id: "coder-id".to_owned(),
        name: "coder".to_owned(),
        color: "#336699".to_owned(),
        harness_id: Some(HarnessId::ClaudeCode),
        model: "default".to_owned(),
        effort: String::new(),
        system_prompt: String::new(),
        node: Some("node-b".to_owned()),
        directory: Some("/srv/coder".to_owned()),
        shared_link: None,
        node_base_url: None,
        sort_order: None,
        created_at: DateTime::UNIX_EPOCH,
        updated_at: DateTime::UNIX_EPOCH,
        extra: wire::Extra::default(),
    };
    std::fs::create_dir_all(root.join("state")).expect("state");
    std::fs::write(
        root.join("state").join("agents.json"),
        serde_json::to_vec(&AgentsFile {
            agents: vec![preset],
        })
        .expect("agents json"),
    )
    .expect("agents file");
}

async fn send(
    app: &Router,
    method: &str,
    target: &str,
    body: &impl serde::Serialize,
) -> testkit::Response {
    let request = Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers: vec![("Host".to_owned(), "127.0.0.1:5174".to_owned())],
        body: serde_json::to_vec(body).expect("body"),
        recipe: None,
        via: None,
    };
    tokio::time::timeout(Duration::from_secs(5), common::answer(app.clone(), request))
        .await
        .expect("task route timed out")
        .expect("task route request")
}

fn body<T: serde::de::DeserializeOwned>(response: &testkit::Response) -> T {
    serde_json::from_slice(&response.body)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&response.body)))
}

fn retired_fields() -> TaskCreateRequest {
    let extra: wire::Extra =
        serde_json::from_str(r#"{"executor": "chat-loop", "chainDepth": 0}"#).expect("extra");
    TaskCreateRequest {
        goal: Some("write the ledger".to_owned()),
        agent_id: Some("coder".to_owned()),
        acceptance_criteria: Some(vec![AcceptanceCriterion {
            id: "1".to_owned(),
            description: "it balances".to_owned(),
            kind: CriterionKind::Manual,
            check: None,
        }]),
        budget: Some(TaskBudget {
            max_turns: Some(4),
            max_wall_clock_ms: None,
        }),
        requested_by: Some("tests".to_owned()),
        extra,
        ..TaskCreateRequest::default()
    }
}

#[tokio::test]
async fn a_task_is_queued_for_its_agents_node_and_outlives_its_callers_wait() {
    let node = node();
    let created = send(
        &node.app,
        "POST",
        "/api/tasks?wait=1&timeoutMs=50",
        &retired_fields(),
    )
    .await;
    assert_eq!(
        created.status,
        504,
        "{}",
        String::from_utf8_lossy(&created.body)
    );
    let left: TaskWaitTimeoutResponse = body(&created);
    let task = left.task.expect("the task as it stands");
    assert_eq!(task.status, WireStatus::Queued);
    assert_eq!(task.node, "node-b");
    assert_eq!(task.harness, Some(HarnessId::ClaudeCode));
    assert_eq!(task.model, None);
    assert_eq!(task.budget.max_turns, Some(4));
    assert_eq!(task.acceptance_criteria.len(), 1);
    let id = Uuid::parse_str(&task.id).expect("task id");
    claimed_only_on_its_node(&node).await;
    killed(&node, id).await;
}

async fn claimed_only_on_its_node(node: &Node) {
    let elsewhere = ClaimSelector {
        names: vec![JobName::RunTaskOn {
            node: "golden-node".to_owned(),
        }],
        only_id: None,
    };
    assert!(
        node.memory
            .claim(&elsewhere)
            .await
            .expect("claim")
            .is_none()
    );
    let home = ClaimSelector {
        names: vec![JobName::RunTaskOn {
            node: "node-b".to_owned(),
        }],
        only_id: None,
    };
    assert!(node.memory.claim(&home).await.expect("claim").is_some());
}

async fn killed(node: &Node, id: Uuid) {
    let waited = send(
        &node.app,
        "GET",
        &format!("/api/tasks/{id}/wait?timeoutMs=20"),
        &(),
    )
    .await;
    assert_eq!(waited.status, 504);
    assert_eq!(
        node.memory.load(id).await.expect("load").status,
        kernel::tasks::TaskStatus::Queued
    );
    let killed = send(
        &node.app,
        "POST",
        &format!("/api/tasks/{id}/kill"),
        &wire::JsonObject::default(),
    )
    .await;
    assert_eq!(killed.status, 200);
    let killed: TaskKillResponse = body(&killed);
    assert_eq!(killed.prior, Some(WireStatus::Queued));
    let after = send(&node.app, "GET", &format!("/api/tasks/{id}/wait"), &()).await;
    let after: TaskResponse = body(&after);
    assert_eq!(after.task.status, WireStatus::Killed);
}

#[tokio::test]
async fn unknown_agents_and_retired_routes_are_refused() {
    let node = node();
    let unknown = send(
        &node.app,
        "POST",
        "/api/tasks",
        &TaskCreateRequest {
            goal: Some("x".to_owned()),
            agent_id: Some("nobody".to_owned()),
            ..TaskCreateRequest::default()
        },
    )
    .await;
    assert_eq!(unknown.status, 404);
    let refused: GatewayError = body(&unknown);
    assert!(refused.error.contains("nobody"), "{refused:?}");
    let missing = send(
        &node.app,
        "POST",
        "/api/tasks",
        &TaskCreateRequest {
            agent_id: Some("coder".to_owned()),
            ..TaskCreateRequest::default()
        },
    )
    .await;
    assert_eq!(missing.status, 400);
    for (method, path) in [
        (
            "POST",
            "/api/tasks/0199e2a0-7b3c-7a00-8000-000000000001/steer",
        ),
        (
            "POST",
            "/api/tasks/0199e2a0-7b3c-7a00-8000-000000000001/approvals/r1",
        ),
    ] {
        let gone = send(&node.app, method, path, &wire::JsonObject::default()).await;
        assert_eq!(gone.status, 404, "{path}");
    }
}
