#![cfg(test)]
use crate::session_support;

use std::sync::Arc;
use std::time::Duration;

use gateway::sessions::SessionRegistry;
use session_support::harness::Harness;
use session_support::{Fixture, Socket, WAIT, frame, http};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use wire::harness::{HarnessEvent, HarnessId, HarnessSessionStatus, HarnessSessionSummary};
use wire::session_id::SessionId;

const ALICE: Option<&str> = Some("alice-test-token");

struct Setup {
    fixture: Fixture,
    harness: Arc<Harness>,
    address: std::net::SocketAddr,
    stop: tokio_util::sync::CancellationToken,
}

async fn setup() -> Setup {
    let harness = Arc::new(Harness::offering(&[
        HarnessId::ClaudeCode,
        HarnessId::Hermes,
    ]));
    let fixture = Fixture::with_lister(
        Some(harness.clone()),
        SessionRegistry::new(Duration::from_secs(1)),
        None,
        &[("hermes", HarnessId::Hermes)],
    )
    .await;
    let (address, stop, _server) = fixture.serve().await;
    Setup {
        fixture,
        harness,
        address,
        stop,
    }
}

async fn watch(setup: &Setup, query: &str, token: Option<&str>) -> Socket {
    let mut request = format!("ws://{}/api/harnesses/ws{query}", setup.address)
        .into_client_request()
        .expect("request");
    if let Some(token) = token {
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("header"),
        );
    }
    tokio_tungstenite::connect_async(request)
        .await
        .expect("connect")
        .0
}

async fn create(setup: &Setup, harness: &str, token: Option<&str>) -> SessionId {
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        "/api/harness-sessions",
        format!(r#"{{"harness":"{harness}"}}"#),
        token,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice::<HarnessSessionSummary>(&body)
        .expect("summary")
        .session_id
}

async fn event(socket: &mut Socket) -> HarnessEvent {
    serde_json::from_str(&frame(socket).await.into_text().expect("text frame")).expect("event")
}

async fn created(socket: &mut Socket) -> (SessionId, HarnessId) {
    let HarnessEvent::SessionCreated {
        session_id,
        summary,
        supersedes,
    } = event(socket).await
    else {
        panic!("session-created expected");
    };
    assert_eq!(supersedes, None);
    assert_eq!(summary.session_id, session_id);
    (session_id, summary.harness_id)
}

async fn updated(socket: &mut Socket) -> (SessionId, HarnessSessionStatus) {
    let HarnessEvent::SessionUpdated {
        session_id, status, ..
    } = event(socket).await
    else {
        panic!("session-updated expected");
    };
    (session_id, status)
}

#[tokio::test]
async fn a_new_chat_and_its_turn_reach_the_registry_socket() {
    let setup = setup().await;
    let mut socket = watch(&setup, "", None).await;
    let session = create(&setup, "claude-code", None).await;
    assert_eq!(
        created(&mut socket).await,
        (session.clone(), HarnessId::ClaudeCode)
    );
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{session}/turn"),
        r#"{"text":"hi"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        updated(&mut socket).await,
        (session.clone(), HarnessSessionStatus::Active)
    );
    setup.harness.traced("prompt:hi").await;
    setup.harness.finish.add_permits(1);
    assert_eq!(
        updated(&mut socket).await,
        (session, HarnessSessionStatus::Idle)
    );
    setup.stop.cancel();
}

#[tokio::test]
async fn another_users_chats_stay_off_the_registry_socket() {
    let setup = setup().await;
    let mut socket = watch(&setup, "", ALICE).await;
    let owners = create(&setup, "claude-code", None).await;
    let alices = create(&setup, "claude-code", ALICE).await;
    let (first, _) = created(&mut socket).await;
    assert_ne!(first, owners);
    assert_eq!(first, alices);
    setup.stop.cancel();
}

#[tokio::test]
async fn a_harness_filter_keeps_other_harnesses_off_the_socket() {
    let setup = setup().await;
    let mut socket = watch(&setup, "?harness=claude-code", None).await;
    create(&setup, "hermes", None).await;
    let claude = create(&setup, "claude-code", None).await;
    assert_eq!(created(&mut socket).await, (claude, HarnessId::ClaudeCode));
    setup.stop.cancel();
}

#[tokio::test]
async fn an_unknown_harness_filter_refuses_the_upgrade() {
    let setup = setup().await;
    let refused = tokio_tungstenite::connect_async(format!(
        "ws://{}/api/harnesses/ws?harness=nope",
        setup.address
    ))
    .await;
    assert!(refused.is_err());
    let (status, _) = http(
        &setup.fixture.gateway,
        "GET",
        "/api/harnesses/ws?harness=nope",
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, 400);
    setup.stop.cancel();
}

#[tokio::test]
async fn a_socket_opens_on_a_node_without_chat_sessions() {
    let fixture = Fixture::new().await;
    let (address, stop, _server) = fixture.serve().await;
    let opened = tokio::time::timeout(
        WAIT,
        tokio_tungstenite::connect_async(format!("ws://{address}/api/harnesses/ws")),
    )
    .await
    .expect("connect timeout");
    assert!(opened.is_ok());
    stop.cancel();
}

async fn blocked(socket: &mut Socket) -> (HarnessSessionStatus, Option<bool>) {
    let HarnessEvent::SessionUpdated {
        status, blocked, ..
    } = event(socket).await
    else {
        panic!("session-updated expected");
    };
    (status, blocked)
}

#[tokio::test]
async fn a_pending_approval_blocks_the_session_until_it_is_answered() {
    let setup = setup().await;
    let mut socket = watch(&setup, "", None).await;
    let session = create(&setup, "claude-code", None).await;
    created(&mut socket).await;
    let (status, _) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{session}/turn"),
        r#"{"text":"hi"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        blocked(&mut socket).await,
        (HarnessSessionStatus::Active, None)
    );
    setup.harness.traced("prompt:hi").await;
    setup
        .harness
        .emit(
            &session,
            kernel::harness::SessionEvent::Permission(kernel::harness::PermissionRequest {
                request_id: "ask-1".into(),
                tool_call_id: None,
                name: "Bash".into(),
                input: wire::JsonObject::default(),
                reason: None,
            }),
        )
        .await;
    assert_eq!(
        blocked(&mut socket).await,
        (HarnessSessionStatus::Active, Some(true))
    );
    let listed = setup
        .fixture
        .service
        .harness_sessions(&session_support::user("owner"));
    assert_eq!(
        listed
            .sessions
            .iter()
            .map(|summary| summary.blocked)
            .collect::<Vec<_>>(),
        vec![Some(true)]
    );
    let (status, _) = http(
        &setup.fixture.gateway,
        "POST",
        &format!(
            "/api/harness-sessions/{}/approvals/ask-1",
            wire::session_id::encode_segment(&session)
        ),
        r#"{"decision":"allow"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        blocked(&mut socket).await,
        (HarnessSessionStatus::Active, Some(false))
    );
    setup.harness.finish.add_permits(1);
    assert_eq!(
        blocked(&mut socket).await,
        (HarnessSessionStatus::Idle, Some(false))
    );
    setup.stop.cancel();
}
