use std::path::Path;

use wire::gateway::GatewayError;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogAgentsResponse;
use wire::task::TaskCreateRequest;
use wire::task::TasksListResponse;

use super::{TestResult, model_stub, provider_base, start, stop};

struct Observed {
    refused: (reqwest::StatusCode, GatewayError),
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

fn retired() -> TestResult<TaskCreateRequest> {
    let extra: wire::Extra =
        serde_json::from_str(r#"{"executor": "chat-loop", "budget": {"maxUsd": 1.0}}"#)?;
    Ok(TaskCreateRequest {
        goal: Some("finish the ledger".to_owned()),
        agent_id: Some("main".to_owned()),
        extra,
        ..TaskCreateRequest::default()
    })
}

async fn observe(gateway: u16) -> TestResult<Observed> {
    let client = reqwest::Client::builder().no_proxy().build()?;
    let base = format!("http://127.0.0.1:{gateway}");
    let response = client
        .post(format!("{base}/api/tasks?wait=1&timeoutMs=20000"))
        .json(&retired()?)
        .send()
        .await?;
    let refused = (response.status(), response.json::<GatewayError>().await?);
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
        refused,
        listed,
        agents,
        outcomes: outcomes.status(),
    })
}

#[tokio::test]
async fn an_agent_on_a_model_provider_is_not_delegable_and_the_retired_routes_are_gone()
-> TestResult {
    let home = tempfile::tempdir()?;
    let gateway = testkit::ports::claim()?;
    let stub = model_stub().await?;
    let database = home.path().join(".rivethub").join("memory.sqlite");
    super::write_config(
        home.path(),
        &provider_base(
            home.path(),
            &format!("http://{}", stub.address),
            &gateway_config(&database, gateway.number()),
        ),
    )?;
    let started = start(
        home.path(),
        &["local", "--no-lan"],
        &["gateway listening on "],
    )
    .await?;
    let observed = observe(gateway.number()).await;
    let stopped = stop(started).await;
    let closed = stub.close().await;
    let observed = observed?;
    stopped?;
    closed?;
    let (status, refused) = observed.refused;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{refused:?}");
    assert!(refused.error.contains("can't be delegated"), "{refused:?}");
    assert_eq!(observed.listed.tasks, Vec::new());
    assert!(
        observed
            .agents
            .agents
            .iter()
            .any(|agent| matches!(agent, CatalogAgent::Node(node) if node.id == "main")),
        "{:?}",
        observed.agents
    );
    assert_eq!(observed.outcomes, reqwest::StatusCode::NOT_FOUND);
    Ok(())
}
