#![cfg(test)]
mod session_skips;
#[expect(
    dead_code,
    reason = "integration binaries share fixtures and use different subsets"
)]
mod session_support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use futures::SinkExt as _;
use serde::de::DeserializeOwned;
use session_support::{Fixture, WAIT, frame, http};
use testkit::ws::{FrameRecord, StepRecord};
use tokio_tungstenite::tungstenite::Message;
use wire::events::SessionBroadcast;
use wire::gateway::term::{TermConfig, TermServerFrame, TermSpawnResponse};

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../wire/tests/golden")
        .join(name)
}

fn recording<T: DeserializeOwned>(name: &str) -> T {
    serde_json::from_slice(&std::fs::read(golden(name)).expect("recording"))
        .expect("typed recording")
}

const HTTP_CLAIMS: &[&str] = &[
    "term-main/config",
    "term-main/config-alias",
    "term-main/config-remote",
    "term-main/post-shell-session",
    "term-main/post-spawn-or-get",
    "term-main/post-spawn-or-get-other-command",
    "term-main/post-string",
    "term-main/post-null",
    "term-main/post-number",
    "term-main/delete-unknown",
    "term-main/post-room-session",
    "term-main/post-clamp-high",
    "term-main/post-claude-session-uuid",
    "term-main/post-claude-session-not-uuid",
    "term-main/post-alias",
    "term-main/post-invalid-json",
    "term-main/post-unknown-command",
    "term-main/post-model-space",
    "term-main/post-model-dotdot",
    "term-main/post-effort-empty",
    "term-main/delete-no-id-early",
    "term-tenancy/tenancy-post-session-alice",
    "term-tenancy/tenancy-post-resume-alice",
    "term-tenancy/tenancy-inject-alice",
    "term-tenancy/tenancy-delete-alice",
    "term-tenancy/tenancy-delete-session-alice",
];
const EVENT_CLAIMS: &[&str] = &[
    "ws-main/connect-a-empty-snapshot",
    "ws-main/post-start-alpha",
    "ws-main/connect-b-filtered-alpha",
    "ws-main/post-start-beta",
    "ws-main/post-batch-alpha",
    "ws-main/delete-alpha",
    "ws-main/post-start-uuid",
    "ws-main/delete-canonical",
    "ws-main/end-beta-then-evict",
];
const TERM_CLAIMS: &[&str] = &[
    "term-ws/spawn-main",
    "term-ws/attach-a",
    "term-ws/attach-b-session",
    "term-ws/type-a",
    "term-ws/resize-a-claims",
    "term-ws/resize-b-recorded",
    "term-ws/claim-b",
];

#[test]
fn every_recorded_scenario_is_replayed_or_explicitly_excluded() {
    let http = testkit::http::cases(&golden("term/http.json")).expect("http cases");
    let gateway = testkit::ws::steps(&golden("gateway/ws.json")).expect("gateway steps");
    let terminal = testkit::ws::steps(&golden("term/ws.json")).expect("terminal steps");
    for (ids, claims, skips) in [
        (
            http.into_iter().map(|case| case.id).collect::<Vec<_>>(),
            HTTP_CLAIMS,
            session_skips::HTTP,
        ),
        (
            gateway.into_iter().map(|step| step.id).collect(),
            EVENT_CLAIMS,
            session_skips::EVENTS,
        ),
        (
            terminal.into_iter().map(|step| step.id).collect(),
            TERM_CLAIMS,
            session_skips::TERM,
        ),
    ] {
        let mut accounted = BTreeMap::new();
        for id in claims {
            assert!(
                accounted.insert(*id, "replayed").is_none(),
                "duplicate {id}"
            );
        }
        for (id, reason) in skips {
            assert_ne!(reason.trim(), "");
            assert!(accounted.insert(*id, reason).is_none(), "duplicate {id}");
        }
        assert_eq!(ids.len(), accounted.len());
        for id in ids {
            assert!(accounted.contains_key(id.as_str()), "unaccounted: {id}");
        }
    }
}

fn substitute_spawn(text: &str, response: &TermSpawnResponse) -> String {
    let pty = regex::Regex::new(r"\{\{PTY_\d+\}\}").expect("mask pattern");
    let pid = regex::Regex::new(r"\{\{PID_\d+\}\}").expect("mask pattern");
    pid.replace_all(&pty.replace_all(text, response.id.as_str()), "42")
        .replace(
            "{{EPOCH_MS}}",
            &response.created_at.timestamp_millis().to_string(),
        )
}

