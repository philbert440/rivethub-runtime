#![cfg(test)]
#[expect(
    dead_code,
    reason = "integration binaries share fixtures and use different subsets"
)]
mod session_support;

use std::sync::Arc;

use futures::SinkExt as _;
use kernel::terminal::{TermSize, TerminalId};
use session_support::{Call, Fixture, WAIT, frame, http, user};
use tokio_tungstenite::tungstenite::Message;
use wire::events::{AgentEvent, AgentEventKind, ProtocolVersion, SessionBroadcast, Snapshot};
use wire::gateway::term::{TermServerFrame, TermSpawnRequest};

fn event(session: &str, text: &str) -> AgentEvent {
    AgentEvent {
        v: ProtocolVersion::V1,
        session: session.into(),
        kind: AgentEventKind::MessageUser {
            text: text.into(),
            ts: None,
        },
        extra: wire::Extra::default(),
    }
}

fn spawn(session: &str) -> TermSpawnRequest {
    TermSpawnRequest {
        session: Some(session.into()),
        command: Some("echo".into()),
        model: None,
        effort: None,
        resume: None,
        agent_id: None,
        force: None,
        cols: None,
        rows: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_spawns_have_one_owner_and_one_process() {
    let fixture = Fixture::new().await;
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let service = fixture.service.clone();
        calls.spawn(async move { service.spawn(&user("owner"), spawn("same-session")).await });
    }
    while let Some(call) = calls.join_next().await {
        assert_eq!(call.expect("task").expect("spawn").id, "pty-1");
    }
    assert_eq!(
        fixture
            .mux
            .calls
            .borrow()
            .iter()
            .filter(|call| matches!(call, Call::Spawn { .. }))
            .count(),
        1
    );
    assert!(
        fixture
            .service
            .spawn(&user("alice"), spawn("same-session"))
            .await
            .is_err()
    );
    assert_eq!(fixture.service.list(&user("alice")), Vec::new());
    fixture.gateway.close().await;
}

#[tokio::test]
async fn tenant_isolation_includes_batches_snapshots_live_events_and_aliases() {
    let fixture = Fixture::new().await;
    fixture
        .registry
        .ingest(
            &user("owner"),
            vec![event("11111111-2222-4333-8444-555555555555", "private")],
        )
        .expect("event");
    fixture
        .registry
        .ingest(&user("alice"), vec![event("alice", "own")])
        .expect("event");
    let (snapshot, mut live) = fixture
        .registry
        .subscribe(&user("alice"), None)
        .expect("subscribe");
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions.first().expect("session").id, "alice");
    assert!(
        fixture
            .registry
            .state(
                &user("alice"),
                "claude-code:11111111-2222-4333-8444-555555555555"
            )
            .is_err()
    );
    assert!(
        fixture
            .registry
            .ingest(
                &user("alice"),
                vec![
                    event("new-before-denied", "a"),
                    event("11111111-2222-4333-8444-555555555555", "bad")
                ]
            )
            .is_err()
    );
    assert!(
        fixture
            .registry
            .authorize(&user("alice"), "new-before-denied")
            .is_err()
    );
    fixture
        .registry
        .ingest(&user("owner"), vec![event("owner-other", "secret")])
        .expect("event");
    fixture
        .registry
        .ingest(&user("alice"), vec![event("alice", "visible")])
        .expect("event");
    let delivered = tokio::time::timeout(WAIT, live.recv())
        .await
        .expect("timeout")
        .expect("event");
    assert!(
        matches!(delivered, SessionBroadcast::Event(AgentEvent { session, .. }) if session == "alice")
    );
    fixture.gateway.close().await;
}

#[tokio::test]
async fn identity_defaults_to_refusal_and_invalid_tokens_do_not_become_local_owner() {
    let fixture = Fixture::new().await;
    for token in ["", "bad", "alice-test-tokeN"] {
        assert_eq!(
            http(
                &fixture.gateway,
                "GET",
                "/sessions",
                Vec::new(),
                Some(token)
            )
            .await
            .0,
            401
        );
    }
    let unconfigured = Arc::new(gateway::Gateway::new(
        Arc::new(testkit::FakeMemoryStore::default()),
        fixture.gateway.settings().clone(),
    ));
    assert_eq!(
        http(&unconfigured, "GET", "/sessions", Vec::new(), None)
            .await
            .0,
        401
    );
    assert_eq!(
        http(
            &fixture.gateway,
            "GET",
            "/sessions",
            Vec::new(),
            Some("alice-test-token")
        )
        .await
        .0,
        200
    );
    fixture.gateway.close().await;
}

