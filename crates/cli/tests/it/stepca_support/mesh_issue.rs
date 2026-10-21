use std::ffi::OsStr;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use cli::{Cli, Dispatch, Exit, Report};
use config::Environment;
use tokio_util::sync::CancellationToken;
use wire::Extra;
use wire::mesh::{MeshFile, MeshNode, MeshNodeStatus, MeshRole};

use super::support::server::{Behavior, Server};
use super::support::token;

#[tokio::test]
async fn enroll_signs_locally_and_renew_keeps_the_same_key() {
    let world = enrolled().await.expect("enroll");
    let key = std::fs::read(world.home.path().join(".rivethub/tls/holder.key.pem")).expect("key");
    assert_eq!(
        std::fs::metadata(world.home.path().join(".rivethub/tls/holder.key.pem"))
            .expect("mode")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        !world
            .home
            .path()
            .join(".rivethub/enrollment.token")
            .exists()
    );
    assert!(!world.report.stdout.contains("BEGIN "));
    assert!(!world.report.stderr.contains("BEGIN "));
    assert!(!tree_contains(&world.shared, &key).expect("scan"));
    let file = read_mesh(&world.mesh).expect("mesh");
    let node = file.nodes.get("node-a").expect("node").clone();
    assert_eq!(node.name, "node-a");
    assert_eq!(node.host, "node-a.example.test");
    assert_eq!(node.port, 5174);
    assert_eq!(node.agents, vec!["agent-a".to_owned()]);
    assert_eq!(node.role, Some(MeshRole::Agent));
    assert_eq!(node.status, MeshNodeStatus::Online);
    assert_eq!(node.metadata.expect("metadata").gateway_port, Some(5174));
    assert_eq!(step_calls(&world.step_log).expect("log"), 1);
    let report = run(
        &world.env,
        &[
            "mesh",
            "renew",
            "ops@hub.example.test",
            "--name",
            "node-a",
            "--ca-url",
            world.server.url.as_str(),
            "--root",
            world.root.to_str().expect("root"),
        ],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(report.exit, Exit::Ok, "{}", report.stderr);
    assert!(report.stdout.contains("step-ca mTLS"));
    assert!(report.stdout.contains("mesh.json was not rewritten"));
    assert_eq!(
        std::fs::read(world.home.path().join(".rivethub/tls/holder.key.pem")).expect("key"),
        key
    );
    assert_eq!(step_calls(&world.step_log).expect("log"), 1);
    let again = read_mesh(&world.mesh).expect("mesh");
    assert_eq!(again.nodes.get("node-a").expect("node").host, node.host);
    world.server.stop().await.expect("stop");
}

#[tokio::test]
async fn enroll_preserves_operator_fields_and_warns_on_host_change() {
    let world = prepared().await.expect("prepare");
    write_prior(&world.mesh).expect("prior");
    let report = run(
        &world.env,
        &[
            "mesh",
            "enroll",
            "ops@hub.example.test",
            "--name",
            "node-a",
            "--advertise",
            "new.example.test",
            "--ca-url",
            world.server.url.as_str(),
            "--root",
            world.root.to_str().expect("root"),
            "--provisioner-password-file",
            world.password.to_str().expect("password"),
            "--roster",
            world.roster.to_str().expect("roster"),
        ],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(report.exit, Exit::Ok, "{}", report.stderr);
    assert_eq!(
        report.stderr,
        "Node node-a: keeping sshUser, installRoot, platform recorded for host old.example on new host new.example.test — check mesh.json\n"
    );
    let saved = read_mesh(&world.mesh).expect("mesh");
    let node = saved.nodes.get("node-a").expect("node");
    assert_eq!(node.host, "new.example.test");
    assert_eq!(node.ssh_user.as_deref(), Some("alice"));
    assert_eq!(node.install_root.as_deref(), Some("/opt/node"));
    assert_eq!(node.platform.as_deref(), Some("linux"));
    world.server.stop().await.expect("stop");
}

#[tokio::test]
async fn roster_named_mesh_json_and_shared_state_are_refused() {
    let world = enrolled().await.expect("enroll");
    let mesh_roster = world.home.path().join("mesh.json");
    std::fs::write(&mesh_roster, "node-a:192.0.2.15:agent-a\n").expect("roster");
    let linked = world.home.path().join("linked.json");
    std::os::unix::fs::symlink(&mesh_roster, &linked).expect("link");
    for roster in [&mesh_roster, &linked] {
        let before = step_calls(&world.step_log).expect("log");
        let report = run(
            &world.env,
            &[
                "mesh",
                "enroll",
                "ops@hub.example.test",
                "--name",
                "node-a",
                "--advertise",
                "node-a.example.test",
                "--ca-url",
                world.server.url.as_str(),
                "--root",
                world.root.to_str().expect("root"),
                "--provisioner-password-file",
                world.password.to_str().expect("password"),
                "--roster",
                roster.to_str().expect("roster"),
            ],
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(report.exit, Exit::Failure);
        assert!(report.stderr.contains("mesh.json"));
        assert_eq!(step_calls(&world.step_log).expect("log"), before);
    }
    let report = run(
        &world.env,
        &[
            "mesh",
            "enroll",
            "ops@hub.example.test",
            "--name",
            "node-b",
            "--advertise",
            "node-a.example.test",
            "--state",
            "/rivet-shared/rr-cli-mesh-state",
            "--ca-url",
            world.server.url.as_str(),
            "--root",
            world.root.to_str().expect("root"),
            "--provisioner-password-file",
            world.password.to_str().expect("password"),
            "--roster",
            world.roster.to_str().expect("roster"),
        ],
        &CancellationToken::new(),
    )
    .await;
    assert!(
        report
            .stderr
            .contains("refusing to write holder keys under /rivet-shared")
    );
    assert!(!Path::new("/rivet-shared/rr-cli-mesh-state").exists());
    world.server.stop().await.expect("stop");
}

#[tokio::test]
async fn a_path_beside_the_shared_root_is_not_treated_as_shared() {
    let world = enrolled().await.expect("enroll");
    let sibling = run(
        &world.env,
        &[
            "mesh",
            "enroll",
            "ops@hub.example.test",
            "--name",
            "node-a",
            "--advertise",
            "node-a.example.test",
            "--state",
            "/rivet-shared-evil/rr-cli-mesh-state",
            "--ca-url",
            world.server.url.as_str(),
            "--root",
            world.root.to_str().expect("root"),
            "--provisioner-password-file",
            world.password.to_str().expect("password"),
            "--roster",
            world.roster.to_str().expect("roster"),
        ],
        &CancellationToken::new(),
    )
    .await;
    assert!(!sibling.stderr.contains("refusing to write holder keys"));
    match std::fs::remove_dir_all("/rivet-shared-evil") {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("cleanup: {error}"),
    }
    world.server.stop().await.expect("stop");
}

struct World {
    home: tempfile::TempDir,
    env: Environment,
    mesh: PathBuf,
    shared: PathBuf,
    root: PathBuf,
    password: PathBuf,
    roster: PathBuf,
    step_log: PathBuf,
    server: Server,
    report: Report,
}

type TestError = Box<dyn std::error::Error + Send + Sync>;

async fn prepared() -> Result<World, TestError> {
    let home = tempfile::tempdir()?;
    let bin = home.path().join("bin");
    let mesh = home.path().join("registry.json");
    let shared = home.path().join("shared");
    let step_log = home.path().join("step.log");
    let root = home.path().join("root.pem");
    let password = home.path().join("provisioner.password");
    let roster = home.path().join("roster.txt");
    std::fs::create_dir(&shared)?;
    std::fs::write(&password, "secret\n")?;
    std::fs::write(&roster, "node-a:192.0.2.15:agent-a\n")?;
    let server = Server::start(Behavior::Sign, "spiffe://rivet.mesh/node/node-a").await?;
    std::fs::write(&root, &server.root_pem)?;
    let minted = token(&["192.0.2.15", "node-a", "node-a.rivet.mesh"])?;
    install_step(&bin, &minted)?;
    let env = Environment::from_vars([
        (OsStr::new("HOME"), home.path().as_os_str()),
        (OsStr::new("PATH"), bin.as_os_str()),
        (OsStr::new("RIVETHUB_MESH_FILE"), mesh.as_os_str()),
        (OsStr::new("RIVETHUB_SHARED_DIR"), shared.as_os_str()),
        (OsStr::new("RIVETHUB_STEP_LOG"), step_log.as_os_str()),
    ]);
    Ok(World {
        home,
        env,
        mesh,
        shared,
        root,
        password,
        roster,
        step_log,
        server,
        report: Report::success(String::new()),
    })
}

async fn enrolled() -> Result<World, TestError> {
    let world = prepared().await?;
    let root = world.root.to_str().ok_or("root path is not utf-8")?;
    let password = world
        .password
        .to_str()
        .ok_or("password path is not utf-8")?;
    let roster = world.roster.to_str().ok_or("roster path is not utf-8")?;
    let report = run(
        &world.env,
        &[
            "mesh",
            "enroll",
            "ops@hub.example.test",
            "--name",
            "node-a",
            "--advertise",
            "node-a.example.test",
            "--ca-url",
            world.server.url.as_str(),
            "--root",
            root,
            "--provisioner-password-file",
            password,
            "--roster",
            roster,
        ],
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(report.exit, Exit::Ok, "{}", report.stderr);
    assert!(report.stdout.contains("POST /1.0/sign"));
    Ok(World { report, ..world })
}

fn write_prior(path: &Path) -> Result<(), TestError> {
    let stamp = DateTime::<Utc>::UNIX_EPOCH;
    let node = MeshNode {
        id: "node-a".to_owned(),
        name: "node-a".to_owned(),
        role: Some(MeshRole::Agent),
        agents: vec!["agent-a".to_owned()],
        host: "old.example".to_owned(),
        port: 5174,
        providers: Vec::new(),
        models: Vec::new(),
        capabilities: Vec::new(),
        status: MeshNodeStatus::Online,
        last_seen: stamp,
        registered_at: stamp,
        version: "0".to_owned(),
        metadata: None,
        ssh_user: Some("alice".to_owned()),
        install_root: Some("/opt/node".to_owned()),
        platform: Some("linux".to_owned()),
        extra: Extra::default(),
    };
    let mut file = MeshFile::empty(stamp);
    file.nodes.insert(node.id.clone(), node);
    std::fs::write(path, serde_json::to_vec(&file)?)?;
    Ok(())
}

fn install_step(bin: &Path, minted: &str) -> Result<(), TestError> {
    std::fs::create_dir(bin)?;
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' '{minted}' >> \"$RIVETHUB_STEP_LOG\"\nprintf '%s\\n' '{minted}'\n"
    );
    let path = bin.join("step");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(&path)?;
    std::io::Write::write_all(&mut file, script.as_bytes())?;
    Ok(())
}

fn read_mesh(path: &Path) -> Result<MeshFile, TestError> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn step_calls(path: &Path) -> Result<usize, TestError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text.lines().count()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn tree_contains(root: &Path, needle: &[u8]) -> Result<bool, TestError> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
        } else if metadata.is_file() && !needle.is_empty() {
            let bytes = std::fs::read(&path)?;
            if bytes.windows(needle.len()).any(|window| window == needle) {
                return Ok(true);
            }
        }
    }
    Ok(false)
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
            stdout_raw: Vec::new(),
        },
    }
}
