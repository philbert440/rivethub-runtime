use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kernel::memory::MemoryStore;
use rivethub::composition::Composition;
use terminal::herdr::namespace;
use terminal::herdr::namespace::SessionStamp;
use terminal::herdr::paths;
use terminal::session::RosterKey;
use terminal::session::SessionKey;
use tokio_util::sync::CancellationToken;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn herdr_script(dir: &Path) -> TestResult {
    let binary = dir.join("herdr");
    std::fs::write(&binary, "#!/bin/sh\necho 0.8.2\n")?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn write_config(home: &Path, work: &Path, state: &Path) -> TestResult {
    let commands = home.join("commands.json");
    std::fs::write(
        &commands,
        format!(
            r#"{{"default":"shell","cwd":"{}","commands":{{"shell":{{"label":"Shell","cmd":["/bin/true"],"room":false}}}}}}"#,
            work.display()
        ),
    )?;
    let config = format!(
        "runtime:\n  workspace: {work}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:9/v1\n    model: m\n    probe_models: false\nden:\n  host: 127.0.0.1\n  port: 5974\n  terminal:\n    enabled: true\n  commands_file: {commands}\n  files_root: {work}\n",
        work = work.display(),
        commands = commands.display(),
    );
    std::fs::write(state.join("config.yaml"), config)?;
    Ok(())
}

fn plant_foreign_socket(
    home: &Path,
    config_home: &Path,
) -> TestResult<(tempfile::TempDir, PathBuf)> {
    let directory = tempfile::tempdir_in(home)?;
    let namespace_name = config_home.file_name().ok_or("config home name")?;
    let socket = directory
        .path()
        .join(namespace_name)
        .join("herdr")
        .join("sessions")
        .join("dhostforeign1")
        .join("herdr.sock");
    std::fs::create_dir_all(socket.parent().ok_or("foreign session")?)?;
    std::fs::write(&socket, b"foreign")?;
    Ok((directory, socket))
}

fn plant_owned(
    config_home: &Path,
    namespace_id: &str,
    key: &str,
    user: &str,
) -> TestResult<PathBuf> {
    let key = SessionKey::new(key)?;
    let dir = paths::sessions_dir(config_home).join(paths::session_dir_name(&key));
    std::fs::create_dir_all(&dir)?;
    namespace::write_stamp(
        &dir,
        &SessionStamp {
            namespace: namespace_id.to_owned(),
            session_key: key,
            user: user.to_owned(),
            roster_key: RosterKey::new("shell")?,
            argv: vec!["/bin/true".to_owned()],
        },
    )?;
    std::fs::write(dir.join("herdr.sock"), [])?;
    Ok(dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_startup_adopts_only_its_namespace() -> TestResult {
    let home = tempfile::Builder::new()
        .prefix("")
        .rand_bytes(1)
        .tempdir()?;
    let work = home.path().join("work");
    let bin = home.path().join("bin");
    let state = home.path().join(".rivethub");
    std::fs::create_dir_all(&work)?;
    std::fs::create_dir_all(&bin)?;
    std::fs::create_dir_all(&state)?;
    herdr_script(&bin)?;
    write_config(home.path(), &work, &state)?;
    let env = config::Environment::from_vars([
        ("HOME", home.path().to_str().ok_or("home")?),
        ("PATH", bin.to_str().ok_or("path")?),
    ]);
    let loaded = config::Config::load(
        &env,
        &config::ConfigLocation::Current(state.join("config.yaml")),
        config::Profile::Node,
    )?;
    let config_home = paths::resolve_config_home(&state, 5974);
    assert!(config_home.starts_with(&state));
    let namespace_id = namespace::namespace_id(&config_home)?;
    let owned_dir = plant_owned(&config_home, &namespace_id, "owned-session", "owner")?;
    let foreign_user = plant_owned(&config_home, &namespace_id, "coco-session", "coco")?;
    let (_foreign, foreign_socket) = plant_foreign_socket(home.path(), &config_home)?;
    let shutdown = CancellationToken::new();
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let composition = tokio::time::timeout(
        Duration::from_secs(30),
        Composition::build(&loaded.config, &env, store, &shutdown),
    )
    .await??;
    let owner = gateway::tenancy::UserId::new("owner".to_owned())?;
    let other = gateway::tenancy::UserId::new("coco".to_owned())?;
    let service = composition
        .gateway()
        .terminal()
        .ok_or("terminal service missing")?
        .clone();
    let listed = service.list(&owner);
    assert_eq!(listed.len(), 1);
    let info = listed.first().ok_or("owned session missing")?;
    assert_eq!(info.session, "owned-session");
    assert_eq!(info.command, "shell");
    assert_eq!(info.state, wire::gateway::term::PtyState::Running);
    assert_eq!(info.mux, Some(wire::gateway::term::Mux::Herdr));
    let listed_config = service.config(&owner);
    assert!(listed_config.enabled);
    assert_eq!(listed_config.default, "shell");
    let listed_command = listed_config.commands.first().ok_or("roster command")?;
    assert_eq!(listed_command.id, "shell");
    assert_eq!(listed_command.label, "Shell");
    assert!(!listed_command.session);
    assert_eq!(service.list(&other).len(), 0);
    let forbidden = service.kill(&other, &info.id).await.expect_err("forbidden");
    assert_eq!(forbidden.to_string(), "session is owned by another user");
    assert!(owned_dir.join("herdr.sock").is_file());
    assert!(foreign_user.join("herdr.sock").is_file());
    assert_eq!(std::fs::read(&foreign_socket)?, b"foreign");
    let claim = composition
        .gateway()
        .sessions()
        .claim(&other, "owned-session")
        .expect_err("claim");
    assert_eq!(claim.to_string(), "session is owned by another user");
    let authorize = composition
        .gateway()
        .sessions()
        .authorize(&other, "owned-session")
        .expect_err("authorize");
    assert_eq!(authorize.to_string(), "session is owned by another user");
    tokio::time::timeout(Duration::from_secs(10), composition.close()).await??;
    Ok(())
}
