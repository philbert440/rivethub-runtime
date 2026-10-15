#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use std::io::Read;
use std::io::Write;
use std::net::TcpListener;

use common::gateway_with;
use gateway::Settings;
use wire::gateway::devices::MeshOverview;

fn member(
    id: &'static str,
    name: &'static str,
    url: &str,
    capabilities: &'static [&'static str],
) -> common::roster::Member {
    common::roster::Member {
        id,
        name,
        host: "127.0.0.1",
        url: Some(url.to_owned()),
        port: None,
        capabilities,
    }
}

fn app(settings: Settings) -> axum::Router {
    gateway_with(settings).router(false)
}

fn call(router: axum::Router, target: &str) -> Result<testkit::Response, String> {
    let request = testkit::Request {
        method: "GET".to_owned(),
        target: target.to_owned(),
        headers: vec![("host".to_owned(), "127.0.0.1".to_owned())],
        body: Vec::new(),
        recipe: None,
        via: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("runtime: {error}"))?;
    runtime.block_on(common::answer(router, request))
}

#[test]
fn symlink_and_sibling_prefix_stay_inside_the_static_root() {
    let scratch = tempfile::tempdir().expect("temp");
    let root = scratch.path().join("static");
    let sibling = scratch.path().join("static-evil");
    std::fs::create_dir(&root).expect("root");
    std::fs::create_dir(&sibling).expect("sibling");
    std::fs::write(root.join("ok.txt"), b"yes").expect("ok");
    std::fs::write(sibling.join("secret.txt"), b"no").expect("secret");
    std::os::unix::fs::symlink(sibling.join("secret.txt"), root.join("escape")).expect("escape");
    std::os::unix::fs::symlink(root.join("ok.txt"), root.join("linked")).expect("linked");
    std::os::unix::fs::symlink(&sibling, root.join("out")).expect("out");
    let mut settings = common::settings();
    settings.static_dir = Some(root);
    let router = app(settings);
    let ok = call(router.clone(), "/ok.txt").expect("ok");
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body, b"yes");
    let linked = call(router.clone(), "/linked").expect("linked");
    assert_eq!(linked.status, 200);
    assert_eq!(linked.body, b"yes");
    let escape = call(router.clone(), "/escape").expect("escape");
    assert_eq!(escape.status, 404);
    assert_eq!(escape.body, br#"{"error":"not found"}"#);
    let dotdot = call(router.clone(), "/foo/../ok.txt").expect("dotdot");
    assert_eq!(dotdot.status, 404);
    assert_eq!(dotdot.body, br#"{"error":"not found"}"#);
    let nested = call(router, "/out/secret.txt").expect("nested");
    assert_eq!(nested.status, 404);
    assert_eq!(nested.body, br#"{"error":"not found"}"#);
}

#[test]
fn a_live_peer_is_online_and_a_refused_peer_is_not() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0_u8; 2048];
        let _read = stream.read(&mut buf).expect("request");
        let body = br#"{"ok":true,"sessions":0,"name":"peer","node":"peer"}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).expect("head");
        stream.write_all(body).expect("body");
    });
    let scratch = tempfile::tempdir().expect("temp");
    let mesh = scratch.path().join("mesh.json");
    let document = common::roster::document(
        &[
            member("golden-node", "Golden Node", "http://127.0.0.1:1", &[]),
            member(
                "peer-live",
                "Live",
                &format!("http://127.0.0.1:{port}"),
                &[],
            ),
        ],
        1_760_000_000_000,
    )
    .expect("document");
    std::fs::write(&mesh, document).expect("mesh");
    let mut settings = common::settings();
    settings.mesh_file = Some(mesh);
    settings.node = "absent-local".to_owned();
    let response = call(app(settings), "/mesh.json").expect("mesh");
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let overview: MeshOverview = serde_json::from_slice(&response.body).expect("overview");
    let live = overview
        .nodes
        .iter()
        .find(|node| node.id == "peer-live")
        .expect("live");
    let refused = overview
        .nodes
        .iter()
        .find(|node| node.id == "golden-node")
        .expect("refused");
    assert!(live.online);
    assert!(!refused.online);
}

#[test]
fn api_mesh_returns_the_same_overview_as_mesh_json() {
    let port = common::spawn_healthz().expect("healthz");
    let scratch = tempfile::tempdir().expect("temp");
    let mesh = scratch.path().join("mesh.json");
    let document = common::roster::document(
        &[member(
            "golden-node",
            "Golden Node",
            &format!("http://127.0.0.1:{port}"),
            &[],
        )],
        1_760_000_000_000,
    )
    .expect("document");
    std::fs::write(&mesh, document).expect("mesh");
    let mut settings = common::settings();
    settings.mesh_file = Some(mesh);
    settings.node = "golden-node".to_owned();
    let router = app(settings);
    let direct = call(router.clone(), "/mesh.json").expect("mesh");
    let alias = call(router, "/api/mesh").expect("alias");
    assert_eq!(
        direct.status,
        200,
        "{}",
        String::from_utf8_lossy(&direct.body)
    );
    assert_eq!(
        alias.status,
        200,
        "{}",
        String::from_utf8_lossy(&alias.body)
    );
    assert_eq!(direct.body, alias.body);
    let overview: MeshOverview = serde_json::from_slice(&direct.body).expect("overview");
    let local = overview
        .nodes
        .iter()
        .find(|node| node.id == "golden-node")
        .expect("local");
    assert!(local.online);
    assert_eq!(local.sessions, Some(0));
    assert!(matches!(
        local.latest,
        Some(wire::gateway::devices::MeshLatest::NoActivity)
    ));
}

#[test]
fn a_dead_local_url_is_offline() {
    let scratch = tempfile::tempdir().expect("temp");
    let mesh = scratch.path().join("mesh.json");
    std::fs::write(
        &mesh,
        common::roster::document(
            &[member(
                "golden-node",
                "Golden Node",
                "http://127.0.0.1:1",
                &[],
            )],
            1_760_000_000_000,
        )
        .expect("document"),
    )
    .expect("mesh");
    let mut settings = common::settings();
    settings.mesh_file = Some(mesh);
    settings.node = "golden-node".to_owned();
    let response = call(app(settings), "/mesh.json").expect("mesh");
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let overview: MeshOverview = serde_json::from_slice(&response.body).expect("overview");
    let local = overview
        .nodes
        .iter()
        .find(|node| node.id == "golden-node")
        .expect("local");
    assert!(!local.online);
    assert_eq!(local.sessions, Some(0));
    assert!(matches!(
        local.latest,
        Some(wire::gateway::devices::MeshLatest::NoActivity)
    ));
}
