use std::collections::BTreeMap;
use std::path::PathBuf;

use libtest_mimic::Arguments;
use libtest_mimic::Completion;
use libtest_mimic::Failed;
use libtest_mimic::Trial;
use libtest_mimic::run;
use terminal::herdr::DEFAULT_PANE;
use terminal::herdr::HerdrEventKind;
use terminal::herdr::PINNED_VERSION;
use terminal::herdr::codec;
use terminal::herdr::paths;
use terminal::herdr::probe;
use terminal::mux::HerdrSetup;
use terminal::session::SessionKey;

fn process_env() -> config::Environment {
    config::Environment::from_process()
}

fn search_paths() -> Vec<PathBuf> {
    let path = process_env().text("PATH").unwrap_or_default().to_owned();
    let mut dirs: Vec<PathBuf> = path.split(':').map(PathBuf::from).collect();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
    }
    dirs
}

fn home_dir() -> Option<PathBuf> {
    process_env().home().map(PathBuf::from)
}

fn run_probe() -> Result<terminal::herdr::HerdrBinary, terminal::herdr::ProbeFailure> {
    let outcome =
        tokio::runtime::Runtime::new().map_err(|error| terminal::herdr::ProbeFailure::Version {
            binary: PathBuf::from("herdr"),
            found: format!("dev test runtime: {error}"),
        })?;
    outcome.block_on(probe(&search_paths(), home_dir().as_deref()))
}

async fn real_lifecycle(binary: PathBuf) -> Result<Completion, Failed> {
    let state = tempfile::tempdir().map_err(|error| Failed::from(error.to_string()))?;
    let uid = nix::unistd::Uid::current().as_raw();
    let config_home = paths::resolve_config_home(state.path(), 5974, uid);
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
    let trials = match run_probe() {
        Ok(binary) => vec![
            Trial::test("herdr pinned binary is installed", {
                let binary = binary.path.clone();
                move || match run_probe() {
                    Ok(found) if found.path == binary => Ok(()),
                    Ok(found) => Err(Failed::from(format!(
                        "the probe moved: {} then {}",
                        binary.display(),
                        found.path.display()
                    ))),
                    Err(failure) => Err(Failed::from(format!("probe flapped: {failure:?}"))),
                }
            })
            .with_ignored_flag(true),
            Trial::ignorable_test("herdr server lifecycle", move || {
                let runtime = tokio::runtime::Runtime::new()
                    .map_err(|error| Failed::from(format!("dev test runtime: {error}")))?;
                runtime.block_on(real_lifecycle(binary.path))
            })
            .with_ignored_flag(true),
        ],
        Err(failure) => vec![Trial::ignorable_test(
            "herdr pinned binary is installed",
            move || {
                Ok(Completion::Ignored {
                    reason: Some(format!(
                        "herdr {PINNED_VERSION} is not installed on this machine ({failure:?}) — the socket and RPC conformance runs need the real binary"
                    )),
                })
            },
        )
        .with_ignored_flag(true)],
    };
    run(&args, trials).exit();
}
