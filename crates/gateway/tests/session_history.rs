#![cfg(test)]
#[expect(
    dead_code,
    reason = "integration binaries share fixtures and use different subsets"
)]
mod session_support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt as _, StreamExt as _};
use gateway::sessions::{SessionRegistry, owners_file};
use kernel::transcript::{
    ListLimit, SessionLister, StoreSession, StoreTranscript, TranscriptRead, TranscriptReaderError,
};
use parking_lot::Mutex;
use session_support::harness::Harness;
use session_support::{Call, Fixture, Socket, WAIT, frame, http, user};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use wire::events::ContextSource;
use wire::gateway::GatewayError;
use wire::gateway::sessions::{
    HarnessSessionResponse, HarnessSessionTranscriptResponse, HarnessSocketRefusal,
    HarnessSocketRefusalCode, HarnessTranscriptTurn, MessageUsage, SessionMessageRole,
    SessionPostRequest, TranscriptAttachment,
};
use wire::harness::{
    HarnessEvent, HarnessId, HarnessOfferingList, HarnessSessionList, HarnessSessionStatus,
    HarnessStoreSessionList,
};
use wire::session_id::{SessionId, encode_segment};

const U1: &str = "3f2c9a10-1b2c-4d5e-8f90-a1b2c3d4e5f6";
const U2: &str = "00000000-0000-4000-8000-000000000002";
const U3: &str = "00000000-0000-4000-8000-000000000003";
const ALICE: Option<&str> = Some("alice-test-token");

#[derive(Default)]
struct Stores {
    rows: Vec<StoreSession>,
    transcripts: BTreeMap<(HarnessId, String), Vec<HarnessTranscriptTurn>>,
    cwd: BTreeMap<String, String>,
    limits: Mutex<Vec<usize>>,
    later: Mutex<BTreeMap<String, Vec<HarnessTranscriptTurn>>>,
}

#[async_trait]
impl SessionLister for Stores {
    async fn list(&self, harnesses: &[HarnessId], limit: ListLimit) -> Vec<StoreSession> {
        self.limits.lock().push(limit.get());
        let mut rows: Vec<StoreSession> = self
            .rows
            .iter()
            .filter(|row| harnesses.contains(&row.harness))
            .cloned()
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
        rows.truncate(limit.get());
        rows
    }

    async fn describe(
        &self,
        harness: HarnessId,
        native: &str,
        _parent: Option<&str>,
    ) -> Result<Option<StoreSession>, TranscriptReaderError> {
        Ok(self
            .rows
            .iter()
            .find(|row| row.harness == harness && row.id == native)
            .cloned())
    }

    async fn store_cwd(
        &self,
        _harness: HarnessId,
        native: &str,
    ) -> Result<Option<String>, TranscriptReaderError> {
        Ok(self.cwd.get(native).cloned())
    }

    async fn transcript(
        &self,
        harness: HarnessId,
        native: &str,
    ) -> Result<TranscriptRead, TranscriptReaderError> {
        if harness == HarnessId::Cowork {
            return Ok(TranscriptRead::NoSource);
        }
        Ok(TranscriptRead::Read(StoreTranscript {
            turns: self
                .transcripts
                .get(&(harness, native.to_owned()))
                .cloned()
                .or_else(|| self.later.lock().get(native).cloned())
                .unwrap_or_default(),
            truncated: false,
        }))
    }
}

fn row(harness: HarnessId, id: &str, updated_at: i64) -> StoreSession {
    StoreSession::new(harness, id, updated_at)
}

fn titled(mut row: StoreSession, title: &str) -> StoreSession {
    title.clone_into(&mut row.title);
    row
}

fn child(mut row: StoreSession, parent: &str) -> StoreSession {
    row.parent_session_id = Some(parent.to_owned());
    row
}

fn turn(role: SessionMessageRole, text: &str) -> HarnessTranscriptTurn {
    HarnessTranscriptTurn {
        role,
        text: text.to_owned(),
        thinking: None,
        tools: None,
        usage: None,
        model: None,
        stop_reason: None,
        last_block: None,
        attachments: None,
        complete: false,
        compact: false,
    }
}

fn answered(text: &str, model: &str, prompt_tokens: u64) -> HarnessTranscriptTurn {
    HarnessTranscriptTurn {
        model: Some(model.to_owned()),
        usage: Some(MessageUsage {
            prompt_tokens,
            completion_tokens: 1,
            cached_tokens: 0,
        }),
        ..turn(SessionMessageRole::Assistant, text)
    }
}

struct Setup {
    fixture: Fixture,
    harness: Arc<Harness>,
    lister: Arc<Stores>,
    _state: tempfile::TempDir,
}

