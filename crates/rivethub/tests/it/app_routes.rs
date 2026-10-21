use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use gateway::routes::tasks::TasksApi;
use kernel::harness::Command;
use kernel::harness::HarnessAdapter;
use kernel::harness::HarnessCommands;
use kernel::tasks::TaskAgents;
use kernel::tasks::TaskError;
use memory_postgres::PostgresTasks;
use sqlx::PgPool;
use tower::ServiceExt as _;
use wire::gateway::catalog::CatalogSheet;
use wire::gateway::catalog::OutcomesResponse;
use wire::harness::HarnessId;
use wire::task::EvalVerdict;
use wire::task::TasksListResponse;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
struct NoAgents;

#[async_trait]
impl TaskAgents for NoAgents {
    async fn contains(&self, _agent_id: &str) -> Result<bool, TaskError> {
        Ok(false)
    }
}

const TS_ROWS: &str = r#"
INSERT INTO rh_tasks (id, goal, context_refs, acceptance_criteria, spec, executor, agent_id,
    requested_by, origin, chain_depth, node_affinity, budget, usage, status, attempt, max_attempts,
    error, result, session_key, harness_session_ids, created_at, completed_at, duration_ms, eval,
    eval_attempt)
VALUES
('c169bcae-54fc-40b4-8964-e9fe9cbacd95', 'Reply with exactly the text: pong-ts-to-rust.', '[]', '[]',
 '{"excludeTools": ["delegate_task"]}', 'chat-loop', 'rivet', 'mcp-sidecar', 'api', 1, 'node-a',
 '{"maxWallClockMs": 300000}', NULL, 'killed', 0, 1, 'Killed by parent', NULL,
 'task:c169bcae-54fc-40b4-8964-e9fe9cbacd95', '[]', '2026-10-15T23:03:20.463347+00:00',
 '2026-10-15T23:08:20.507724+00:00', 0, NULL, 0),
('2f0e1d5c-8b44-4c11-9a3e-0d8e6f2b7a10', 'Write the release note', '[{"kind": "file", "ref": "NOTES.md"}]',
 '[{"id": "c1", "kind": "manual", "description": "note exists"}, {"id": "c2", "kind": "automated", "check": "test -f NOTES.md", "description": "file present"}]',
 '{"promptMode": "task", "excludeTools": ["delegate_task"], "role": "worker", "cached": false, "delegation": true, "toolsUsed": ["write"]}',
 'harness-session', 'rivet', 'phil', 'api', 0, NULL, '{"maxWallClockMs": 600000, "maxTurns": 20, "maxUsd": 2.5}',
 '{"inputTokens": 10, "outputTokens": 20, "totalTokens": 30, "turns": 2, "wallClockMs": 1500, "costUsd": 0.25}',
 'completed', 1, 1, NULL,
 '{"artifacts": [{"kind": "file", "ref": "NOTES.md"}], "summary": "written", "verdict": "completed", "output": "done", "usage": {"inputTokens": 10, "outputTokens": 20, "totalTokens": 30, "turns": 2, "wallClockMs": 1500}, "criteriaSelfReport": [{"id": "c1", "met": true, "evidence": "file"}, {"id": "c2", "met": true, "ref": "self-reported without evidence"}]}',
 'task:2f0e1d5c-8b44-4c11-9a3e-0d8e6f2b7a10', '["claude-code:abc"]', '2026-10-15T20:00:00+00:00',
 '2026-10-15T20:00:02+00:00', 1500,
 '{"attempts": 2, "criteriaReport": [{"id": "c1", "met": false, "evidence": "empty"}], "diverged": true, "verdict": "refuted", "verifierTaskIds": ["v1", "v2"]}',
 2),
('6a7b8c9d-0e1f-4a2b-8c3d-4e5f6a7b8c9d', 'Escalate this', '[]',
 '[{"id": "c1", "kind": "manual", "description": "human check"}]', '{}', 'chat-loop', 'rivet', NULL,
 'api', 0, NULL, '{}', NULL, 'failed', 1, 1, 'gave up',
 '{"artifacts": [], "summary": "stuck", "verdict": "failed", "error": "gave up", "usage": {"inputTokens": 1, "outputTokens": 1, "totalTokens": 2, "turns": 1, "wallClockMs": 10}}',
 'task:6a7b8c9d-0e1f-4a2b-8c3d-4e5f6a7b8c9d', '[]', '2026-10-16T08:00:00+00:00', '2026-10-16T08:00:01+00:00', 10,
 '{"attempts": 3, "criteriaReport": [], "diverged": false, "verdict": "escalated", "verifierTaskIds": ["v3"], "escalatedAt": "2026-10-16T08:05:00.000Z"}',
 3)
