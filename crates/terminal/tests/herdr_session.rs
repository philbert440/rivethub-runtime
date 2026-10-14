#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use terminal::Error;
use terminal::herdr::AgentKind;
use terminal::herdr::HerdrCli;
use terminal::herdr::HerdrEventKind;
use terminal::herdr::PINNED_VERSION;
use terminal::herdr::classify;
use terminal::herdr::codec;
use terminal::herdr::codec::RequestParams;
use terminal::herdr::codec::RpcMethod;
use terminal::herdr::paths;
use terminal::herdr::probe;
use terminal::mux;
use terminal::session::SessionKey;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixListener;

type Failed = Box<dyn std::error::Error + Send + Sync>;

fn write_herdr_script(dir: &Path, version: &str) -> Result<std::path::PathBuf, Failed> {
    let binary = dir.join("herdr");
    std::fs::write(&binary, format!("#!/bin/sh\necho {version}\n"))?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    Ok(binary)
}

#[test]
fn herdr_cli_argv_matches_the_recorded_shapes() {
    let cli = HerdrCli {
        binary: std::path::PathBuf::from("/usr/local/bin/herdr"),
        session: "d0123456789ab".to_owned(),
    };
    assert_eq!(cli.server(), ["--session", "d0123456789ab", "server"]);
    assert_eq!(
        cli.server_stop(),
        ["--session", "d0123456789ab", "server", "stop"]
    );
    assert_eq!(cli.attach(), ["--session", "d0123456789ab"]);
    assert_eq!(
        cli.snapshot(),
        ["--session", "d0123456789ab", "session", "snapshot"]
    );
    assert_eq!(
        cli.agent_list(),
        ["--session", "d0123456789ab", "agent", "list"]
    );
    assert_eq!(
        cli.agent_read("d0123456789ab", 200),
        [
            "--session",
            "d0123456789ab",
            "agent",
            "read",
            "d0123456789ab",
            "--lines",
            "200"
        ]
    );
    assert_eq!(
        cli.pane_read("w1:p1", 200),
        [
            "--session",
            "d0123456789ab",
            "pane",
            "read",
            "w1:p1",
            "--lines",
            "200"
        ]
    );
    assert_eq!(
        cli.pane_list(),
        ["--session", "d0123456789ab", "pane", "list"]
    );
}

#[test]
fn agent_kinds_are_the_closed_recorded_set() {
    assert_eq!(
        classify("claude"),
        terminal::herdr::PaneKind::Agent(terminal::herdr::AgentKind::Claude)
    );
    assert_eq!(
        classify("qodercli"),
        terminal::herdr::PaneKind::Agent(terminal::herdr::AgentKind::Qodercli)
    );
    assert_eq!(
        classify("maki"),
        terminal::herdr::PaneKind::Agent(terminal::herdr::AgentKind::Maki)
    );
    assert_eq!(
        classify("/usr/local/bin/claude"),
        terminal::herdr::PaneKind::Plain
    );
    assert_eq!(classify("not-a-kind"), terminal::herdr::PaneKind::Plain);
}

async fn the_probe_accepts_only_the_pinned_version() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let pinned = write_herdr_script(dir.path(), PINNED_VERSION)?;
    let found = probe(&[dir.path().to_path_buf()], None)
        .await
        .map_err(|failure| format!("pinned probe: {failure:?}"))?;
    assert_eq!(found.path, pinned);

    let stale = write_herdr_script(dir.path(), "0.7.9")?;
    let failure = probe(&[dir.path().to_path_buf()], None)
        .await
        .err()
        .ok_or("expected a version refusal")?;
    let terminal::herdr::ProbeFailure::Version { binary, found } = failure else {
        return Err(format!("unexpected failure: {failure:?}").into());
    };
    assert_eq!(binary, stale);
    assert_eq!(found, "0.7.9");

    let empty = tempfile::tempdir()?;
    let outcome = probe(&[empty.path().to_path_buf()], None).await;
    assert!(matches!(
        outcome,
        Err(terminal::herdr::ProbeFailure::NotFound)
    ));
    Ok(())
}

