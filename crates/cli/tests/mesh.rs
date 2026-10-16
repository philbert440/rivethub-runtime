use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::routing::post;
use chrono::{DateTime, Utc};
use cli::{Cli, Dispatch, Exit, Report};
use config::Environment;
use serde::Deserialize;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use wire::Extra;
use wire::mesh::{MeshFile, MeshMetadata, MeshNode, MeshNodeStatus, MeshRole};

const SKIPPED: &[(&str, &str)] = &[
    (
        "ssh-hub-enroll",
        "SSH rivethub-hub enroll and the rivet-ca.sh key tarball are retired. Enrollment mints a step-ca token locally and records the node through the mesh registry. The holder key stays in the state directory.",
    ),
    (
        "ssh-hub-sync",
        "SSH rivethub-hub mesh-export is retired. Sync reads the hub gateway mesh.json and merges it with Registry::register, keeping local-only nodes.",
    ),
    (
        "ssh-hub-renew",
        "SSH rivet-ca renewal is retired. Renewal is rh cert renew over step-ca mTLS and does not rewrite mesh.json.",
    ),
    (
        "legacy-yaml-seed",
        "join --manual does not parse a YAML seed. It reads a typed mesh.json and posts /api/mesh/join. The recorded HTTP error strings are still returned.",
    ),
];

#[test]
fn skipped_mesh_scenarios_are_named_once_with_a_reason() {
    let mut names = std::collections::BTreeSet::new();
    assert_ne!(SKIPPED, []);
    for (name, reason) in SKIPPED {
        assert_ne!(*name, "");
        assert_ne!(*reason, "");
        assert!(names.insert(*name), "duplicate scenario {name}");
    }
    assert_eq!(names.len(), SKIPPED.len());
}

