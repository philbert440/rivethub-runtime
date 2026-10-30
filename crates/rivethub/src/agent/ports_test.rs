use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::body::Bytes;
use axum::extract::ConnectInfo;
use axum::extract::State;
use axum::http::Request;
use axum::http::StatusCode;
use axum::http::header;
use axum::routing::get;
use axum::routing::post;
use gateway::Gateway;
use kernel::memory::MemoryStore;
use kernel::tasks::TaskAgents;
use kernel::tasks::TaskRoute;
use testkit::TaskMemory;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tower::ServiceExt;
use wire::gateway::HealthzResponse;
use wire::gateway::agents::AgentPresetCreateRequest;
use wire::gateway::agents::AgentResponse;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogAgentsResponse;
use wire::gateway::sessions::SessionMessage;
use wire::gateway::sessions::SessionMessageRole;
use wire::gateway::sessions::SessionPostReply;
use wire::gateway::sessions::SessionPostRequest;
use wire::harness::HarnessId;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;
use wire::task::TaskCreateRequest;
use wire::task::TaskExecutorKind;
use wire::task::TaskResponse;
use wire::task::TaskStatus;

use super::PresetFile;
use super::Roster;

const NODE: &str = "node-a";
const BODY_LIMIT: usize = 1 << 20;

type Turns = (mpsc::UnboundedSender<SessionPostRequest>, Arc<Semaphore>);

struct Rig {
    router: Router,
    engine: Arc<tasks::Engine<TaskMemory, TaskMemory>>,
    turns: mpsc::UnboundedReceiver<SessionPostRequest>,
    release: Arc<Semaphore>,
    stub: testkit::stub::Stub,
    root: tempfile::TempDir,
}

fn settings(root: &Path) -> gateway::Settings {
    gateway::Settings {
        hostname: "node-a-host".to_owned(),
        node: NODE.to_owned(),
        allowed_origins: Vec::new(),
        allowed_hosts: Vec::new(),
        files_root: Some(root.to_path_buf()),
        static_dir: None,
        root_redirect: None,
        state_dir: root.join("state"),
        directory_root: root.join("agents"),
        mesh_file: Some(root.join("absent-mesh.json")),
        shared_dir: root.join("shared"),
        legacy_mesh_dir: root.join("legacy"),
        allowed_harnesses: None,
        devices_enabled: false,
        probe_timeout: Duration::from_millis(200),
        mesh_cache_ttl: Duration::ZERO,
        mesh_ca: None,
    }
}

async fn healthz() -> axum::Json<HealthzResponse> {
    axum::Json(HealthzResponse {
        ok: true,
        sessions: 0,
        name: "stub".to_owned(),
        node: NODE.to_owned(),
        degraded: None,
        degraded_providers: Vec::new(),
        version: None,
        revision: None,
    })
}

async fn turn(State((posts, release)): State<Turns>, body: Bytes) -> axum::Json<SessionPostReply> {
    posts
        .send(serde_json::from_slice(&body).expect("posted turn"))
        .expect("record turn");
    release.acquire().await.expect("release").forget();
    axum::Json(SessionPostReply {
        message: SessionMessage {
            id: "m1".to_owned(),
            session_id: "s1".to_owned(),
            role: SessionMessageRole::Assistant,
            text: "done".to_owned(),
            ts: chrono::DateTime::from_timestamp_millis(1_760_000_001_000).expect("stamp"),
            usage: None,
            model: None,
            duration_ms: None,
            thinking: None,
            tools: None,
        },
    })
}