"#;

fn router(pool: &PgPool, harness: Option<Arc<dyn HarnessAdapter>>) -> TestResult<axum::Router> {
    let tasks = PostgresTasks::from_pool(pool.clone());
    let directory = tasks::Directory::new("local", url::Url::parse("http://127.0.0.1:9/")?)?;
    Ok(gateway::routes::tasks::router(Arc::new(TasksApi {
        store: Arc::new(tasks.clone()),
        jobs: Arc::new(tasks.clone()),
        query: Arc::new(tasks.clone()),
        watch: Arc::new(tasks),
        catalog: Arc::new(tasks::Catalog::new(&directory, false, Vec::new(), harness)),
        agents: Arc::new(NoAgents),
        approvals: None,
        node: "local".to_owned(),
        mesh: false,
        allowed_harnesses: None,
        command_timeout: gateway::routes::tasks::COMMAND_LIST_TIMEOUT,
    })))
}

async fn get(router: &axum::Router, path: &str) -> TestResult<(StatusCode, Vec<u8>)> {
    let response = router
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty())?)
        .await?;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20).await?;
    Ok((status, body.to_vec()))
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn ts_era_task_rows_list_through_the_router(pool: PgPool) -> TestResult {
    sqlx::query(TS_ROWS).execute(&pool).await?;
    let app = router(&pool, None)?;
    let (status, body) = get(&app, "/api/tasks").await?;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let listed: TasksListResponse = serde_json::from_slice(&body)?;
    assert_eq!(listed.tasks.len(), 3);
    let refuted = listed
        .tasks
        .iter()
        .find(|task| task.id == "2f0e1d5c-8b44-4c11-9a3e-0d8e6f2b7a10")
        .ok_or("refuted task")?;
    let eval = refuted.eval.as_ref().ok_or("eval")?;
    assert_eq!(eval.verdict, EvalVerdict::Refuted);
    assert_eq!(eval.verifier_task_ids, ["v1", "v2"]);
    assert_eq!(refuted.harness_session_ids, ["claude-code:abc"]);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn an_undecodable_task_column_is_named(pool: PgPool) -> TestResult {
    sqlx::query(TS_ROWS).execute(&pool).await?;
    sqlx::query("UPDATE rh_tasks SET eval = '{\"verdict\": \"maybe\"}' WHERE eval_attempt = 3")
        .execute(&pool)
        .await?;
    let (status, body) = get(&router(&pool, None)?, "/api/tasks").await?;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let error: wire::gateway::GatewayError = serde_json::from_slice(&body)?;
    assert!(error.error.contains("column \"eval\""), "{}", error.error);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn outcomes_roll_up_ts_era_rows(pool: PgPool) -> TestResult {
    sqlx::query(TS_ROWS).execute(&pool).await?;
    let (status, body) = get(&router(&pool, None)?, "/api/outcomes").await?;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let outcomes: OutcomesResponse = serde_json::from_slice(&body)?;
    assert_eq!(outcomes.totals.tasks, 2);
    assert_eq!(outcomes.totals.completed, 1);
    assert_eq!(outcomes.totals.failed, 1);
    assert_eq!(outcomes.totals.refuted, 1);
    assert_eq!(outcomes.totals.escalated, 1);
    assert_eq!(outcomes.totals.diverged, 1);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn the_catalog_lists_the_commands_harness_discovery_found(pool: PgPool) -> TestResult {
    let harness = Arc::new(testkit::FakeHarness::new(Vec::new()));
    harness.set_commands(vec![HarnessCommands {
        harness: HarnessId::ClaudeCode,
        commands: vec![Command {
            name: "review".to_owned(),
            description: "review the change".to_owned(),
        }],
    }]);
    let (status, body) = get(&router(&pool, Some(harness))?, "/api/catalog").await?;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let sheet: CatalogSheet = serde_json::from_slice(&body)?;
    let session = sheet
        .executors
        .iter()
        .find(|entry| entry.key == "harness-session")
        .ok_or("harness-session executor")?;
    let command = session.commands.first().ok_or("one command")?;
    assert_eq!(command.name, "review");
    assert_eq!(command.source, "claude-code");
    assert!(
        sheet
            .executors
            .iter()
            .filter(|entry| entry.key != "harness-session")
            .all(|entry| entry.commands.is_empty())
    );
    assert_eq!(sheet.skills, Vec::new());
    let (status, _) = get(&router(&pool, None)?, "/api/catalog").await?;
    assert_eq!(status, StatusCode::OK);
    Ok(())
}