#[tokio::test]
async fn mesh_help_usages_and_refusals_match_the_inventory() {
    let home = tempfile::tempdir().expect("home");
    let env = environment(&home, &[]).expect("env");
    let bare = run(&env, &["mesh"], &CancellationToken::new()).await;
    assert_eq!(bare.exit, Exit::Ok);
    assert_eq!(bare.stderr, "");
    assert!(bare.stdout.contains("enroll"));
    let unknown = run(&env, &["mesh", "nosuch"], &CancellationToken::new()).await;
    assert_eq!(unknown.exit, Exit::Ok);
    assert_eq!(unknown.stderr, "");
    assert!(unknown.stdout.contains("enroll"));
    assert_eq!(
        run(&env, &["mesh", "enroll"], &CancellationToken::new())
            .await
            .stderr,
        "Usage: rh mesh enroll <user@host> --name <node> [--advertise <host|ip>] [--hub-cmd rivethub-hub]\n"
    );
    assert_eq!(
        run(&env, &["mesh", "sync"], &CancellationToken::new())
            .await
            .stderr,
        "Usage: rh mesh sync <user@host> [--hub-cmd rivethub-hub]\n"
    );
    assert_eq!(
        run(&env, &["mesh", "renew"], &CancellationToken::new())
            .await
            .stderr,
        "Usage: rh mesh renew <user@host> --name <node> [--hub-cmd rivethub-hub]\n"
    );
    assert_eq!(
        run(&env, &["mesh", "forget"], &CancellationToken::new())
            .await
            .stderr,
        "Usage: rh mesh forget <node> [--force]\n"
    );
    assert_eq!(
        run(
            &env,
            &["mesh", "join", "--manual"],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "Usage: rh mesh join --manual <host> [--port <port>]\n"
    );
    let pointer = run(&env, &["mesh", "join"], &CancellationToken::new()).await;
    assert_eq!(pointer.exit, Exit::Ok);
    assert!(
        pointer
            .stdout
            .contains("rh mesh enroll <user@host> --name <node>")
    );
    let retired = run(
        &env,
        &["mesh", "join", "hub.example"],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(retired.exit, Exit::Failure);
    assert_eq!(
        retired.stderr,
        "mesh join hub.example is no longer a join (exiting non-zero so scripts do not assume success). Use `rh mesh enroll <user@host> --name <node>`, or `rh mesh join --manual <host>` for the legacy YAML flow.\n"
    );
}

#[tokio::test]
async fn mesh_name_and_target_refusals_match_the_inventory() {
    let home = tempfile::tempdir().expect("home");
    let env = environment(&home, &[]).expect("env");
    assert_eq!(
        run(
            &env,
            &["mesh", "enroll", "ops@hub.example", "--name", "NO"],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "  ❌ invalid node name \"NO\" — must match [a-z0-9]([a-z0-9-]*[a-z0-9])? and be at most 63 characters\n"
    );
    assert_eq!(
        run(
            &env,
            &["mesh", "enroll", "ops@hub.example"],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "  ❌ --name <node> is required\n"
    );
    assert_eq!(
        run(
            &env,
            &["mesh", "enroll", "not a host", "--name", "node-a"],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "  ❌ refusing unsafe user@host\n"
    );
    assert_eq!(
        run(
            &env,
            &["mesh", "enroll", "ops@", "--name", "node-a"],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "  ❌ expected user@host, got \"ops@\"\n"
    );
    assert_eq!(
        run(
            &env,
            &[
                "mesh",
                "enroll",
                "ops@hub.example",
                "--name",
                "node-a",
                "--hub-cmd",
                "bad cmd"
            ],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "  ❌ refusing unsafe --hub-cmd\n"
    );
    assert_eq!(
        run(
            &env,
            &[
                "mesh",
                "enroll",
                "ops@hub.example",
                "--name",
                "node-a",
                "--advertise",
                "bad host"
            ],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "  ❌ refusing unsafe --advertise\n"
    );
}

#[tokio::test]
async fn mesh_limit_and_step_ca_refusals_match_the_inventory() {
    let home = tempfile::tempdir().expect("home");
    let env = environment(&home, &[]).expect("env");
    assert_eq!(
        run(
            &env,
            &["mesh", "--ssh-user", "bad user", "list"],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "❌ Refusing unsafe --ssh-user \"bad user\"\n"
    );
    assert_eq!(
        run(
            &env,
            &["mesh", "join", "--manual", "127.0.0.1", "--port", "0"],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "  ❌ port must be between 1 and 65535\n"
    );
    assert_eq!(
        run(
            &env,
            &["mesh", "ping", "--timeout", "0"],
            &CancellationToken::new()
        )
        .await
        .stderr,
        "  ❌ ping timeout must be a positive number of milliseconds\n"
    );
    let missing = run(
        &env,
        &[
            "mesh",
            "enroll",
            "ops@hub.example",
            "--name",
            "node-a",
            "--advertise",
            "node-a.example",
        ],
        &CancellationToken::new(),
    )
    .await;
    assert!(missing.stderr.contains("step-ca enrollment needs"));
    assert!(missing.stderr.contains("rivet-ca.sh is retired"));
    assert!(!home.path().join("mesh.json").exists());
}

#[tokio::test]
async fn forget_guards_the_local_node_and_does_not_create_a_missing_file() {
    let home = tempfile::tempdir().expect("home");
    let mesh = home.path().join("mesh.json");
    let env = environment(
        &home,
        &[
            ("RIVETHUB_MESH_FILE", mesh.as_os_str()),
            ("RIVETHUB_NODE_NAME", OsStr::new("local")),
        ],
    )
    .expect("env");
    let missing = run(
        &env,
        &["mesh", "forget", "local"],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(missing.stderr, "  ❌ no mesh file\n");
    assert!(!mesh.exists());
    write_mesh(
        &mesh,
        vec![
            sample("local", "local.example", 5174),
            sample("peer", "peer.example", 9),
        ],
    )
    .expect("mesh");
    let refused = run(
        &env,
        &["mesh", "forget", "local"],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(
        refused.stderr,
        "  ❌ refusing to forget local node \"local\" without --force\n"
    );
    assert!(read_mesh(&mesh).expect("mesh").nodes.contains_key("local"));
    let removed = run(
        &env,
        &["mesh", "forget", "local", "--force"],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(removed.exit, Exit::Ok, "{}", removed.stderr);
    assert!(!read_mesh(&mesh).expect("mesh").nodes.contains_key("local"));
    assert!(read_mesh(&mesh).expect("mesh").nodes.contains_key("peer"));
    let unknown = run(
        &env,
        &["mesh", "forget", "missing"],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(
        unknown.stderr,
        "  ❌ mesh node \"missing\" is not in the registry\n"
    );
}

#[tokio::test]
async fn list_and_status_report_the_registry_and_leave_bad_files_unchanged() {
    let home = tempfile::tempdir().expect("home");
    let mesh = home.path().join("mesh.json");
    let env = environment(&home, &[("RIVETHUB_MESH_FILE", mesh.as_os_str())]).expect("env");
    let absent = run(&env, &["mesh", "list"], &CancellationToken::new()).await;
    assert_eq!(absent.stderr, "  ❌ no mesh file\n");
    assert!(!mesh.exists());
    write_mesh(&mesh, vec![sample("node-a", "node-a.example", 5174)]).expect("mesh");
    let listed = run(&env, &["mesh", "list"], &CancellationToken::new()).await;
    assert_eq!(listed.stdout, "node-a node-a node-a.example 5174 online\n");
    let status = run(&env, &["mesh", "status"], &CancellationToken::new()).await;
    assert!(status.stdout.contains(&format!("mesh {}", mesh.display())));
    assert!(status.stdout.contains("updated "));
    let json = run(&env, &["mesh", "list", "--json"], &CancellationToken::new()).await;
    let parsed: MeshFile = serde_json::from_str(&json.stdout).expect("json");
    assert_eq!(parsed, read_mesh(&mesh).expect("mesh"));
    let bytes = b"{";
    std::fs::write(&mesh, bytes).expect("bad");
    let invalid = run(&env, &["mesh", "list"], &CancellationToken::new()).await;
    assert_eq!(invalid.exit, Exit::Failure);
    assert_eq!(std::fs::read(&mesh).expect("bytes"), bytes);
    let flat = br#"{"nodes":[]}"#;
    std::fs::write(&mesh, flat).expect("flat");
    let rejected = run(&env, &["mesh", "list"], &CancellationToken::new()).await;
    assert_eq!(rejected.exit, Exit::Failure);
    assert!(rejected.stderr.contains("flat nodes array"));
    assert_eq!(std::fs::read(&mesh).expect("bytes"), flat);
}

#[tokio::test]
async fn sync_merges_a_typed_registry_and_join_posts_the_local_node() {
    let home = tempfile::tempdir().expect("home");
    let mesh = home.path().join("mesh.json");
    let remote = sample("peer", "peer.example", 9);
    let seed = sample("seed-a", "seed.example", 9);
    let app = Router::new()
        .route("/mesh.json", get(serve_mesh))
        .route("/api/mesh/join", post(accept_join))
        .route("/health", get(health))
        .with_state(AppState::Mesh(file_of(vec![remote.clone(), seed.clone()])));
    let server = testkit::stub_server(app).await.expect("server");
    write_mesh(
        &mesh,
        vec![
            sample("keep", "keep.example", 1),
            hinted("127.0.0.1", server.addr.port()),
        ],
    )
    .expect("mesh");
    let env = environment(&home, &[("RIVETHUB_MESH_FILE", mesh.as_os_str())]).expect("env");
    let port = server.addr.port().to_string();
    let synced = run(
        &env,
        &["mesh", "sync", "ops@127.0.0.1"],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(synced.exit, Exit::Ok, "{}", synced.stderr);
    assert!(synced.stdout.contains("mesh-export is retired"));
    let saved = read_mesh(&mesh).expect("mesh");
    assert_eq!(saved.nodes.get("peer").expect("peer").host, "peer.example");
    assert_eq!(saved.nodes.get("peer").expect("peer").port, 9);
    assert!(saved.nodes.contains_key("keep"));
    std::fs::write(
        &mesh,
        serde_json::to_vec(&MeshFile::empty(DateTime::<Utc>::UNIX_EPOCH)).expect("empty"),
    )
    .expect("reset");
    let joined = run(
        &env,
        &[
            "mesh",
            "join",
            "--manual",
            "127.0.0.1",
            "--port",
            &port,
            "--name",
            "node-b",
            "--advertise",
            "node-b.example.test",
        ],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(joined.exit, Exit::Ok, "{}", joined.stderr);
    assert!(joined.stdout.contains("step-ca was not run"));
    let saved = read_mesh(&mesh).expect("mesh");
    assert!(saved.nodes.contains_key("seed-a"));
    let local = saved.nodes.get("node-b").expect("local");
    assert_eq!(local.host, "node-b.example.test");
    assert_eq!(local.port, 5174);
    server.close().await.expect("close");
}

#[tokio::test]
async fn join_reports_seed_http_status_and_connection_errors() {
    let home = tempfile::tempdir().expect("home");
    let mesh = home.path().join("mesh.json");
    let env = environment(&home, &[("RIVETHUB_MESH_FILE", mesh.as_os_str())]).expect("env");
    let app = Router::new()
        .route("/mesh.json", get(serve_mesh))
        .route("/api/mesh/join", post(reject_join))
        .with_state(AppState::Status(StatusCode::BAD_GATEWAY));
    let server = testkit::stub_server(app).await.expect("server");
    let port = server.addr.port().to_string();
    let status = run(
        &env,
        &[
            "mesh",
            "join",
            "--manual",
            "127.0.0.1",
            "--port",
            &port,
            "--name",
            "node-b",
            "--advertise",
            "node-b.example.test",
        ],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(status.stderr, "❌ Seed node responded with HTTP 502\n");
    server.close().await.expect("close");
    let refused = run(
        &env,
        &[
            "mesh",
            "join",
            "--manual",
            "127.0.0.1",
            "--port",
            "1",
            "--name",
            "node-b",
            "--advertise",
            "node-b.example.test",
        ],
        &CancellationToken::new(),
    )
    .await;
    assert!(refused.stderr.starts_with("❌ Cannot reach seed node: "));
    let app = Router::new()
        .route("/mesh.json", get(serve_mesh))
        .route("/api/mesh/join", post(reject_join))
        .with_state(AppState::JoinFail);
    let server = testkit::stub_server(app).await.expect("server");
    let port = server.addr.port().to_string();
    let failed = run(
        &env,
        &[
            "mesh",
            "join",
            "--manual",
            "127.0.0.1",
            "--port",
            &port,
            "--name",
            "node-b",
            "--advertise",
            "node-b.example.test",
        ],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(failed.stderr, "❌ Join failed: HTTP 500 — nope\n");
    server.close().await.expect("close");
}

#[tokio::test]
async fn ping_sorts_results_and_returns_typed_json_when_a_node_fails() {
    let home = tempfile::tempdir().expect("home");
    let mesh = home.path().join("mesh.json");
    let app = Router::new()
        .route("/health", get(health))
        .with_state(AppState::Health);
    let server = testkit::stub_server(app).await.expect("server");
    let mut alive = sample("aaaa", "127.0.0.1", server.addr.port());
    alive.metadata = Some(MeshMetadata {
        gateway_url: Some(
            format!("http://127.0.0.1:{}/health", server.addr.port())
                .parse()
                .expect("url"),
        ),
        gateway_port: Some(server.addr.port()),
        extra: Extra::default(),
    });
    write_mesh(&mesh, vec![sample("zzzz", "127.0.0.1", 1), alive]).expect("mesh");
    let env = environment(&home, &[("RIVETHUB_MESH_FILE", mesh.as_os_str())]).expect("env");
    let report = run(&env, &["mesh", "ping", "--json"], &CancellationToken::new()).await;
    assert_eq!(report.exit, Exit::Failure);
    let parsed: PingOut = serde_json::from_str(&report.stdout).expect("ping");
    assert_eq!(parsed.nodes.len(), 2);
    assert_eq!(parsed.nodes.first().expect("first").id, "aaaa");
    assert!(parsed.nodes.first().expect("first").ok);
    assert_eq!(parsed.nodes.get(1).expect("second").id, "zzzz");
    assert!(!parsed.nodes.get(1).expect("second").ok);
    server.close().await.expect("close");
}

#[tokio::test]
async fn ping_timeout_is_an_event_not_a_sleep() {
    let home = tempfile::tempdir().expect("home");
    let mesh = home.path().join("mesh.json");
    let stall = CancellationToken::new();
    let app = Router::new()
        .route("/health", get(stall_health))
        .with_state(AppState::Stall(stall.clone()));
    let server = testkit::stub_server(app).await.expect("server");
    let mut node = sample("aaaa", "127.0.0.1", server.addr.port());
    node.metadata = Some(MeshMetadata {
        gateway_url: Some(
            format!("http://127.0.0.1:{}/health", server.addr.port())
                .parse()
                .expect("url"),
        ),
        gateway_port: Some(server.addr.port()),
        extra: Extra::default(),
    });
    write_mesh(&mesh, vec![node]).expect("mesh");
    let env = environment(&home, &[("RIVETHUB_MESH_FILE", mesh.as_os_str())]).expect("env");
    let report = tokio::time::timeout(
        Duration::from_secs(10),
        run(
            &env,
            &["mesh", "ping", "--timeout", "200"],
            &CancellationToken::new(),
        ),
    )
    .await
    .expect("client timeout");
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stdout.contains("aaaa unreachable:"));
    stall.cancel();
    server.close().await.expect("close");
}

#[tokio::test]
async fn a_cancelled_mesh_command_fails_before_it_starts() {
    let home = tempfile::tempdir().expect("home");
    let env = environment(&home, &[]).expect("env");
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let report = run(&env, &["mesh", "list"], &shutdown).await;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("cancelled"));
    assert!(!report.stderr.contains("❌"));
}

#[derive(Clone)]
enum AppState {
    Mesh(MeshFile),
    Status(StatusCode),
    JoinFail,
    Health,
    Stall(CancellationToken),
}

async fn serve_mesh(State(state): State<AppState>) -> Response {
    match state {
        AppState::Status(code) => code.into_response(),
        AppState::Mesh(file) => Json(file).into_response(),
        AppState::JoinFail => Json(MeshFile::empty(DateTime::<Utc>::UNIX_EPOCH)).into_response(),
        AppState::Health | AppState::Stall(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn accept_join(Json(node): Json<MeshNode>) -> Json<Joined> {
    let mut nodes = BTreeMap::new();
    nodes.insert(node.id.clone(), node);
    Json(Joined {
        status: JoinWord::Joined,
        nodes,
    })
}

async fn reject_join() -> (StatusCode, &'static str) {
    (StatusCode::INTERNAL_SERVER_ERROR, "nope")
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn stall_health(State(state): State<AppState>) -> StatusCode {
    if let AppState::Stall(token) = state {
        token.cancelled().await;
    }
    StatusCode::OK
}

#[derive(Serialize)]
struct Joined {
    status: JoinWord,
    nodes: BTreeMap<String, MeshNode>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum JoinWord {
    Joined,
}

#[derive(Deserialize)]
struct PingOut {
    nodes: Vec<PingItem>,
}

#[derive(Deserialize)]
struct PingItem {
    id: String,
    ok: bool,
}

fn environment(
    home: &tempfile::TempDir,
    extra: &[(&str, &OsStr)],
) -> Result<Environment, std::io::Error> {
    let shared = home.path().join("shared");
    std::fs::create_dir_all(&shared)?;
    let mut pairs = vec![
        ("HOME".to_owned(), home.path().as_os_str().to_owned()),
        (
            "RIVETHUB_SHARED_DIR".to_owned(),
            shared.as_os_str().to_owned(),
        ),
    ];
    for (key, value) in extra {
        pairs.push(((*key).to_owned(), (*value).to_owned()));
    }
    Ok(Environment::from_vars(pairs.iter().map(|(key, value)| {
        (OsStr::new(key.as_str()), value.as_os_str())
    })))
}

async fn run(env: &Environment, args: &[&str], shutdown: &CancellationToken) -> Report {
    let parsed = Cli::parse_args(
        std::iter::once("rh")
            .chain(args.iter().copied())
            .map(Into::into),
    );
    let cli = match parsed {
        Ok(cli) => cli,
        Err(report) => return report,
    };
    match cli::dispatch(cli, env, shutdown).await {
        Dispatch::Complete(report) => report,
        Dispatch::Gateway { .. }
        | Dispatch::Worker { .. }
        | Dispatch::Mcp { .. }
        | Dispatch::Agent { .. }
        | Dispatch::Local(_) => Report {
            stdout: String::new(),
            stderr: "unexpected service launch\n".to_owned(),
            exit: Exit::Failure,
        },
    }
}

fn sample(id: &str, host: &str, port: u16) -> MeshNode {
    let stamp = DateTime::<Utc>::UNIX_EPOCH;
    MeshNode {
        id: id.to_owned(),
        name: id.to_owned(),
        role: Some(MeshRole::Agent),
        agents: Vec::new(),
        host: host.to_owned(),
        port,
        providers: Vec::new(),
        models: Vec::new(),
        capabilities: Vec::new(),
        status: MeshNodeStatus::Online,
        last_seen: stamp,
        registered_at: stamp,
        version: "0".to_owned(),
        metadata: None,
        ssh_user: None,
        install_root: None,
        platform: None,
        extra: Extra::default(),
    }
}

fn hinted(host: &str, port: u16) -> MeshNode {
    sample("hint", host, port)
}

fn file_of(nodes: Vec<MeshNode>) -> MeshFile {
    let mut file = MeshFile::empty(DateTime::<Utc>::UNIX_EPOCH);
    for node in nodes {
        file.nodes.insert(node.id.clone(), node);
    }
    file
}

fn write_mesh(
    path: &Path,
    nodes: Vec<MeshNode>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec(&file_of(nodes))?)?;
    Ok(())
}

fn read_mesh(path: &Path) -> Result<MeshFile, Box<dyn std::error::Error + Send + Sync>> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

type Response = axum::response::Response;