async fn the_home_local_bin_is_searched_when_the_path_misses() -> Result<(), Failed> {
    let home = tempfile::tempdir()?;
    let bin = home.path().join(".local").join("bin");
    std::fs::create_dir_all(&bin)?;
    std::fs::write(
        bin.join("herdr"),
        format!("#!/bin/sh\necho {PINNED_VERSION}\n"),
    )?;
    std::fs::set_permissions(bin.join("herdr"), std::fs::Permissions::from_mode(0o755))?;
    let empty = tempfile::tempdir()?;
    let found = probe(&[empty.path().to_path_buf()], Some(home.path()))
        .await
        .map_err(|failure| format!("home probe: {failure:?}"))?;
    assert_eq!(found.path, bin.join("herdr"));
    Ok(())
}

async fn mux_resolution_lands_on_the_recorded_states() -> Result<(), Failed> {
    let state_dir = tempfile::tempdir()?;
    let with_herdr = tempfile::tempdir()?;
    write_herdr_script(with_herdr.path(), PINNED_VERSION)?;
    let paths = vec![with_herdr.path().to_path_buf()];

    let auto = mux::resolve(
        common::settings(&config::Environment::default()).mux,
        &paths,
        None,
        state_dir.path(),
        5174,
    )
    .await;
    assert!(matches!(auto, mux::MuxPlan::Herdr(_)), "{auto:?}");

    let explicit = mux::resolve(
        common::settings(&common::env_from(&[("RIVETHUB_TERM_MUX", "herdr")])).mux,
        &paths,
        None,
        state_dir.path(),
        5174,
    )
    .await;
    assert!(matches!(explicit, mux::MuxPlan::Herdr(_)), "{explicit:?}");

    let stale = write_herdr_script(with_herdr.path(), "0.1.0")?;
    let unavailable = mux::resolve(
        common::settings(&common::env_from(&[("RIVETHUB_TERM_MUX", "herdr")])).mux,
        &paths,
        None,
        state_dir.path(),
        5174,
    )
    .await;
    let mux::MuxPlan::Unavailable(unavailable) = &unavailable else {
        return Err(format!("expected terminals unavailable, got {unavailable:?}").into());
    };
    assert_eq!(
        unavailable.reason.text(),
        format!(
            "herdr at {} is not {PINNED_VERSION} (RivetHub pins {PINNED_VERSION} / protocol 20)",
            stale.display()
        )
    );

    let auto_without = mux::resolve(
        common::settings(&config::Environment::default()).mux,
        &paths,
        None,
        state_dir.path(),
        5174,
    )
    .await;
    assert_eq!(auto_without, mux::MuxPlan::Direct);

    let direct = mux::resolve(
        common::settings(&common::env_from(&[("RIVETHUB_TERM_MUX", "none")])).mux,
        &paths,
        None,
        state_dir.path(),
        5174,
    )
    .await;
    assert_eq!(direct, mux::MuxPlan::Direct);

    let bogus = mux::resolve(
        common::settings(&common::env_from(&[("RIVETHUB_TERM_MUX", "Bogus")])).mux,
        &paths,
        None,
        state_dir.path(),
        5174,
    )
    .await;
    assert_eq!(bogus, mux::MuxPlan::Direct);
    Ok(())
}

struct FakeStep {
    method: RpcMethod,
    params: RequestParams,
    frames: Vec<String>,
}

async fn serve_steps(listener: &UnixListener, steps: &[FakeStep]) -> Result<(), Failed> {
    for expected in steps {
        let (stream, _) = listener.accept().await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line).await?;
        if read == 0 {
            return Err("the client closed the socket early".into());
        }
        let frame = codec::decode_request(&line)?;
        assert_eq!(
            frame.method, expected.method,
            "expected {} got {frame:?}",
            expected.method
        );
        assert_eq!(
            frame.params, expected.params,
            "params for {}: expected {:?}",
            expected.method, expected.params
        );
        for template in &expected.frames {
            writer
                .write_all(template.replace("{ID}", &frame.id).as_bytes())
                .await?;
            writer.flush().await?;
        }
    }
    Ok(())
}

async fn run_server(listener: UnixListener, steps: Vec<FakeStep>) -> Result<(), Failed> {
    serve_steps(&listener, &steps).await
}

async fn run_server_until_stopped(
    listener: UnixListener,
    steps: Vec<FakeStep>,
    closed: tokio::sync::oneshot::Sender<()>,
) -> Result<(), Failed> {
    serve_steps(&listener, &steps).await?;
    let (probe, _) = listener.accept().await?;
    drop(probe);
    drop(listener);
    closed
        .send(())
        .map_err(|()| "the stop-ordering signal was dropped".to_string())?;
    Ok(())
}