#[tokio::test]
async fn terminal_bytes_are_preserved_and_exit_precedes_close() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .spawn(&user("owner"), spawn("bytes"))
        .await
        .expect("spawn");
    let (address, stop, server) = fixture.serve().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/term?id=pty-1"))
        .await
        .expect("connect");
    assert!(matches!(frame(&mut socket).await, Message::Text(_)));
    assert!(matches!(frame(&mut socket).await, Message::Binary(_)));
    for bytes in [vec![0xc3], vec![0xa9], vec![0xff, 0x00]] {
        socket
            .send(Message::binary(bytes.clone()))
            .await
            .expect("input");
        fixture
            .mux
            .called(|calls| calls.contains(&Call::Input(bytes.clone())))
            .await;
    }
    fixture
        .mux
        .emit(&TerminalId::new("pty-1"), vec![0xff, 0x00, 0xc3]);
    assert_eq!(
        frame(&mut socket).await.into_data().as_ref(),
        &[0xff, 0x00, 0xc3]
    );
    fixture.mux.exit(&TerminalId::new("pty-1"), 3);
    let exit: TermServerFrame =
        serde_json::from_str(&frame(&mut socket).await.into_text().expect("exit text"))
            .expect("exit");
    assert_eq!(
        exit,
        TermServerFrame::Exit {
            code: Some(3),
            signal: None
        }
    );
    assert!(
        matches!(frame(&mut socket).await, Message::Close(Some(close)) if close.code == tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal)
    );
    drop(socket);
    stop.cancel();
    tokio::time::timeout(WAIT, server)
        .await
        .expect("shutdown")
        .expect("task")
        .expect("server");
}

#[tokio::test]
async fn snapshot_then_events_has_no_gap_under_concurrent_ingest() {
    let fixture = Fixture::new().await;
    let (address, stop, server) = fixture.serve().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("connect");
    fixture
        .registry
        .ingest(&user("owner"), vec![event("ordered", "after-upgrade")])
        .expect("ingest");
    let snapshot: Snapshot =
        serde_json::from_str(&frame(&mut socket).await.into_text().expect("snapshot"))
            .expect("snapshot");
    assert_eq!(snapshot.sessions, Vec::new());
    let delivered: AgentEvent =
        serde_json::from_str(&frame(&mut socket).await.into_text().expect("event")).expect("event");
    assert_eq!(delivered.session, "ordered");
    drop(socket);
    stop.cancel();
    tokio::time::timeout(WAIT, server)
        .await
        .expect("shutdown")
        .expect("task")
        .expect("server");
}

#[tokio::test]
async fn roster_commands_may_start_outside_the_files_root_but_must_be_absolute() {
    use gateway::term::{LaunchCommand, TerminalOptions, TerminalService};
    let fixture = Fixture::new().await;
    let root = fixture.directory.path().join("allowed");
    let sibling = fixture.directory.path().join("allowed-sibling");
    std::fs::create_dir(&root).expect("root");
    std::fs::create_dir(&sibling).expect("sibling");
    for (cwd, accepted) in [
        (sibling.clone(), true),
        (std::path::PathBuf::from("relative/dir"), false),
    ] {
        let result = TerminalService::new(
            fixture.mux.clone(),
            fixture.registry.clone(),
            TerminalOptions {
                root: root.clone(),
                default: "shell".into(),
                max_ptys: 1,
                commands: vec![LaunchCommand {
                    id: "shell".into(),
                    label: "Shell".into(),
                    argv: vec!["sh".into()],
                    cwd: cwd.clone(),
                    harness: None,
                    session: false,
                    env: std::collections::BTreeMap::new(),
                }],
            },
        )
        .await;
        assert_eq!(result.is_ok(), accepted, "{}", cwd.display());
    }
    fixture.gateway.close().await;
}

