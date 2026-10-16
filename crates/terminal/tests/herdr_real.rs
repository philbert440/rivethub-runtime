use std::collections::BTreeMap;
use std::path::PathBuf;

use libtest_mimic::Arguments;
use libtest_mimic::Completion;
use libtest_mimic::Failed;
use libtest_mimic::Trial;
use libtest_mimic::run;
use terminal::herdr::DEFAULT_PANE;
use terminal::herdr::HerdrEventKind;
use terminal::herdr::codec;
use terminal::herdr::paths;
use terminal::herdr::probe;
use terminal::mux::HerdrSetup;
use terminal::session::SessionKey;

fn process_env() -> config::Environment {
    config::Environment::from_process()
}

fn search_paths() -> Vec<PathBuf> {
    let env = process_env();
    let mut dirs = Vec::new();
    if let Some(path) = env.text("PATH") {
        dirs.extend(std::env::split_paths(path));
    }
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
    }
    dirs
}

fn home_dir() -> Option<PathBuf> {
    process_env().home().map(PathBuf::from)
}

fn run_probe() -> Result<terminal::herdr::HerdrBinary, Failed> {
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| Failed::from(error.to_string()))?;
    runtime
        .block_on(probe(&search_paths(), home_dir().as_deref()))
        .map_err(|error| Failed::from(error.to_string()))
}

fn manager_for(binary: PathBuf, state: &std::path::Path, port: u16) -> terminal::TerminalManager {
    let config_home = paths::resolve_config_home(state, port);
    let mut path = std::ffi::OsString::from("/usr/bin:/bin");
    if let Some(dir) = binary.parent() {
        path.push(":");
        path.push(dir);
    }
    let setup = HerdrSetup {
        binary,
        config_home,
    };
    let settings = terminal::TerminalSettings::from_config(
        &config::Config::default(),
        &config::Environment::from_vars([("HOME", "/tmp")]).with_override("PATH", path),
        "herdr-kill".to_owned(),
    );
    terminal::TerminalManager::new(settings, terminal::MuxPlan::Herdr(setup))
}

async fn kill_by_returned_id(binary: PathBuf) -> Result<Completion, Failed> {
    let state = tempfile::tempdir().map_err(|error| Failed::from(error.to_string()))?;
    let work = state.path().join("work");
    std::fs::create_dir_all(&work).map_err(|error| Failed::from(error.to_string()))?;
    let manager = std::sync::Arc::new(manager_for(binary, state.path(), 5975));
    let tasks = tokio_util::task::TaskTracker::new();
    manager.run_tracked(tokio_util::sync::CancellationToken::new(), &tasks);
    manager
        .adopt_namespace(state.path(), "owner")
        .await
        .map_err(|error| Failed::from(format!("adopt: {error}")))?;
    let session: wire::session_id::SessionId = "claude-code:00000000-0000-4000-8000-000000000410"
        .parse()
        .map_err(|error: wire::session_id::SessionIdError| Failed::from(error.to_string()))?;
    let id = kernel::terminal::TerminalMux::spawn(
        manager.as_ref(),
        &session,
        vec!["/bin/cat".to_owned()],
        &work,
        &BTreeMap::new(),
        kernel::terminal::TermSize { cols: 80, rows: 24 },
    )
    .await
    .map_err(|error| Failed::from(format!("spawn: {error}")))?;
    let listed: Vec<String> = manager
        .list()
        .into_iter()
        .map(|record| format!("{} {:?}", record.id, record.backing))
        .collect();
    if !manager.list().iter().any(|record| record.id == id) {
        return Err(Failed::from(format!(
            "spawn returned {id} but the roster is {listed:?}"
        )));
    }
    kernel::terminal::TerminalMux::kill(manager.as_ref(), &id)
        .await
        .map_err(|error| Failed::from(format!("kill {id} roster {listed:?}: {error}")))?;
    if !manager.list().iter().any(|record| record.id == id) {
        return Err(Failed::from(format!(
            "kill dropped {id}; roster is now {:?}",
            manager
                .list()
                .into_iter()
                .map(|record| record.id.to_string())
                .collect::<Vec<_>>()
        )));
    }
    let code = kernel::terminal::TerminalMux::wait(manager.as_ref(), &id)
        .await
        .map_err(|error| Failed::from(format!("wait after kill of {id}: {error}")))?;
    if code != 0 {
        return Err(Failed::from(format!("kill exit code {code}")));
    }
    manager
        .kill(&id)
        .await
        .map_err(|error| Failed::from(format!("second kill of exited {id}: {error}")))?;
    let unknown = kernel::terminal::TerminalId::new("pty-00000000");
    let missing = manager
        .kill(&unknown)
        .await
        .err()
        .ok_or_else(|| Failed::from("an unknown id must stay a typed error"))?;
    if !matches!(missing, terminal::Error::UnknownTerminal { .. }) {
        return Err(Failed::from(format!(
            "unexpected missing-id error: {missing}"
        )));
    }
    Ok(Completion::Completed)
}

