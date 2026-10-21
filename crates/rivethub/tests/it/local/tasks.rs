use std::path::Path;

use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogAgentsResponse;
use wire::task::TaskCreateRequest;
use wire::task::TaskResponse;
use wire::task::TaskStatus;
use wire::task::TasksListResponse;

use super::{DELEGATE_TEXT, TestResult, free_port, model_stub, provider_base, start, stop};

struct Observed {
    created: TaskResponse,
    listed: TasksListResponse,
    agents: CatalogAgentsResponse,
    outcomes: reqwest::StatusCode,
}

fn gateway_config(database: &Path, gateway: u16) -> String {
    format!(
        "mesh:\n  node_name: standalone-node\nmemory:\n  backend: sqlite\n  sqlite:\n    path: {}\ngateway:\n  host: 127.0.0.1\n  port: {gateway}\n",
        database.display()
    )
}

fn request() -> TaskCreateRequest {
    TaskCreateRequest {
        goal: "finish the ledger".to_owned(),
        agent_id: "main".to_owned(),
        executor: None,
        executor_target: None,
        requested_by: None,
        parent_task_id: None,
        chain_depth: None,
        node_affinity: None,
        spec: None,
        budget: None,
        context_refs: None,
        acceptance_criteria: None,
    }
}

async fn observe(gateway: u16) -> TestResult<Observed> {
    let client = reqwest::Client::builder().no_proxy().build()?;
    let base = format!("http://127.0.0.1:{gateway}");
    let created = client
        .post(format!("{base}/api/tasks?wait=1&timeoutMs=20000"))
        .json(&request())
        .send()
        .await?
        .error_for_status()?
        .json::<TaskResponse>()
        .await?;
    let listed = client
        .get(format!("{base}/api/tasks"))
        .send()
        .await?
        .error_for_status()?
        .json::<TasksListResponse>()
        .await?;
    let agents = client
        .get(format!("{base}/api/catalog/agents"))
        .send()
        .await?
        .error_for_status()?
        .json::<CatalogAgentsResponse>()
        .await?;
    let outcomes = client.get(format!("{base}/api/outcomes")).send().await?;
    Ok(Observed {
        created,
        listed,
        agents,
        outcomes: outcomes.status(),
    })
}

#[tokio::test]
async fn task_routes_run_tasks_on_the_sqlite_engine() -> TestResult {
    let home = tempfile::tempdir()?;
    let gateway = free_port()?;
    let stub = model_stub().await?;
    let database = home.path().join(".rivethub").join("memory.sqlite");
    super::write_config(
        home.path(),
        &provider_base(
            home.path(),
            &format!("http://{}", stub.address),
            &gateway_config(&database, gateway),
        ),
    )?;
    let started = start(
        home.path(),
        &["local", "--no-lan"],
        &["gateway listening on "],
    )
    .await?;
    let observed = observe(gateway).await;
    let stopped = stop(started).await;
    let closed = stub.close().await;
    let observed = observed?;
    stopped?;
    closed?;
    let task = observed.created.task;
    assert_eq!(task.status, TaskStatus::Completed, "{task:?}");
    assert_eq!(task.claimed_by.as_deref(), Some("standalone-node:agent"));
    let result = task.result.ok_or("task result")?;
    assert_eq!(result.summary, DELEGATE_TEXT);
    let listed: Vec<&str> = observed
        .listed
        .tasks
        .iter()
        .map(|listed| listed.id.as_str())
        .collect();
    assert_eq!(listed, vec![task.id.as_str()]);
    assert!(
        observed
            .agents
            .agents
            .iter()
            .any(|agent| matches!(agent, CatalogAgent::Node(node) if node.id == "main")),
        "{:?}",
        observed.agents
    );
    assert_eq!(observed.outcomes, reqwest::StatusCode::OK);
    Ok(())
}