async fn rig() -> Rig {
    let root = tempfile::tempdir().expect("root");
    std::fs::create_dir_all(root.path().join("state")).expect("state dir");
    let (posts, turns) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let stub = testkit::stub::stub_server(
        Router::new()
            .route("/healthz", get(healthz))
            .route("/api/sessions/{session}/messages", post(turn))
            .with_state((posts, Arc::clone(&release))),
    )
    .await
    .expect("stub");
    let base = url::Url::parse(&format!("http://{}/", stub.addr)).expect("base");
    let roster = Arc::new(Roster {
        node: NODE.to_owned(),
        mesh: false,
        local: BTreeMap::new(),
        presets: Arc::new(PresetFile::new(root.path().join("state"))),
        nodes: Vec::new(),
    });
    let directory = tasks::Directory::new(NODE, base).expect("directory");
    let memory = TaskMemory::new();
    let engine = tasks::Engine::new(
        memory.clone(),
        memory.clone(),
        directory.clone(),
        tasks::Settings {
            node: NODE.to_owned(),
            worker: format!("{NODE}:agent"),
            files_root: Some(root.path().to_path_buf()),
            pricing: BTreeMap::new(),
            api_token: None,
        },
    )
    .expect("engine")
    .with_presets(roster.presets());
    let catalog = Arc::new(
        tasks::Catalog::new(&directory, false, Vec::new(), None).with_presets(roster.presets()),
    );
    let api = super::api(roster, catalog, None, &memory);
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let gateway = Arc::new(Gateway::new(store, settings(root.path())).with_tasks(api));
    Rig {
        router: gateway.router(false),
        engine: Arc::new(engine),
        turns,
        release,
        stub,
        root,
    }
}

async fn call(router: &Router, method: &str, target: &str, body: &str) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(target)
        .header(header::HOST, "127.0.0.1:5976")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:40000".parse::<SocketAddr>().expect("peer"),
    ));
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), BODY_LIMIT)
        .await
        .expect("body");
    (status, bytes.to_vec())
}

async fn queue(router: &Router, agent: &str) -> (StatusCode, Option<String>) {
    let body = TaskCreateRequest {
        goal: "ship it".to_owned(),
        agent_id: agent.to_owned(),
        executor: Some(TaskExecutorKind::HarnessSession),
        executor_target: None,
        requested_by: None,
        parent_task_id: None,
        chain_depth: None,
        node_affinity: None,
        spec: None,
        budget: None,
        context_refs: None,
        acceptance_criteria: None,
    };
    let body = serde_json::to_string(&body).expect("task body");
    let (status, bytes) = call(router, "POST", "/api/tasks", &body).await;
    let id = serde_json::from_slice::<TaskResponse>(&bytes)
        .ok()
        .map(|created| created.task.id);
    (status, id)
}

async fn status(engine: &tasks::Engine<TaskMemory, TaskMemory>, id: &str) -> TaskStatus {
    engine
        .load(id.parse().expect("task id"))
        .await
        .expect("load")
        .status
}