async fn setup(owners: &str, stores: Stores) -> Setup {
    setup_with(
        owners,
        stores,
        &[HarnessId::ClaudeCode, HarnessId::Codex, HarnessId::Hermes],
    )
    .await
}

async fn setup_with(owners: &str, stores: Stores, offered: &[HarnessId]) -> Setup {
    let state = tempfile::tempdir().expect("state");
    let path = owners_file(state.path());
    std::fs::create_dir_all(path.parent().expect("owners directory")).expect("owners directory");
    std::fs::write(&path, owners).expect("owners");
    let registry = SessionRegistry::new(Duration::from_secs(1))
        .with_owners_file(path)
        .with_node_owner(user("owner"));
    let harness = Arc::new(Harness::offering(offered));
    let lister = Arc::new(stores);
    let fixture = Fixture::with_lister(
        Some(harness.clone()),
        registry,
        Some(lister.clone()),
        &[("hermes", HarnessId::Hermes)],
    )
    .await;
    Setup {
        fixture,
        harness,
        lister,
        _state: state,
    }
}

async fn get(setup: &Setup, path: &str, token: Option<&str>) -> (u16, Vec<u8>) {
    http(&setup.fixture.gateway, "GET", path, Vec::new(), token).await
}

fn parsed<T: serde::de::DeserializeOwned>(body: &[u8]) -> T {
    serde_json::from_slice(body)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(body)))
}

fn error(body: &[u8]) -> GatewayError {
    parsed(body)
}

fn enc(session: &str) -> String {
    encode_segment(&session.parse::<SessionId>().expect("session id"))
}

fn claude(native: &str) -> SessionId {
    SessionId::new(HarnessId::ClaudeCode, native)
}

fn listed_ids(body: &[u8]) -> Vec<String> {
    parsed::<HarnessStoreSessionList>(body)
        .sessions
        .into_iter()
        .map(|row| row.id)
        .collect()
}

#[tokio::test]
async fn b1_offered_harnesses_list_sessions_from_disk() {
    let setup = setup("{}", Stores::default()).await;
    let (status, body) = get(&setup, "/api/harnesses", None).await;
    assert_eq!(status, 200);
    let listed: HarnessOfferingList = parsed(&body);
    assert_eq!(listed.harnesses.len(), 3);
    assert!(
        listed
            .harnesses
            .iter()
            .all(|offering| offering.capabilities.observation.list_sessions)
    );
    assert!(
        listed
            .harnesses
            .iter()
            .all(|offering| offering.capabilities.control.resume)
    );
}