#[tokio::test]
async fn terminal_http_recordings() {
    for case in testkit::http::cases(&golden("term/http.json")).expect("http cases") {
        if !HTTP_CLAIMS.contains(&case.id.as_str()) {
            continue;
        }
        let fixture = Fixture::new().await;
        fixture
            .registry
            .claim(&session_support::user("alice"), "alice-room")
            .expect("owner seed");
        fixture
            .registry
            .claim(
                &session_support::user("alice"),
                "00000000-0000-4000-8000-000000000002",
            )
            .expect("resume owner seed");
        if matches!(
            case.id.as_str(),
            "term-main/post-spawn-or-get" | "term-main/post-spawn-or-get-other-command"
        ) {
            let (status, _) = http(
                &fixture.gateway,
                "POST",
                "/term",
                r#"{"command":"shell","session":"acme-shell-1"}"#,
                None,
            )
            .await;
            assert_eq!(status, 201);
        }
        let request = testkit::http::parse_head(&case.request.head.replace("{{PORT}}", "1234"))
            .expect("request head");
        let body = match &case.request.body {
            Some(body) => body.clone(),
            None => String::new(),
        };
        let token = if case.id.ends_with("config-remote") {
            Some("unknown")
        } else {
            None
        };
        let (status, body) = http(
            &fixture.gateway,
            &request.method,
            &request.target,
            body,
            token,
        )
        .await;
        assert_eq!(
            status,
            testkit::http::status_of(&case.response.status_line),
            "{}",
            case.id
        );
        let recorded = case.response.body.as_deref().expect("response body");
        if status >= 400 {
            let actual: wire::gateway::GatewayError =
                serde_json::from_slice(&body).expect("error response");
            let expected: wire::gateway::GatewayError =
                serde_json::from_str(recorded).expect("recorded error");
            assert_eq!(actual, expected, "{}", case.id);
        } else if request.target.ends_with("config") {
            let actual: TermConfig = serde_json::from_slice(&body).expect("config response");
            assert_eq!(
                actual,
                serde_json::from_str::<TermConfig>(recorded).expect("recorded config"),
                "{}",
                case.id
            );
        } else {
            let actual: TermSpawnResponse = serde_json::from_slice(&body).expect("spawn response");
            let expected =
                serde_json::from_str::<TermSpawnResponse>(&substitute_spawn(recorded, &actual))
                    .expect("recorded spawn");
            assert_eq!(actual, expected, "{}", case.id);
        }
        fixture.gateway.close().await;
    }
}

async fn compare_event_frames(
    step: &StepRecord,
    sockets: &mut BTreeMap<String, session_support::Socket>,
) {
    for (client, frames) in &step.frames {
        let Some(socket) = sockets.get_mut(client) else {
            continue;
        };
        for expected in frames {
            let FrameRecord::Text { text } = expected else {
                panic!("event text expected")
            };
            let actual = frame(socket).await.into_text().expect("text");
            assert_eq!(
                serde_json::from_str::<SessionBroadcast>(&actual).expect("event"),
                serde_json::from_str::<SessionBroadcast>(text).expect("recorded event"),
                "{} {client}",
                step.id
            );
        }
    }
}

#[tokio::test]
async fn gateway_ws_recordings() {
    let fixture = Fixture::new().await;
    let (address, stop, server) = fixture.serve().await;
    let mut sockets = BTreeMap::new();
    let recording: testkit::ws::Recording = recording("gateway/ws.json");
    let run = recording.runs.get("ws-main").expect("main run");
    for step in &run.steps {
        if !EVENT_CLAIMS.contains(&format!("ws-main/{}", step.id).as_str()) {
            continue;
        }
        if let Some(connect) = &step.connect {
            let request = testkit::http::parse_head(&connect.request.head).expect("head");
            let (socket, _) =
                tokio_tungstenite::connect_async(format!("ws://{address}{}", request.target))
                    .await
                    .expect("connect");
            sockets.insert(connect.client.clone(), socket);
        }
        if let Some(request) = &step.http {
            let head = testkit::http::parse_head(&request.request.head).expect("head");
            let body = match &request.request.body {
                Some(body) => body.clone(),
                None => String::new(),
            };
            let (status, _) = http(&fixture.gateway, &head.method, &head.target, body, None).await;
            assert_eq!(status, 200, "{}", step.id);
        }
        compare_event_frames(step, &mut sockets).await;
    }
    stop.cancel();
    for socket in sockets.values_mut() {
        assert!(matches!(frame(socket).await, Message::Close(_)));
    }
    drop(sockets);
    tokio::time::timeout(WAIT, server)
        .await
        .expect("shutdown timeout")
        .expect("server task")
        .expect("server");
}