#[tokio::test]
async fn a_chat_directory_outside_the_root_or_missing_is_named_in_the_refusal() {
    let harness = Arc::new(session_support::harness::Harness::default());
    let fixture = Fixture::with_harness(Some(harness)).await;
    let outside = tempfile::tempdir().expect("outside directory");
    for (cwd, refusal) in [
        (
            outside.path().to_path_buf(),
            "working directory is outside the terminal root",
        ),
        (
            fixture.directory.path().join("missing"),
            "working directory does not exist",
        ),
    ] {
        let error = fixture
            .service
            .create_chat(
                &user("owner"),
                wire::harness::HarnessId::ClaudeCode,
                None,
                Some(cwd),
            )
            .await
            .expect_err("refused directory");
        assert_eq!(error.to_string(), refusal);
    }
    fixture.gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handoff_waits_for_turn_end_releases_then_resumes_and_delivers_queued_chat() {
    let harness = Arc::new(session_support::harness::Harness::default());
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    let session = fixture
        .service
        .create_chat(
            &user("owner"),
            wire::harness::HarnessId::ClaudeCode,
            None,
            None,
        )
        .await
        .expect("chat session");
    let chat = fixture
        .service
        .chat(&user("owner"), session.session_id.as_str())
        .expect("chat");
    chat.prompt("first".into(), None, Vec::new())
        .await
        .expect("prompt accepted");
    harness.traced("prompt:first").await;
    let service = fixture.service.clone();
    let id = session.session_id.clone();
    let mut switching = tokio::spawn(async move {
        let mut request = spawn(id.as_str());
        request.command = Some("claude".into());
        service.spawn(&user("owner"), request).await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::ZERO, &mut switching)
            .await
            .is_err()
    );
    assert!(!harness.trace.borrow().contains(&"release".to_owned()));
    harness.finish.add_permits(1);
    let terminal = tokio::time::timeout(WAIT, switching)
        .await
        .expect("handoff timeout")
        .expect("task")
        .expect("terminal");
    assert_eq!(terminal.id, "pty-1");
    assert_eq!(
        &*harness.trace.borrow(),
        &["new", "prompt:first", "ended", "release"]
    );
    let native = session
        .session_id
        .as_str()
        .split_once(':')
        .expect("canonical")
        .1;
    assert!(fixture.mux.calls.borrow().iter().any(|call| matches!(call, Call::Spawn { argv, .. } if argv.ends_with(&["--resume".into(), native.into()]))));
    chat.prompt("queued".into(), None, Vec::new())
        .await
        .expect("queued chat");
    assert!(
        !fixture
            .mux
            .calls
            .borrow()
            .iter()
            .any(|call| matches!(call, Call::Input(_)))
    );
    chat.turn_end().await.expect("terminal hook");
    harness.traced("prompt:queued").await;
    assert!(fixture.mux.calls.borrow().contains(&Call::Kill));
    assert_eq!(
        &*harness.trace.borrow(),
        &[
            "new",
            "prompt:first",
            "ended",
            "release",
            "resume",
            "prompt:queued"
        ]
    );
    harness.finish.add_permits(1);
    fixture.gateway.close().await;
}

#[tokio::test]
async fn terminal_first_session_queues_chat_until_the_hook_and_delete_releases_chat() {
    let harness = Arc::new(session_support::harness::Harness::default());
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    let mut request = spawn("11111111-2222-4333-8444-555555555555");
    request.command = Some("claude".into());
    fixture
        .service
        .spawn(&user("owner"), request)
        .await
        .expect("terminal first");
    let chat = fixture
        .service
        .chat(
            &user("owner"),
            "claude-code:11111111-2222-4333-8444-555555555555",
        )
        .expect("registered session");
    chat.prompt("queued".into(), None, Vec::new())
        .await
        .expect("queued");
    assert_eq!(*harness.trace.borrow(), Vec::<String>::new());
    chat.turn_end().await.expect("hook");
    harness.traced("prompt:queued").await;
    fixture
        .service
        .remove_session(&user("owner"), "11111111-2222-4333-8444-555555555555")
        .await
        .expect("close session");
    harness.traced("release").await;
    assert!(
        fixture
            .service
            .chat(&user("owner"), "11111111-2222-4333-8444-555555555555")
            .is_err()
    );
    fixture.gateway.close().await;
}

#[tokio::test]
async fn malformed_controls_are_rejected_without_writing_to_the_terminal() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .spawn(&user("owner"), spawn("control"))
        .await
        .expect("spawn");
    let (address, stop, server) = fixture.serve().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/term?id=pty-1"))
        .await
        .expect("connect");
    frame(&mut socket).await;
    frame(&mut socket).await;
    socket
        .send(Message::text(r#"{"type":"resize","cols":"90","rows":30}"#))
        .await
        .expect("malformed input");
    assert!(
        matches!(frame(&mut socket).await, Message::Close(Some(close)) if close.code == tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy)
    );
    assert!(
        !fixture
            .mux
            .calls
            .borrow()
            .iter()
            .any(|call| matches!(call, Call::Resize(_) | Call::Input(_)))
    );
    drop(socket);
    stop.cancel();
    tokio::time::timeout(WAIT, server)
        .await
        .expect("shutdown")
        .expect("task")
        .expect("server");
}

async fn viewer(address: std::net::SocketAddr) -> session_support::Socket {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/term?id=pty-1"))
        .await
        .expect("connect");
    assert!(matches!(frame(&mut socket).await, Message::Text(_)));
    assert!(matches!(frame(&mut socket).await, Message::Binary(_)));
    socket
}

#[tokio::test]
async fn when_the_owner_leaves_the_other_viewers_learn_nobody_owns_the_terminal() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .spawn(&user("owner"), spawn("owned"))
        .await
        .expect("spawn");
    let (address, stop, server) = fixture.serve().await;
    let mut owner = viewer(address).await;
    owner
        .send(Message::text(r#"{"type":"resize","cols":90,"rows":30}"#))
        .await
        .expect("resize");
    fixture
        .mux
        .called(|calls| calls.contains(&Call::Resize(TermSize { cols: 90, rows: 30 })))
        .await;
    let mut first = viewer(address).await;
    let mut second = viewer(address).await;
    owner.close(None).await.expect("owner leaves");
    for socket in [&mut first, &mut second] {
        let released: TermServerFrame =
            serde_json::from_str(&frame(socket).await.into_text().expect("owner text"))
                .expect("owner frame");
        assert!(
            matches!(
                released,
                TermServerFrame::Owner {
                    device: None,
                    self_owner: false,
                    ..
                }
            ),
            "{released:?}"
        );
    }
    drop((first, second));
    stop.cancel();
    tokio::time::timeout(WAIT, server)
        .await
        .expect("shutdown")
        .expect("task")
        .expect("server");
}

#[tokio::test]
async fn generated_shell_sessions_use_unique_rivethub_uuids() {
    let fixture = Fixture::new().await;
    let mut request = spawn("unused");
    request.session = None;
    request.command = Some("shell".into());
    let first = fixture
        .service
        .spawn(&user("owner"), request.clone())
        .await
        .expect("first shell");
    let second = fixture
        .service
        .spawn(&user("owner"), request)
        .await
        .expect("second shell");
    assert_ne!(first.session, second.session);
    for session in [&first.session, &second.session] {
        let suffix = session.strip_prefix("rivethub-").expect("RivetHub prefix");
        let uuid = uuid::Uuid::parse_str(suffix).expect("UUID");
        assert_eq!(uuid.get_version(), Some(uuid::Version::Random));
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
        assert_eq!(
            fixture
                .registry
                .authorize(&user("owner"), session)
                .expect("registered owner"),
            *session
        );
        assert!(fixture.registry.authorize(&user("alice"), session).is_err());
    }
    fixture.gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_resume_passes_the_native_id_to_the_harness() {
    let fixture = Fixture::new().await;
    let native = "8922b6dd-d60f-42d9-9690-51b0d6e63176";
    let canonical = format!("claude-code:{native}");
    fixture
        .registry
        .claim(&user("owner"), native)
        .expect("resume owner seed");
    let mut request = spawn(&canonical);
    request.command = Some("claude".into());
    request.resume = Some(canonical.clone());
    fixture
        .service
        .spawn(&user("owner"), request)
        .await
        .expect("resume spawn");
    let spawned = fixture
        .mux
        .calls
        .borrow()
        .iter()
        .find_map(|call| match call {
            Call::Spawn { argv, .. } => Some(argv.clone()),
            Call::Input(_) | Call::Resize(_) | Call::Kill => None,
        })
        .expect("one spawn");
    assert!(
        spawned.ends_with(&["--resume".into(), native.into()]),
        "argv was {spawned:?}"
    );
    assert!(!spawned.iter().any(|arg| arg.contains("claude-code:")));
}

fn spawned_argv(fixture: &Fixture) -> Vec<String> {
    fixture
        .mux
        .calls
        .borrow()
        .iter()
        .find_map(|call| match call {
            Call::Spawn { argv, .. } => Some(argv.clone()),
            Call::Input(_) | Call::Resize(_) | Call::Kill => None,
        })
        .expect("one spawn")
}

#[tokio::test]
async fn a_new_harness_terminal_pins_the_registered_session_id() {
    let fixture = Fixture::new().await;
    let session = "0f6c3a52-7d41-4e8b-a2c9-5b1e8d7f3a60";
    let mut request = spawn(session);
    request.command = Some("claude".into());
    request.model = Some("fable".into());
    let response = fixture
        .service
        .spawn(&user("owner"), request)
        .await
        .expect("fresh spawn");
    assert_eq!(response.session, session);
    assert_eq!(
        spawned_argv(&fixture),
        ["claude", "--session-id", session, "--model", "fable"]
    );
    fixture.gateway.close().await;
}

#[tokio::test]
async fn a_codex_terminal_resumes_through_its_subcommand() {
    let fixture = Fixture::with_lister(
        None,
        gateway::sessions::SessionRegistry::new(std::time::Duration::from_secs(1)),
        None,
        &[("codex", wire::harness::HarnessId::Codex)],
    )
    .await;
    let native = "8922b6dd-d60f-42d9-9690-51b0d6e63176";
    let mut request = spawn(&format!("codex:{native}"));
    request.command = Some("codex".into());
    request.resume = Some(format!("codex:{native}"));
    fixture
        .service
        .spawn(&user("owner"), request)
        .await
        .expect("resume spawn");
    assert_eq!(spawned_argv(&fixture), ["codex", "resume", native]);
    fixture.gateway.close().await;
}

fn inputs(fixture: &Fixture) -> Vec<Vec<u8>> {
    fixture
        .mux
        .calls
        .borrow()
        .iter()
        .filter_map(|call| match call {
            Call::Input(bytes) => Some(bytes.clone()),
            Call::Spawn { .. } | Call::Resize(_) | Call::Kill => None,
        })
        .collect()
}

#[tokio::test]
async fn an_inject_pastes_the_text_then_submits_it() {
    let fixture = Fixture::new().await;
    let (status, body) = http(
        &fixture.gateway,
        "POST",
        "/api/terminal",
        r#"{"session":"aaa"}"#,
        None,
    )
    .await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&body));
    let (status, body) = http(
        &fixture.gateway,
        "POST",
        "/api/terminal/inject",
        r#"{"session":"aaa","text":"ls\u001b[201~ -la"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        inputs(&fixture),
        vec![b"\x1b[200~ls[201~ -la\x1b[201~".to_vec(), b"\r".to_vec()]
    );
}

#[tokio::test]
async fn an_inject_without_submit_types_the_text_after_an_interrupt() {
    let fixture = Fixture::new().await;
    let (status, body) = http(
        &fixture.gateway,
        "POST",
        "/api/terminal",
        r#"{"session":"aaa"}"#,
        None,
    )
    .await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&body));
    let (status, body) = http(
        &fixture.gateway,
        "POST",
        "/api/terminal/inject",
        r#"{"session":"aaa","text":"draft","submit":false,"interrupt":true}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(inputs(&fixture), vec![b"\x1b".to_vec(), b"draft".to_vec()]);
}

#[tokio::test]
async fn an_inject_without_a_terminal_writes_nothing() {
    let fixture = Fixture::new().await;
    let (status, _) = http(
        &fixture.gateway,
        "POST",
        "/api/terminal/inject",
        r#"{"session":"aaa","text":"ls"}"#,
        None,
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(inputs(&fixture), Vec::<Vec<u8>>::new());
}