#[tokio::test]
async fn b2_1_the_legacy_list_merges_stores_newest_first() {
    let stores = Stores {
        rows: vec![
            row(HarnessId::ClaudeCode, U1, 1_760_700_000_000),
            row(HarnessId::ClaudeCode, U2, 1_760_600_000_000),
            row(HarnessId::Hermes, "h1", 1_760_650_000_000),
            row(HarnessId::Pi, U3, 1_760_800_000_000),
        ],
        ..Stores::default()
    };
    let setup = setup("{}", stores).await;
    let (status, body) = get(&setup, "/api/terminal/harness-sessions", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(listed_ids(&body), [U1, "h1", U2]);
    let (status, alias) = get(&setup, "/term/harness-sessions", None).await;
    assert_eq!(status, 200);
    assert_eq!(alias, body);
}

#[tokio::test]
async fn b2_2_and_b2_3_the_limit_defaults_and_caps() {
    let setup = setup("{}", Stores::default()).await;
    for query in ["?limit=0", "?limit=9999", "?limit=20abc&limit=3", ""] {
        let (status, _) = get(
            &setup,
            &format!("/api/terminal/harness-sessions{query}"),
            None,
        )
        .await;
        assert_eq!(status, 200);
    }
    assert_eq!(*setup.lister.limits.lock(), [100, 500, 20, 100]);
}

#[tokio::test]
async fn b2_4_a_user_without_owned_sessions_sees_nothing() {
    let stores = Stores {
        rows: vec![row(HarnessId::ClaudeCode, U1, 1)],
        ..Stores::default()
    };
    let setup = setup("{}", stores).await;
    let (status, body) = get(&setup, "/api/terminal/harness-sessions", ALICE).await;
    assert_eq!(status, 200);
    assert_eq!(body, br#"{"sessions":[]}"#);
}

#[tokio::test]
async fn b2_5_legacy_rows_carry_native_ids_and_epoch_millis() {
    let stores = Stores {
        rows: vec![titled(
            row(HarnessId::ClaudeCode, U1, 1_760_700_000_000),
            "fix it",
        )],
        ..Stores::default()
    };
    let setup = setup("{}", stores).await;
    let (_, body) = get(&setup, "/api/terminal/harness-sessions", None).await;
    assert_eq!(
        String::from_utf8_lossy(&body),
        format!(
            r#"{{"sessions":[{{"id":"{U1}","command":"claude","title":"fix it","updatedAt":1760700000000}}]}}"#
        )
    );
}

#[tokio::test]
async fn listed_children_inherit_their_parent_owner() {
    let stores = Stores {
        rows: vec![
            row(HarnessId::ClaudeCode, U1, 2),
            child(row(HarnessId::ClaudeCode, "agent-a1", 1), U1),
        ],
        ..Stores::default()
    };
    let setup = setup(&format!(r#"{{"{U1}":"alice"}}"#), stores).await;
    let (_, body) = get(&setup, "/api/terminal/harness-sessions", ALICE).await;
    assert_eq!(listed_ids(&body), [U1, "agent-a1"]);
    let (_, body) = get(&setup, "/api/terminal/harness-sessions", None).await;
    assert_eq!(listed_ids(&body), Vec::<String>::new());
    assert_eq!(
        setup.fixture.registry.owner("agent-a1").expect("key"),
        Some(user("alice"))
    );
}

#[tokio::test]
async fn b3_1_plane_rows_are_summaries_with_millisecond_iso_times() {
    let stores = Stores {
        rows: vec![row(HarnessId::ClaudeCode, U1, 1_760_700_000_000)],
        cwd: BTreeMap::from([(U1.to_owned(), "/home/user/proj".to_owned())]),
        ..Stores::default()
    };
    let setup = setup("{}", stores).await;
    let (status, body) = get(&setup, "/api/harnesses/claude-code/sessions", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        String::from_utf8_lossy(&body),
        format!(
            r#"{{"sessions":[{{"sessionId":"claude-code:{U1}","harnessId":"claude-code","cwd":"/home/user/proj","createdAt":"2025-10-17T11:20:00.000Z","updatedAt":"2025-10-17T11:20:00.000Z","status":"ended"}}]}}"#
        )
    );
    assert_eq!(*setup.lister.limits.lock(), [100]);
}

#[tokio::test]
async fn b3_2_a_title_equal_to_the_id_is_omitted_and_the_roster_cwd_applies() {
    let stores = Stores {
        rows: vec![
            row(HarnessId::ClaudeCode, U1, 2),
            titled(row(HarnessId::ClaudeCode, U2, 1), "named"),
        ],
        ..Stores::default()
    };
    let setup = setup("{}", stores).await;
    let (_, body) = get(&setup, "/api/harnesses/claude-code/sessions?limit=5", None).await;
    let sessions = parsed::<HarnessSessionList>(&body).sessions;
    assert_eq!(sessions.len(), 2);
    assert_eq!(nth(&sessions, 0).title, None);
    assert_eq!(nth(&sessions, 1).title.as_deref(), Some("named"));
    let roster = setup.fixture.directory.path().canonicalize().expect("root");
    assert_eq!(nth(&sessions, 0).cwd.as_deref(), roster.to_str());
    assert_eq!(*setup.lister.limits.lock(), [100]);
}

#[tokio::test]
async fn b3_3_unknown_and_unregistered_harnesses_are_not_found() {
    let setup = setup("{}", Stores::default()).await;
    let (status, body) = get(&setup, "/api/harnesses/claude/sessions", None).await;
    assert_eq!(status, 404);
    assert_eq!(error(&body).error, "unknown harness: claude");
    let (status, body) = get(&setup, "/api/harnesses/pi/sessions", None).await;
    assert_eq!(status, 404);
    assert_eq!(error(&body).error, "no driver registered for pi");
}

#[tokio::test]
async fn b3_4_a_live_chat_without_a_file_is_listed_idle() {
    let setup = setup("{}", Stores::default()).await;
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        "/api/harness-sessions",
        r#"{"harness":"claude-code"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let (_, body) = get(&setup, "/api/harnesses/claude-code/sessions", None).await;
    let sessions = parsed::<HarnessSessionList>(&body).sessions;
    assert_eq!(sessions.len(), 1);
    assert_eq!(nth(&sessions, 0).status, HarnessSessionStatus::Idle);
    let (_, body) = get(&setup, "/api/harnesses/claude-code/sessions", ALICE).await;
    assert_eq!(parsed::<HarnessSessionList>(&body).sessions, Vec::new());
}

#[tokio::test]
async fn b3_5_a_subagent_row_links_its_canonical_parent() {
    let stores = Stores {
        rows: vec![
            row(HarnessId::ClaudeCode, U1, 2),
            child(row(HarnessId::ClaudeCode, "agent-a1", 1), U1),
        ],
        ..Stores::default()
    };
    let setup = setup("{}", stores).await;
    let (_, body) = get(&setup, "/api/harnesses/claude-code/sessions", None).await;
    let sessions = parsed::<HarnessSessionList>(&body).sessions;
    assert_eq!(nth(&sessions, 1).parent_session_id, Some(claude(U1)));
}

fn one_claude_row() -> Stores {
    Stores {
        rows: vec![titled(
            row(HarnessId::ClaudeCode, U1, 1_760_700_000_000),
            "fix it",
        )],
        ..Stores::default()
    }
}

#[tokio::test]
async fn b4_1_and_t0_9_a_bare_uuid_resolves_to_its_store_and_redirects() {
    let setup = setup_with(
        "{}",
        one_claude_row(),
        &[HarnessId::Codex, HarnessId::ClaudeCode],
    )
    .await;
    for _ in 0..2 {
        let (status, body) = get(&setup, &format!("/api/harness-sessions/{U1}"), None).await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let response: HarnessSessionResponse = parsed(&body);
        assert_eq!(response.summary.session_id, claude(U1));
        assert_eq!(response.redirected_to, Some(claude(U1)));
        assert_eq!(response.summary.title.as_deref(), Some("fix it"));
    }
    let (status, body) = get(&setup, &format!("/api/harness-sessions/{U2}"), None).await;
    assert_eq!(status, 400);
    let refused = error(&body);
    assert_eq!(refused.code.as_deref(), Some("invalid_session_id"));
    assert_eq!(refused.retryable, Some(false));
    assert_eq!(
        refused.error,
        format!("no registered harness owns bare session id {U2}")
    );
}

#[tokio::test]
async fn t0_6_a_path_fallback_id_redirects_to_the_collapsed_id() {
    let setup = setup("{}", one_claude_row()).await;
    let segment = enc(&format!("claude-code:-home-user/{U1}"));
    let (status, body) = get(&setup, &format!("/api/harness-sessions/{segment}"), None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let response: HarnessSessionResponse = parsed(&body);
    assert_eq!(response.redirected_to, Some(claude(U1)));
    let canonical = enc(&format!("claude-code:{U1}"));
    let (_, body) = get(&setup, &format!("/api/harness-sessions/{canonical}"), None).await;
    assert!(!String::from_utf8_lossy(&body).contains("redirectedTo"));
}

#[tokio::test]
async fn b4_2_an_unknown_session_without_a_live_chat_is_not_found() {
    let setup = setup("{}", one_claude_row()).await;
    let segment = enc(&format!("claude-code:{U2}"));
    let (status, body) = get(&setup, &format!("/api/harness-sessions/{segment}"), None).await;
    assert_eq!(status, 404);
    assert_eq!(
        error(&body).error,
        format!("unknown session claude-code:{U2}")
    );
}

#[tokio::test]
async fn b4_3_another_users_session_is_forbidden() {
    let setup = setup(
        &format!(r#"{{"claude-code:{U1}":"owner"}}"#),
        one_claude_row(),
    )
    .await;
    let segment = enc(&format!("claude-code:{U1}"));
    let (status, body) = get(&setup, &format!("/api/harness-sessions/{segment}"), ALICE).await;
    assert_eq!(status, 403);
    assert_eq!(error(&body).error, "session is owned by another user");
    let (status, _) = get(
        &setup,
        &format!("/api/harness-sessions/{segment}/transcript"),
        ALICE,
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn segments_that_do_not_decode_or_name_no_driver_are_bad_requests() {
    let setup = setup("{}", one_claude_row()).await;
    for segment in ["abc=", "IGNsYXVkZS1jb2RlOng", "task%3A123"] {
        let (status, body) = get(&setup, &format!("/api/harness-sessions/{segment}"), None).await;
        assert_eq!(status, 400, "{segment}");
        assert_eq!(error(&body).code.as_deref(), Some("invalid_session_id"));
    }
    let segment = enc(&format!("grok-build:{U1}"));
    let (status, body) = get(&setup, &format!("/api/harness-sessions/{segment}"), None).await;
    assert_eq!(status, 400);
    assert_eq!(error(&body).error, "no driver registered for grok-build");
    let segment = enc(&format!("claude-code:{U1}"));
    let (status, body) = http(
        &setup.fixture.gateway,
        "DELETE",
        &format!("/api/harness-sessions/{segment}"),
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, 405);
    assert_eq!(error(&body).error, "method not allowed");
}

#[tokio::test]
async fn b5_the_transcript_is_store_scoped_and_carries_the_context_stamp() {
    let stores = Stores {
        rows: vec![
            row(HarnessId::ClaudeCode, U1, 1),
            row(HarnessId::Hermes, "h1", 1),
        ],
        transcripts: BTreeMap::from([
            (
                (HarnessId::ClaudeCode, U1.to_owned()),
                vec![
                    turn(SessionMessageRole::User, "hi"),
                    answered("hello", "claude-opus-4-1[1m]", 10),
                ],
            ),
            (
                (HarnessId::ClaudeCode, "h1".to_owned()),
                vec![turn(SessionMessageRole::User, "claude, not hermes")],
            ),
        ]),
        ..Stores::default()
    };
    let setup = setup("{}", stores).await;
    let segment = enc(&format!("claude-code:{U1}"));
    let (status, body) = get(
        &setup,
        &format!("/api/harness-sessions/{segment}/transcript"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let read: HarnessSessionTranscriptResponse = parsed(&body);
    assert_eq!(read.session_id, claude(U1));
    assert_eq!(read.turns.len(), 2);
    assert_eq!(read.context_window, 1_000_000);
    assert_eq!(read.compact_at, 965_000);
    assert_eq!(read.context_source, ContextSource::Default);
    let (status, body) = get(
        &setup,
        &format!("/api/harness-sessions/{}/transcript", enc("hermes:h1")),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let hermes: HarnessSessionTranscriptResponse = parsed(&body);
    assert_eq!(hermes.turns, Vec::new());
    assert_eq!(hermes.context_window, 262_144);
}

#[tokio::test]
async fn observed_usage_promotes_the_window_and_learns_the_compact_point() {
    let stores = Stores {
        rows: vec![row(HarnessId::Codex, U1, 1)],
        transcripts: BTreeMap::from([(
            (HarnessId::Codex, U1.to_owned()),
            vec![
                answered("a", "gpt-5", 300_000),
                answered("b", "gpt-5", 420_000),
                answered("c", "gpt-5", 20_000),
            ],
        )]),
        ..Stores::default()
    };
    let setup = setup("{}", stores).await;
    let (_, body) = get(
        &setup,
        &format!(
            "/api/harness-sessions/{}/transcript",
            enc(&format!("codex:{U1}"))
        ),
        None,
    )
    .await;
    let read: HarnessSessionTranscriptResponse = parsed(&body);
    assert_eq!(read.context_window, 1_000_000);
    assert_eq!(read.compact_at, 420_000);
    assert_eq!(read.context_source, ContextSource::Observed);
}

#[tokio::test]
async fn a_harness_without_a_transcript_reader_is_unsupported() {
    let stores = Stores {
        rows: vec![row(HarnessId::Cowork, "c1", 1)],
        ..Stores::default()
    };
    let setup = setup_with("{}", stores, &[HarnessId::Cowork]).await;
    let (status, body) = get(
        &setup,
        &format!("/api/harness-sessions/{}/transcript", enc("cowork:c1")),
        None,
    )
    .await;
    assert_eq!(status, 501);
    let refused = error(&body);
    assert_eq!(refused.code.as_deref(), Some("capability_unsupported"));
    assert_eq!(refused.error, "cowork has no transcript source");
}

async fn socket(
    setup: &Setup,
    segment: &str,
    token: Option<&str>,
) -> (Socket, tokio_util::sync::CancellationToken) {
    let (address, stop, _server) = setup.fixture.serve().await;
    let mut request = format!("ws://{address}/api/harness-sessions/ws?session={segment}")
        .into_client_request()
        .expect("request");
    if let Some(token) = token {
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("header"),
        );
    }
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect");
    (socket, stop)
}

async fn refusal(socket: &mut Socket) -> HarnessSocketRefusalCode {
    let HarnessSocketRefusal::Error { code, .. } =
        serde_json::from_str(&frame(socket).await.into_text().expect("refusal")).expect("refusal");
    assert!(matches!(frame(socket).await, Message::Close(_)));
    code
}

async fn event(socket: &mut Socket) -> HarnessEvent {
    serde_json::from_str(&frame(socket).await.into_text().expect("event")).expect("event")
}

async fn silent(socket: &mut Socket) {
    assert!(
        tokio::time::timeout(Duration::from_millis(300), socket.next())
            .await
            .is_err(),
        "no frame expected"
    );
}

#[tokio::test]
async fn b10_1_a_bad_segment_is_refused_with_one_error_frame() {
    let setup = setup("{}", Stores::default()).await;
    let (mut socket, stop) = socket(&setup, "abc=", None).await;
    assert_eq!(
        refusal(&mut socket).await,
        HarnessSocketRefusalCode::InvalidSessionId
    );
    stop.cancel();
}

#[tokio::test]
async fn b10_2_another_users_session_is_refused_as_forbidden() {
    let setup = setup(&format!(r#"{{"{U1}":"owner"}}"#), one_claude_row()).await;
    let (mut socket, stop) = socket(&setup, &enc(&format!("claude-code:{U1}")), ALICE).await;
    assert_eq!(
        refusal(&mut socket).await,
        HarnessSocketRefusalCode::Forbidden
    );
    stop.cancel();
}

#[tokio::test]
async fn an_empty_session_query_refuses_the_upgrade() {
    let setup = setup("{}", Stores::default()).await;
    let (address, stop, _server) = setup.fixture.serve().await;
    let refused = tokio_tungstenite::connect_async(format!(
        "ws://{address}/api/harness-sessions/ws?session="
    ))
    .await;
    assert!(refused.is_err());
    stop.cancel();
}

#[tokio::test]
async fn b10_3_and_b10_4_a_cold_session_answers_one_sync_per_window() {
    let mut stores = one_claude_row();
    stores.transcripts.insert(
        (HarnessId::ClaudeCode, U1.to_owned()),
        vec![
            turn(SessionMessageRole::User, "hi"),
            turn(SessionMessageRole::Assistant, "hello"),
        ],
    );
    let setup = setup("{}", stores).await;
    let (mut socket, stop) = socket(&setup, U1, None).await;
    silent(&mut socket).await;
    for _ in 0..2 {
        socket
            .send(Message::text(r#"{"type":"sync"}"#))
            .await
            .expect("sync");
    }
    let HarnessEvent::Transcript {
        session_id,
        rev,
        from,
        total,
        turns,
        ..
    } = event(&mut socket).await
    else {
        panic!("transcript snapshot expected");
    };
    assert_eq!(
        (session_id, rev, from, total, turns.len()),
        (claude(U1), 1, 0, 2, 2)
    );
    silent(&mut socket).await;
    stop.cancel();
}

fn kind(event: &HarnessEvent) -> String {
    if let HarnessEvent::SessionUpdated { status, .. } = event {
        format!("updated:{status:?}")
    } else if let HarnessEvent::AssistantDelta { text, .. } = event {
        format!("delta:{text}")
    } else if matches!(event, HarnessEvent::TurnComplete { .. }) {
        "complete".to_owned()
    } else if let HarnessEvent::Transcript { from, .. } = event {
        format!("transcript:{from}")
    } else {
        format!("{event:?}")
    }
}

fn nth<T>(items: &[T], index: usize) -> &T {
    items.get(index).expect("listed item")
}

#[tokio::test]
async fn a_live_turn_streams_deltas_then_a_committed_snapshot() {
    let setup = setup("{}", Stores::default()).await;
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        "/api/harness-sessions",
        r#"{"harness":"claude-code"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let created: wire::harness::HarnessSessionSummary = parsed(&body);
    let session = created.session_id.clone();
    let (mut socket, stop) = socket(&setup, &encode_segment(&session), None).await;
    silent(&mut socket).await;
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{session}/turn"),
        r#"{"text":"hi"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    setup.harness.traced("prompt:hi").await;
    setup.harness.finish.add_permits(1);
    let mut kinds = Vec::new();
    tokio::time::timeout(WAIT, async {
        loop {
            let next = event(&mut socket).await;
            let done = matches!(
                next,
                HarnessEvent::SessionUpdated {
                    status: HarnessSessionStatus::Idle,
                    ..
                }
            );
            kinds.push(kind(&next));
            if done {
                return;
            }
        }
    })
    .await
    .expect("turn events");
    assert_eq!(
        kinds,
        [
            "updated:Active",
            "delta:answer",
            "complete",
            "transcript:0",
            "updated:Idle"
        ]
    );
    let (_, body) = get(&setup, "/api/harnesses/claude-code/sessions", None).await;
    assert_eq!(
        nth(&parsed::<HarnessSessionList>(&body).sessions, 0).status,
        HarnessSessionStatus::Idle
    );
    stop.cancel();
}

async fn created_chat(setup: &Setup) -> SessionId {
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        "/api/harness-sessions",
        r#"{"harness":"claude-code"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    parsed::<wire::harness::HarnessSessionSummary>(&body).session_id
}

#[tokio::test]
async fn a_turn_on_a_session_this_runtime_did_not_start_resumes_it() {
    let setup = setup("{}", one_claude_row()).await;
    let session = claude(U1);
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{session}/turn"),
        r#"{"text":"hi"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    setup.harness.traced("resume").await;
    setup.harness.traced("prompt:hi").await;
    setup.harness.finish.add_permits(1);
    setup.harness.traced("ended").await;
}

#[tokio::test]
async fn another_users_session_is_not_resumed() {
    let setup = setup(
        &format!(r#"{{"claude-code:{U1}":"owner"}}"#),
        one_claude_row(),
    )
    .await;
    let (status, _) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{}/turn", claude(U1)),
        r#"{"text":"hi"}"#,
        ALICE,
    )
    .await;
    assert_eq!(status, 403);
    assert!(setup.harness.trace.borrow().is_empty());
}

#[tokio::test]
async fn a_system_prompt_rides_ahead_of_the_turn_text() {
    let setup = setup("{}", Stores::default()).await;
    let session = created_chat(&setup).await;
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{session}/turn"),
        r#"{"text":"hi","systemPrompt":"  be brief "}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    setup
        .harness
        .traced("prompt:[System instructions]\nbe brief\n\nhi")
        .await;
    setup.harness.finish.add_permits(1);
    setup.harness.traced("ended").await;
}

async fn uploaded(setup: &Setup, name: &str, media_type: &str, token: Option<&str>) -> String {
    staged(setup, name, media_type, token).await.uri
}

async fn staged(
    setup: &Setup,
    name: &str,
    media_type: &str,
    token: Option<&str>,
) -> wire::gateway::uploads::UploadCreated {
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/uploads?name={name}&mediaType={media_type}"),
        b"pixels".to_vec(),
        token,
    )
    .await;
    assert_eq!(status, 201, "{}", String::from_utf8_lossy(&body));
    parsed(&body)
}

fn attached_turn(path: &str) -> String {
    serde_json::to_string(&SessionPostRequest {
        exclude_tools: None,
        text: "look".to_owned(),
        user_id: None,
        agent: None,
        thinking: None,
        system_prompt: None,
        attachments: Some(vec![wire::gateway::agents::UserTurnAttachment {
            mime: "image/png".to_owned(),
            path_or_uri: path.to_owned(),
            name: Some("shot.png".to_owned()),
        }]),
        harness: None,
        model: None,
        working_dir: None,
    })
    .expect("turn json")
}

#[tokio::test]
async fn an_uploaded_attachment_reaches_the_harness_with_the_turn() {
    let setup = setup("{}", Stores::default()).await;
    let session = created_chat(&setup).await;
    let uri = uploaded(&setup, "shot.png", "image/png", None).await;
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{session}/turn"),
        attached_turn(&uri),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    setup
        .harness
        .traced(&format!("prompt:look|attached:shot.png:image/png:{uri}"))
        .await;
    setup.harness.finish.add_permits(1);
    setup.harness.traced("ended").await;
}

fn image_turn(text: &str) -> HarnessTranscriptTurn {
    HarnessTranscriptTurn {
        attachments: Some(vec![TranscriptAttachment {
            media_type: "image/png".to_owned(),
            name: None,
            uri: None,
            upload_id: None,
        }]),
        ..turn(SessionMessageRole::User, text)
    }
}

async fn transcript_turns(setup: &Setup, session: &str) -> Vec<HarnessTranscriptTurn> {
    let (status, body) = get(
        setup,
        &format!("/api/harness-sessions/{}/transcript", enc(session)),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    parsed::<HarnessSessionTranscriptResponse>(&body).turns
}

#[tokio::test]
async fn a_reloaded_transcript_names_the_uploads_its_user_turn_carried() {
    let mut stores = one_claude_row();
    stores.transcripts.insert(
        (HarnessId::ClaudeCode, U1.to_owned()),
        vec![
            turn(SessionMessageRole::User, "look"),
            answered("hello", "claude-opus-4-1", 10),
            image_turn("look"),
        ],
    );
    let setup = setup("{}", stores).await;
    let shot = staged(&setup, "shot.png", "image/png", None).await;
    let session = claude(U1).to_string();
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{session}/turn"),
        attached_turn(&shot.uri),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    setup.harness.finish.add_permits(1);
    setup.harness.traced("ended").await;
    let named = vec![TranscriptAttachment {
        media_type: "image/png".to_owned(),
        name: Some("shot.png".to_owned()),
        uri: Some(shot.uri.clone()),
        upload_id: Some(shot.id.clone()),
    }];
    let turns = transcript_turns(&setup, &session).await;
    assert_eq!(
        turns.first().and_then(|turn| turn.attachments.clone()),
        None
    );
    assert_eq!(
        turns.get(2).and_then(|turn| turn.attachments.clone()),
        Some(named.clone())
    );
    let (status, _) = http(
        &setup.fixture.gateway,
        "DELETE",
        &format!("/api/uploads/{}", shot.id),
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let turns = transcript_turns(&setup, &session).await;
    assert_eq!(
        turns.get(2).and_then(|turn| turn.attachments.clone()),
        Some(named)
    );
}

fn delegated_attached_turn(path: &str) -> Vec<u8> {
    let mut turn: SessionPostRequest =
        serde_json::from_str(&attached_turn(path)).expect("turn json");
    turn.harness = Some(HarnessId::ClaudeCode);
    serde_json::to_vec(&turn).expect("turn json")
}

#[tokio::test]
async fn a_delegated_turn_names_the_uploads_it_carried_when_reloaded() {
    let setup = setup("{}", Stores::default()).await;
    let shot = staged(&setup, "shot.png", "image/png", None).await;
    setup.harness.finish.add_permits(1);
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        "/api/sessions/task:abc/messages?wait=1",
        delegated_attached_turn(&shot.uri),
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let session = setup
        .harness
        .specs
        .lock()
        .first()
        .map(|spec| spec.session.clone())
        .expect("one harness session");
    let native = session
        .as_str()
        .split_once(':')
        .map(|(_, native)| native.to_owned())
        .expect("native id");
    setup.lister.later.lock().insert(
        native,
        vec![
            image_turn("look"),
            answered("answer", "claude-opus-4-1", 10),
        ],
    );
    let turns = transcript_turns(&setup, session.as_str()).await;
    assert_eq!(
        turns.first().and_then(|turn| turn.attachments.clone()),
        Some(vec![TranscriptAttachment {
            media_type: "image/png".to_owned(),
            name: Some("shot.png".to_owned()),
            uri: Some(shot.uri.clone()),
            upload_id: Some(shot.id.clone()),
        }])
    );
}

#[tokio::test]
async fn an_attachment_that_is_not_the_callers_upload_is_refused() {
    let setup = setup("{}", Stores::default()).await;
    let session = created_chat(&setup).await;
    let theirs = uploaded(&setup, "theirs.png", "image/png", Some("alice-test-token")).await;
    for path in ["/etc/passwd", theirs.as_str()] {
        let (status, body) = http(
            &setup.fixture.gateway,
            "POST",
            &format!("/api/harness-sessions/{session}/turn"),
            attached_turn(path),
            None,
        )
        .await;
        assert_eq!(status, 404, "{path}: {}", String::from_utf8_lossy(&body));
    }
    assert!(
        !setup
            .harness
            .trace
            .borrow()
            .iter()
            .any(|line| line.starts_with("prompt:"))
    );
}

#[tokio::test]
async fn an_approval_answer_reaches_the_harness_and_is_broadcast() {
    let setup = setup("{}", Stores::default()).await;
    let session = created_chat(&setup).await;
    let segment = encode_segment(&session);
    let (mut socket, stop) = socket(&setup, &segment, None).await;
    silent(&mut socket).await;
    let (status, body) = http(
        &setup.fixture.gateway,
        "POST",
        &format!("/api/harness-sessions/{segment}/approvals/req-1"),
        r#"{"decision":"allow-session"}"#,
        None,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    setup.harness.traced("answer:req-1:AllowSession").await;
    let HarnessEvent::ApprovalResolved {
        session_id,
        request_id,
        decision,
    } = event(&mut socket).await
    else {
        panic!("approval-resolved expected");
    };
    assert_eq!(
        (session_id, request_id.as_str(), decision),
        (
            session,
            "req-1",
            wire::harness::ApprovalOutcome::AllowSession
        )
    );
    stop.cancel();
}

#[tokio::test]
async fn a_fresh_terminal_resumes_a_session_its_store_already_holds() {
    let stores = Stores {
        rows: vec![row(HarnessId::ClaudeCode, U1, 1)],
        ..Stores::default()
    };
    let setup = setup_with("{}", stores, &[HarnessId::ClaudeCode]).await;
    for session in [U1, U2] {
        let request = wire::gateway::term::TermSpawnRequest {
            session: Some(session.into()),
            command: Some("claude".into()),
            model: None,
            effort: None,
            resume: None,
            agent_id: None,
            force: None,
            cols: None,
            rows: None,
        };
        setup
            .fixture
            .service
            .spawn(&user("owner"), request)
            .await
            .expect("spawn");
    }
    let spawned: Vec<Vec<String>> = setup
        .fixture
        .mux
        .calls
        .borrow()
        .iter()
        .filter_map(|call| match call {
            Call::Spawn { argv, .. } => Some(argv.clone()),
            Call::Input(_) | Call::Resize(_) | Call::Kill => None,
        })
        .collect();
    assert_eq!(
        spawned,
        [
            vec!["claude".to_owned(), "--resume".to_owned(), U1.to_owned()],
            vec![
                "claude".to_owned(),
                "--session-id".to_owned(),
                U2.to_owned()
            ],
        ]
    );
    setup.fixture.gateway.close().await;
}

#[test]
fn harness_codes_map_to_their_statuses() {
    use strum::IntoEnumIterator as _;
    for code in wire::harness::HarnessCode::iter() {
        assert_eq!(gateway::error::harness_status(code).as_u16(), code.status());
    }
}