async fn compare_terminal_frame(socket: &mut session_support::Socket, expected: &FrameRecord) {
    match expected {
        FrameRecord::Text { text } => {
            let actual = frame(socket).await.into_text().expect("text frame");
            let actual: TermServerFrame = serde_json::from_str(&actual).expect("terminal frame");
            let mut text = text.replace("{{PTY_1}}", "pty-1");
            if let TermServerFrame::Owner {
                since: Some(since), ..
            } = &actual
            {
                text = text.replace("{{EPOCH_MS}}", &since.timestamp_millis().to_string());
            }
            assert_eq!(
                actual,
                serde_json::from_str::<TermServerFrame>(&text).expect("recorded terminal frame")
            );
        }
        FrameRecord::Binary {
            text: Some(text), ..
        } => assert_eq!(frame(socket).await.into_data().as_ref(), text.as_bytes()),
        FrameRecord::Binary { text: None, .. }
        | FrameRecord::Close { .. }
        | FrameRecord::TcpClosed => panic!("unexpected recording frame"),
    }
}

#[tokio::test]
async fn terminal_ws_recordings() {
    let fixture = Fixture::new().await;
    let (address, stop, server) = fixture.serve().await;
    let recording: testkit::ws::Recording = recording("term/ws.json");
    let run = recording.runs.get("term-ws").expect("run");
    let mut sockets: BTreeMap<String, session_support::Socket> = BTreeMap::new();
    for step in &run.steps {
        if !TERM_CLAIMS.contains(&format!("term-ws/{}", step.id).as_str()) {
            continue;
        }
        if let Some(request) = &step.http {
            let head = testkit::http::parse_head(&request.request.head).expect("head");
            assert_eq!(
                http(
                    &fixture.gateway,
                    &head.method,
                    &head.target,
                    request.request.body.clone().expect("body"),
                    None
                )
                .await
                .0,
                201
            );
        }
        if let Some(connect) = &step.connect {
            let head =
                testkit::http::parse_head(&connect.request.head.replace("{{PTY_1}}", "pty-1"))
                    .expect("head");
            let (socket, _) =
                tokio_tungstenite::connect_async(format!("ws://{address}{}", head.target))
                    .await
                    .expect("connect");
            sockets.insert(connect.client.clone(), socket);
        }
        for sent in &step.sent {
            let message = match &sent.payload {
                testkit::ws::SentPayload::Text { text } => Message::text(text),
                testkit::ws::SentPayload::Binary { hex } => {
                    Message::binary(hex::decode(hex).expect("hex input"))
                }
            };
            sockets
                .get_mut(&sent.client)
                .expect("client")
                .send(message)
                .await
                .expect("send");
        }
        if !step.sent.is_empty() {
            terminal_output(step, &fixture, &mut sockets).await;
        }
        for (client, frames) in &step.frames {
            for expected in frames {
                if step.sent.is_empty() || !matches!(expected, FrameRecord::Text { .. }) {
                    compare_terminal_frame(sockets.get_mut(client).expect("client"), expected)
                        .await;
                }
            }
        }
    }
    drop(sockets);
    stop.cancel();
    tokio::time::timeout(WAIT, server)
        .await
        .expect("shutdown timeout")
        .expect("server task")
        .expect("server");
}

async fn terminal_output(
    step: &StepRecord,
    fixture: &Fixture,
    sockets: &mut BTreeMap<String, session_support::Socket>,
) {
    for (client, frames) in &step.frames {
        for expected in frames {
            if matches!(expected, FrameRecord::Text { .. }) {
                compare_terminal_frame(sockets.get_mut(client).expect("client"), expected).await;
            }
        }
    }
    if step.id == "type-a" {
        fixture
            .mux
            .called(|calls| calls.contains(&session_support::Call::Input(b"hello\r".to_vec())))
            .await;
    }
    let live = step
        .frames
        .values()
        .flatten()
        .find_map(|frame| match frame {
            FrameRecord::Binary {
                text: Some(text), ..
            } => Some(text),
            FrameRecord::Text { .. }
            | FrameRecord::Binary { text: None, .. }
            | FrameRecord::Close { .. }
            | FrameRecord::TcpClosed => None,
        });
    if let Some(live) = live {
        fixture.mux.emit(
            &kernel::terminal::TerminalId::new("pty-1"),
            live.as_bytes().to_vec(),
        );
    }
}