const ACK_PLAIN: &[&str] = &["{\"id\":\"{ID}\",\"result\":{\"type\":\"workspace_created\"}}\n"];
const ACK_SUBSCRIPTION: &[&str] =
    &["{\"id\":\"{ID}\",\"result\":{\"type\":\"subscription_started\"}}\n"];

fn fake_step(method: RpcMethod, params: RequestParams, frames: &[&str]) -> FakeStep {
    FakeStep {
        method,
        params,
        frames: frames.iter().map(ToString::to_string).collect(),
    }
}

fn setup_herdr_dir() -> Result<(tempfile::TempDir, std::path::PathBuf), Failed> {
    let dir = tempfile::tempdir()?;
    let setup_dir = dir.path().join("c");
    std::fs::create_dir_all(&setup_dir)?;
    Ok((dir, setup_dir))
}

fn agent_pane_steps(name: &str) -> Vec<FakeStep> {
    vec![
        fake_step(
            RpcMethod::WorkspaceCreate,
            RequestParams::WorkspaceCreate(codec::WorkspaceCreateParams {
                cwd: PathBuf::from("/home/user/AcmeKit"),
                env: std::collections::BTreeMap::from([(
                    "ACME_ENTRY_ENV".to_owned(),
                    "envdump".to_owned(),
                )]),
                label: "claude".to_owned(),
            }),
            ACK_PLAIN,
        ),
        fake_step(
            RpcMethod::AgentStart,
            RequestParams::AgentStart(codec::AgentStartParams {
                name: name.to_owned(),
                kind: AgentKind::Claude,
                pane_id: "w1:p1".to_owned(),
                args: vec![
                    "--resume".to_owned(),
                    "00000000-0000-4000-8000-000000000002".to_owned(),
                ],
                timeout_ms: 10_000,
            }),
            ACK_PLAIN,
        ),
        fake_step(
            RpcMethod::EventsSubscribe,
            RequestParams::EventsSubscribe(codec::EventsSubscribeParams {
                subscriptions: vec![
                    codec::SubscriptionParams {
                        event: HerdrEventKind::PaneAgentStatusChanged,
                        pane_id: "w1:p1".to_owned(),
                    },
                    codec::SubscriptionParams {
                        event: HerdrEventKind::PaneAgentDetected,
                        pane_id: "w1:p1".to_owned(),
                    },
                ],
            }),
            ACK_SUBSCRIPTION,
        ),
    ]
}