async fn real_lifecycle(binary: PathBuf) -> Result<Completion, Failed> {
    let state = tempfile::tempdir().map_err(|error| Failed::from(error.to_string()))?;
    let config_home = paths::resolve_config_home(state.path(), 5974);
    let key = SessionKey::new("claude-code:00000000-0000-4000-8000-000000000400")
        .map_err(|error| Failed::from(error.to_string()))?;
    let name = paths::session_dir_name(&key);
    let socket = paths::session_socket(&config_home, &name)
        .map_err(|error| Failed::from(error.to_string()))?;
    let cwd = state.path().join("work");
    std::fs::create_dir_all(&cwd).map_err(|error| Failed::from(error.to_string()))?;
    let setup = HerdrSetup {
        binary: binary.clone(),
        config_home: config_home.clone(),
    };
    let session = terminal::herdr::HerdrSession::ensure(
        &setup,
        &key,
        &["true".to_owned()],
        &cwd,
        &BTreeMap::new(),
        "acme",
    )
    .await
    .map_err(|error| Failed::from(format!("the recorded setup sequence failed: {error}")))?;
    if session.client.socket() != socket {
        return Err(Failed::from(format!(
            "the client landed on {} instead of the derived {}",
            session.client.socket().display(),
            socket.display()
        )));
    }
    session
        .client
        .pane_get(codec::PaneGetParams {
            pane_id: DEFAULT_PANE.to_owned(),
        })
        .await
        .map_err(|error| Failed::from(format!("pane.get: {error}")))?;
    session
        .client
        .events_subscribe(codec::EventsSubscribeParams {
            subscriptions: vec![
                codec::SubscriptionParams {
                    event: HerdrEventKind::PaneAgentStatusChanged,
                    pane_id: DEFAULT_PANE.to_owned(),
                },
                codec::SubscriptionParams {
                    event: HerdrEventKind::PaneAgentDetected,
                    pane_id: DEFAULT_PANE.to_owned(),
                },
            ],
        })
        .await
        .map_err(|error| Failed::from(format!("events.subscribe: {error}")))?;
    session
        .stop()
        .await
        .map_err(|error| Failed::from(format!("stop: {error}")))?;
    if socket.exists() {
        return Err(Failed::from(
            "the server stop reported success but the session dir is still there",
        ));
    }
    Ok(Completion::Completed)
}

fn main() {
    let args = Arguments::from_args();
    let trials = vec![
        Trial::test("herdr pinned binary is installed", || {
            run_probe().map(|_| ())
        })
        .with_ignored_flag(true),
        Trial::ignorable_test("herdr server lifecycle", || {
            let binary = run_probe()?;
            let runtime =
                tokio::runtime::Runtime::new().map_err(|error| Failed::from(error.to_string()))?;
            runtime.block_on(real_lifecycle(binary.path))
        })
        .with_ignored_flag(true),
        Trial::ignorable_test("herdr spawn can be killed by the returned id", || {
            let binary = run_probe()?;
            let runtime =
                tokio::runtime::Runtime::new().map_err(|error| Failed::from(error.to_string()))?;
            runtime.block_on(kill_by_returned_id(binary.path))
        })
        .with_ignored_flag(true),
    ];
    run(&args, trials).exit();
}
