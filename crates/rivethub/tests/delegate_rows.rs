use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use kernel::queue::JobQueue;
use kernel::tasks::{Delegation, Finish, TaskListFilter, TaskQuery, TaskRecord, TaskStore};
use memory_postgres::PostgresTasks;
use rmcp::model::CallToolRequestParams;
use serde::Deserialize;
use sqlx::{PgPool, Row};
use tasks::{Directory, Engine, LocalAgent, MeshIdentity, Settings};
use tokio_util::sync::CancellationToken;
use wire::JsonObject;
use wire::gateway::agents::AgentPreset;
use wire::harness::HarnessId;
use wire::mesh::{MeshNode, MeshNodeStatus};
use wire::task::{TaskResult, TaskStatus, TaskUsage, TaskVerdict};

#[derive(Deserialize)]
struct Fixture {
    scenarios: Vec<Scenario>,
    skipped: Vec<Skip>,
}

#[derive(Deserialize)]
struct Skip {
    name: String,
    reason: String,
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    arguments: JsonObject,
    expected: JsonObject,
    job: String,
}

#[expect(
    clippy::expect_used,
    reason = "fixed test directory and generated test certificate"
)]
fn directory() -> Directory {
    let mut directory =
        Directory::new("local", "http://127.0.0.1:9/".parse().expect("url")).expect("directory");
    directory.local_agent(
        "worker",
        LocalAgent {
            model: None,
            provider: None,
            effort: None,
            harness: None,
            system_prompt: None,
            working_dir: None,
        },
    );
    directory.preset(AgentPreset {
        id: "preset-id".into(),
        name: "preset".into(),
        color: "blue".into(),
        harness_id: Some(HarnessId::Codex),
        model: "preset-model".into(),
        effort: "low".into(),
        system_prompt: "be brief".into(),
        node: None,
        directory: None,
        shared_link: None,
        node_base_url: None,
        sort_order: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        extra: wire::Extra::default(),
    });
    directory.mesh_node(MeshNode {
        id: "edge".into(),
        name: "edge".into(),
        role: None,
        agents: vec!["worker".into(), "preset".into()],
        host: "127.0.0.1".into(),
        port: 9,
        providers: Vec::new(),
        models: Vec::new(),
        capabilities: Vec::new(),
        status: MeshNodeStatus::Online,
        last_seen: Utc::now(),
        registered_at: Utc::now(),
        version: "test".into(),
        metadata: None,
        ssh_user: None,
        install_root: None,
        platform: None,
        extra: wire::Extra::default(),
    });
    let certificate =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("certificate");
    directory.set_mesh_identity(Some(
        MeshIdentity::from_pem(
            &certificate.cert.pem(),
            &certificate.signing_key.serialize_pem(),
            &certificate.cert.pem(),
        )
        .expect("identity"),
    ));
    directory
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn mcp_delegation_rows_match_shapes(pool: PgPool) {
    let mut setup = pool.begin().await.expect("setup");
    let search_path: String = sqlx::query_scalar("SHOW search_path")
        .fetch_one(&mut *setup)
        .await
        .expect("search path");
    sqlx::raw_sql(include_str!("fixtures/graphile_worker.txt"))
        .execute(&mut *setup)
        .await
        .expect("graphile");
    sqlx::query("SELECT set_config('search_path', $1, false)")
        .bind(search_path)
        .execute(&mut *setup)
        .await
        .expect("restore search path");
    setup.commit().await.expect("setup commit");
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../testkit/tests/fixtures/tasks/delegate-rows.json"
    ))
    .expect("fixture");
    let mut skipped = BTreeSet::new();
    for skip in fixture.skipped {
        assert_ne!(skip.reason.trim(), "");
        assert!(skipped.insert(skip.name));
    }
    assert!(skipped.is_empty());
    for scenario in fixture.scenarios {
        replay(&pool, scenario).await;
    }
}