async fn agent_panes_run_the_recorded_setup_sequence() -> Result<(), Failed> {
    let (home, config_home) = setup_herdr_dir()?;
    let key = SessionKey::new("claude-code:00000000-0000-4000-8000-000000000001")?;
    let name = paths::session_dir_name(&key);
    let sessions = config_home.join("herdr").join("sessions").join(&name);
    std::fs::create_dir_all(&sessions)?;
    let socket_path = sessions.join("herdr.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let binary = write_herdr_script(home.path(), PINNED_VERSION)?;
    let server = tokio::spawn(run_server(listener, agent_pane_steps(&name)));
    let setup = mux::HerdrSetup {
        binary,
        config_home,
    };
    let session = terminal::herdr::HerdrSession::ensure(
        &setup,
        &key,
        &[
            "claude".to_owned(),
            "--resume".to_owned(),
            "00000000-0000-4000-8000-000000000002".to_owned(),
        ],
        Path::new("/home/user/AcmeKit"),
        &std::collections::BTreeMap::from([("ACME_ENTRY_ENV".to_owned(), "envdump".to_owned())]),
        "claude",
    )
    .await?;
    assert_eq!(session.pane, "w1:p1");
    session.stop().await?;
    let joined = tokio::time::timeout(Duration::from_secs(5), server).await??;
    joined?;
    Ok(())
}

async fn stop_reports_success_only_after_the_server_is_gone() -> Result<(), Failed> {
    let (home, config_home) = setup_herdr_dir()?;
    let key = SessionKey::new("pi:00000000-0000-4000-8000-00000000000a")?;
    let name = paths::session_dir_name(&key);
    let sessions = config_home.join("herdr").join("sessions").join(&name);
    std::fs::create_dir_all(&sessions)?;
    let socket_path = sessions.join("herdr.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let binary = write_herdr_script(home.path(), PINNED_VERSION)?;
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(run_server_until_stopped(
        listener,
        vec![
            fake_step(
                RpcMethod::WorkspaceCreate,
                RequestParams::WorkspaceCreate(codec::WorkspaceCreateParams {
                    cwd: PathBuf::from("/home/user/AcmeKit"),
                    env: std::collections::BTreeMap::new(),
                    label: "pi".to_owned(),
                }),
                ACK_PLAIN,
            ),
            fake_step(
                RpcMethod::PaneSendText,
                RequestParams::PaneSendText(codec::PaneSendTextParams {
                    pane_id: "w1:p1".to_owned(),
                    text: "bash -l\n".to_owned(),
                }),
                ACK_PLAIN,
            ),
        ],
        closed_tx,
    ));
    let setup = mux::HerdrSetup {
        binary,
        config_home,
    };
    let session = terminal::herdr::HerdrSession::ensure(
        &setup,
        &key,
        &["bash".to_owned(), "-l".to_owned()],
        Path::new("/home/user/AcmeKit"),
        &std::collections::BTreeMap::new(),
        "pi",
    )
    .await?;
    session.stop().await?;
    let () = tokio::time::timeout(Duration::from_secs(5), closed_rx)
        .await
        .map_err(|_| "stop reported success before the server socket closed")??;
    assert!(
        !socket_path.exists(),
        "a confirmed stop removes the session dir"
    );
    let joined = tokio::time::timeout(Duration::from_secs(5), server).await??;
    joined?;
    Ok(())
}

async fn plain_panes_are_started_with_send_text() -> Result<(), Failed> {
    let (home, config_home) = setup_herdr_dir()?;
    let key = SessionKey::new("pi:00000000-0000-4000-8000-000000000009")?;
    let name = paths::session_dir_name(&key);
    let sessions = config_home.join("herdr").join("sessions").join(&name);
    std::fs::create_dir_all(&sessions)?;
    let socket_path = sessions.join("herdr.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let binary = write_herdr_script(home.path(), PINNED_VERSION)?;
    let server = tokio::spawn(run_server(
        listener,
        vec![
            fake_step(
                RpcMethod::WorkspaceCreate,
                RequestParams::WorkspaceCreate(codec::WorkspaceCreateParams {
                    cwd: PathBuf::from("/home/user/AcmeKit"),
                    env: std::collections::BTreeMap::new(),
                    label: "pi".to_owned(),
                }),
                ACK_PLAIN,
            ),
            fake_step(
                RpcMethod::PaneSendText,
                RequestParams::PaneSendText(codec::PaneSendTextParams {
                    pane_id: "w1:p1".to_owned(),
                    text: "bash -l\n".to_owned(),
                }),
                ACK_PLAIN,
            ),
        ],
    ));
    let setup = mux::HerdrSetup {
        binary,
        config_home,
    };
    terminal::herdr::HerdrSession::ensure(
        &setup,
        &key,
        &["bash".to_owned(), "-l".to_owned()],
        Path::new("/home/user/AcmeKit"),
        &std::collections::BTreeMap::new(),
        "pi",
    )
    .await?;
    let joined = tokio::time::timeout(Duration::from_secs(5), server).await??;
    joined?;
    Ok(())
}

async fn a_missing_server_hits_the_recorded_timeout_text() -> Result<(), Failed> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("herdr").join("sessions").join("herdr.sock");
    let error = paths::wait_for_socket(&socket, Duration::from_millis(300), "d0123456789ab")
        .await
        .err()
        .ok_or("expected the timeout")?;
    let Error::HerdrServerTimeout { timeout_ms, .. } = error else {
        return Err(format!("expected the server-open timeout, got {error}").into());
    };
    assert_eq!(timeout_ms, 300);
    Ok(())
}

#[test]
fn the_herdr_probe_paths_and_session_setup() -> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(the_probe_accepts_only_the_pinned_version())?;
    runtime.block_on(the_home_local_bin_is_searched_when_the_path_misses())?;
    runtime.block_on(mux_resolution_lands_on_the_recorded_states())?;
    runtime.block_on(agent_panes_run_the_recorded_setup_sequence())?;
    runtime.block_on(plain_panes_are_started_with_send_text())?;
    runtime.block_on(stop_reports_success_only_after_the_server_is_gone())?;
    runtime.block_on(a_missing_server_hits_the_recorded_timeout_text())
}