#[tokio::test]
async fn preset_edits_reach_the_next_task_without_a_restart() {
    let mut rig = rig().await;
    let create = AgentPresetCreateRequest {
        name: "Kit".to_owned(),
        harness_id: Some(HarnessId::ClaudeCode.into()),
        model: Some("m1".to_owned()),
        effort: Some("high".to_owned()),
        ..AgentPresetCreateRequest::default()
    };
    let create = serde_json::to_string(&create).expect("preset body");
    let (code, body) = call(&rig.router, "POST", "/api/agents", &create).await;
    assert_eq!(
        code,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let preset = serde_json::from_slice::<AgentResponse>(&body)
        .expect("created preset")
        .agent;

    let (code, body) = call(&rig.router, "GET", "/api/catalog/agents", "").await;
    assert_eq!(code, StatusCode::OK);
    let listed = serde_json::from_slice::<CatalogAgentsResponse>(&body).expect("catalog");
    assert!(
        listed.agents.iter().any(|agent| matches!(
            agent,
            CatalogAgent::Preset(row) if row.id == preset.id
                && row.model.as_deref() == Some("m1")
                && row.effort.as_deref() == Some("high")
        )),
        "{listed:?}"
    );

    let (code, first) = queue(&rig.router, "Kit").await;
    assert_eq!(code, StatusCode::CREATED);
    let first = first.expect("first task");
    let running = Arc::clone(&rig.engine);
    let in_flight = tokio::spawn(async move { running.run_one().await });
    let posted = rig.turns.recv().await.expect("first turn");
    assert_eq!(posted.harness, Some(HarnessId::ClaudeCode));
    assert_eq!(posted.model.as_deref(), Some("m1"));
    let directory = std::fs::canonicalize(preset.directory.as_deref().expect("directory"))
        .expect("preset directory");
    assert_eq!(posted.working_dir.as_deref(), directory.to_str());

    let target = format!("/api/agents/{}", preset.id);
    let (code, _) = call(&rig.router, "PATCH", &target, r#"{"model":"m2"}"#).await;
    assert_eq!(code, StatusCode::OK);
    rig.release.add_permits(1);
    in_flight.await.expect("join").expect("first run");
    assert_eq!(status(&rig.engine, &first).await, TaskStatus::Completed);
    assert!(rig.turns.try_recv().is_err());

    let (code, second) = queue(&rig.router, "Kit").await;
    assert_eq!(code, StatusCode::CREATED);
    rig.release.add_permits(1);
    rig.engine.run_one().await.expect("second run");
    let posted = rig.turns.recv().await.expect("second turn");
    assert_eq!(posted.model.as_deref(), Some("m2"));
    assert_eq!(
        status(&rig.engine, &second.expect("second task")).await,
        TaskStatus::Completed
    );

    let (code, _) = call(&rig.router, "DELETE", &target, "").await;
    assert_eq!(code, StatusCode::OK);
    let (code, third) = queue(&rig.router, "Kit").await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert_eq!(third, None);
    assert!(rig.turns.try_recv().is_err());
    assert!(rig.root.path().join("state").join("agents.json").is_file());
    rig.stub.close().await.expect("stub shuts down");
}

fn host(id: &str, agents: &[&str], seen: i64, status: MeshNodeStatus) -> MeshNode {
    let stamp = chrono::DateTime::from_timestamp_millis(seen).expect("stamp");
    MeshNode {
        id: id.to_owned(),
        name: id.to_owned(),
        role: None,
        agents: agents.iter().map(|agent| (*agent).to_owned()).collect(),
        host: "127.0.0.1".to_owned(),
        port: 9,
        providers: Vec::new(),
        models: Vec::new(),
        capabilities: Vec::new(),
        status,
        last_seen: stamp,
        registered_at: stamp,
        version: "0.1.0".to_owned(),
        metadata: None,
        ssh_user: None,
        install_root: None,
        platform: None,
        extra: wire::Extra::default(),
    }
}

#[tokio::test]
async fn a_task_routes_to_the_node_that_hosts_its_agent() {
    let root = tempfile::tempdir().expect("root");
    let mut local = BTreeMap::new();
    local.insert(
        "worker".to_owned(),
        super::super::LocalBinding {
            model: None,
            provider: None,
        },
    );
    let roster = Roster {
        node: NODE.to_owned(),
        mesh: true,
        local,
        presets: Arc::new(PresetFile::new(root.path().to_path_buf())),
        nodes: vec![
            host(
                "east",
                &["claude"],
                1_760_000_001_000,
                MeshNodeStatus::Online,
            ),
            host(
                "west",
                &["claude"],
                1_760_000_002_000,
                MeshNodeStatus::Online,
            ),
            host(
                "north",
                &["claude"],
                1_760_000_003_000,
                MeshNodeStatus::Offline,
            ),
        ],
    };
    let routed = |agent: &str, node: &str| {
        Some(TaskRoute {
            agent_id: agent.to_owned(),
            node: node.to_owned(),
        })
    };
    assert_eq!(
        roster.route("worker", None).await.expect("local"),
        routed("worker", NODE)
    );
    assert_eq!(
        roster.route("claude", None).await.expect("mesh"),
        routed("claude", "west")
    );
    assert_eq!(
        roster.route("claude", Some("east")).await.expect("pinned"),
        routed("claude", "east")
    );
    assert_eq!(
        roster.route("ghost", Some("east")).await.expect("unknown"),
        None
    );
}