#[expect(
    clippy::expect_used,
    reason = "fixture setup and replay failures must fail the test"
)]
async fn replay(pool: &PgPool, scenario: Scenario) {
    let store = PostgresTasks::from_pool(pool.clone());
    let mut notifications = JobQueue::subscribe(&store).await.expect("subscribe");
    let engine = Arc::new(
        Engine::new(
            store.clone(),
            store.clone(),
            directory(),
            Settings {
                node: "local".into(),
                worker: "test".into(),
                files_root: None,
                pricing: std::collections::BTreeMap::new(),
                api_token: None,
            },
        )
        .expect("engine"),
    );
    let port: Arc<dyn Delegation> = engine.clone();
    let server = mcp::Server::builder(
        Arc::new(testkit::FakeMemoryStore::default()),
        false,
        CancellationToken::new(),
    )
    .delegation(port, mcp::Surface::Stdio)
    .build()
    .expect("server");
    let mut request = CallToolRequestParams::new("delegate_task");
    request.arguments = Some(
        serde_json::from_str(&serde_json::to_string(&scenario.arguments).expect("arguments"))
            .expect("mcp arguments"),
    );
    let mut pending = tokio::spawn(async move { server.call(request).await });
    tokio::select! {
        result = &mut pending => panic!("{} ended before enqueue: {result:?}", scenario.name),
        result = tokio::time::timeout(Duration::from_secs(10), notifications.recv()) => {
            result.expect("enqueue deadline").expect("enqueued");
        }
    }
    let tasks = store
        .list(&TaskListFilter {
            status: Some(TaskStatus::Queued),
            ..TaskListFilter::default()
        })
        .await
        .expect("tasks");
    assert_eq!(tasks.len(), 1);
    let task = tasks.first().expect("queued task");
    assert_queued(pool, task, &scenario).await;
    finish(&store, task).await;
    engine.wake_waiters();
    let reply = tokio::time::timeout(Duration::from_secs(10), pending)
        .await
        .expect("reply deadline")
        .expect("join")
        .expect("reply");
    assert_ne!(reply.is_error, Some(true));
    let stored: sqlx::types::Json<JsonObject> = sqlx::query_scalar(
        "SELECT to_jsonb(t) FROM (SELECT usage, result FROM rh_tasks WHERE id = $1) t",
    )
    .bind(task.id)
    .fetch_one(pool)
    .await
    .expect("finished row");
    let expected: JsonObject = serde_json::from_str(include_str!(
        "../../testkit/tests/fixtures/tasks/completed-columns.json"
    ))
    .expect("completed fixture");
    assert_eq!(stored.0, expected);
}

#[expect(
    clippy::expect_used,
    reason = "missing persisted task or job fails the replay"
)]
async fn assert_queued(pool: &PgPool, task: &TaskRecord, scenario: &Scenario) {
    let stored: sqlx::types::Json<JsonObject> = sqlx::query_scalar(
        "SELECT to_jsonb(t) FROM (SELECT executor, agent_id, origin, requested_by, \
         node_affinity, budget, spec FROM rh_tasks WHERE id = $1) t",
    )
    .bind(task.id)
    .fetch_one(pool)
    .await
    .expect("row");
    assert_eq!(stored.0, scenario.expected, "{}", scenario.name);
    assert_eq!(
        task.session_key.as_deref(),
        Some(task.spec.session.as_str())
    );
    assert_eq!(task.spec.session, format!("task:{}", task.id));
    assert_eq!(task.budget.max_turns, None);
    assert_eq!(task.budget.max_tokens, None);
    assert_eq!(task.budget.max_usd, None);
    assert!(task.usage.is_none());
    assert!(task.result.is_none());
    assert!(task.eval.is_none());
    let job = sqlx::query(
        "SELECT t.identifier, j.key, j.priority, j.max_attempts, \
         j.payload->>'taskId' AS task_id FROM graphile_worker._private_jobs j \
         JOIN graphile_worker._private_tasks t ON t.id = j.task_id \
         WHERE j.payload->>'taskId' = $1",
    )
    .bind(task.id.to_string())
    .fetch_one(pool)
    .await
    .expect("job");
    assert_eq!(
        job.try_get::<String, _>("identifier").expect("job column"),
        scenario.job
    );
    assert_eq!(
        job.try_get::<String, _>("task_id").expect("job column"),
        task.id.to_string()
    );
    assert_eq!(
        job.try_get::<String, _>("key").expect("job column"),
        format!("task:{}", task.id)
    );
    assert_eq!(job.try_get::<i16, _>("priority").expect("job column"), 0);
    assert_eq!(
        job.try_get::<i16, _>("max_attempts").expect("job column"),
        1
    );
}

#[expect(clippy::expect_used, reason = "a failed finish must fail the replay")]
async fn finish(store: &PostgresTasks, task: &TaskRecord) {
    let usage = TaskUsage {
        input_tokens: 1,
        output_tokens: 2,
        total_tokens: 3,
        cost_usd: None,
        turns: 1,
        wall_clock_ms: 10,
    };
    let result = TaskResult {
        verdict: TaskVerdict::Completed,
        summary: "done".into(),
        output: None,
        artifacts: Vec::new(),
        criteria_self_report: None,
        usage,
        error: None,
    };
    store
        .finish(
            task.id,
            &Finish {
                status: TaskStatus::Completed,
                result: Some(result),
                error: None,
                usage: Some(usage),
                completed_at: Utc::now(),
                duration_ms: Some(10),
            },
        )
        .await
        .expect("finish");
}
