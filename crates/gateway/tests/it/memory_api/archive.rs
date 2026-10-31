use super::*;
use chrono::DateTime;
use chrono::Utc;
use kernel::memory::MemoryExport;
use kernel::memory::MemoryMessageTally;
use kernel::memory::MemoryStoreError;
use kernel::memory::MemorySummaryNode;
use kernel::memory::SummaryDag;
use kernel::memory::SummaryKind;
use kernel::memory::SummaryOwner;
use kernel::memory::VectorState;
use tokio_util::sync::CancellationToken;
use wire::gateway::memory::MemorySummaryDagResponse;

const DUMP: &[u8] = &[0x1f, 0x8b, 0x08, 0x00, 0x2d, 0x2d];

#[derive(Default)]
struct RecordingExport {
    calls: parking_lot::Mutex<Vec<Option<DateTime<Utc>>>>,
}

#[async_trait::async_trait]
impl MemoryExport for RecordingExport {
    async fn export(
        &self,
        since: Option<DateTime<Utc>>,
        _cancellation: CancellationToken,
    ) -> Result<Vec<u8>, MemoryStoreError> {
        self.calls.lock().push(since);
        Ok(DUMP.to_vec())
    }
}

#[expect(
    clippy::expect_used,
    reason = "an in-process request that cannot be answered must fail the test"
)]
fn get(app: &axum::Router, path: &str) -> testkit::Response {
    block_on(answer(app.clone(), recorded("GET", path, Vec::new()))).expect("in-process answer")
}

fn header<'a>(response: &'a testkit::Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[expect(
    clippy::expect_used,
    reason = "the test literals are constants; a malformed literal must fail the test"
)]
fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("RFC 3339 literal")
        .with_timezone(&Utc)
}

fn node(
    id: &str,
    parent: Option<&str>,
    level: u32,
    kind: SummaryKind,
    vector: VectorState,
) -> MemorySummaryNode {
    MemorySummaryNode {
        id: id.to_owned(),
        parent_id: parent.map(str::to_owned),
        level,
        kind,
        content: format!("{kind} summary"),
        message_count: 2,
        earliest_at: Some(at("2026-10-10T12:00:00Z")),
        latest_at: Some(at("2026-10-10T12:05:00Z")),
        created_at: at("2026-10-10T12:10:00Z"),
        source_message_ids: vec!["00000000-0000-4000-8000-000000000101".to_owned()],
        vector,
    }
}

fn exporting() -> (Arc<RecordingExport>, axum::Router) {
    let export = Arc::new(RecordingExport::default());
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let gateway = Arc::new(
        Gateway::new(store, common::settings())
            .with_memory_export(Arc::clone(&export) as Arc<dyn MemoryExport>),
    );
    (export, gateway.router(false))
}

#[test]
fn summaries_route_returns_the_session_dag_root_first() {
    let (gateway, store) = common::gateway();
    let branch = "00000000-0000-4000-8000-000000000001";
    let dag = SummaryDag {
        session_key: Some(SESSION.to_owned()),
        summaries: vec![
            node(branch, None, 1, SummaryKind::Branch, VectorState::Embedded),
            node(
                "00000000-0000-4000-8000-000000000002",
                Some(branch),
                0,
                SummaryKind::Leaf,
                VectorState::Pending,
            ),
        ],
        messages: MemoryMessageTally {
            total: 3,
            user: 1,
            assistant: 1,
            system: 0,
            tool: 1,
            embedded: 2,
            pending: 1,
            failed: 0,
            skipped: 0,
        },
    };
    block_on(store.script_summary_dag(SummaryOwner::Session(SESSION.to_owned()), dag.clone()));
    let app = gateway.router(false);
    let response = get(&app, &format!("/api/memory/summaries/{SESSION}"));
    assert_eq!(response.status, 200);
    let parsed: MemorySummaryDagResponse =
        serde_json::from_slice(&response.body).expect("summary dag response");
    assert_eq!(parsed.session_id, SESSION);
    assert_eq!(
        serde_json::to_string(&parsed.summaries).expect("summaries"),
        serde_json::to_string(&dag.summaries).expect("summaries")
    );
    assert_eq!(
        serde_json::to_string(&parsed.messages).expect("messages"),
        serde_json::to_string(&dag.messages).expect("messages")
    );
    let requests = block_on(store.requests());
    assert!(
        requests.contains(&testkit::MemoryRequest::SummaryDag(SummaryOwner::Session(
            SESSION.to_owned()
        )))
    );

    let response = get(&app, "/api/memory/summaries/unknown-session");
    assert_eq!(response.status, 200);
    let parsed: MemorySummaryDagResponse =
        serde_json::from_slice(&response.body).expect("empty summary dag response");
    assert_eq!(parsed.session_id, "unknown-session");
    assert_eq!(parsed.summaries.len(), 0);
    assert_eq!(
        parsed.messages,
        wire::gateway::memory::MemoryMessageTally::default()
    );
}

#[test]
fn a_conversation_answers_its_summary_dag_under_its_own_key() {
    let (gateway, store) = common::gateway();
    let conversation = uuid::uuid!("00000000-0000-4000-8000-000000000061");
    let dag = SummaryDag {
        session_key: Some("discord:00000000-0000-4000-8000-000000000062".to_owned()),
        summaries: vec![node(
            "00000000-0000-4000-8000-000000000063",
            None,
            0,
            SummaryKind::Leaf,
            VectorState::Embedded,
        )],
        messages: MemoryMessageTally::default(),
    };
    block_on(store.script_summary_dag(SummaryOwner::Conversation(conversation), dag.clone()));
    let app = gateway.router(false);
    let response = get(
        &app,
        &format!("/api/memory/conversations/{conversation}/summaries"),
    );
    assert_eq!(response.status, 200);
    let parsed: MemorySummaryDagResponse =
        serde_json::from_slice(&response.body).expect("conversation summary dag");
    assert_eq!(
        parsed.session_id,
        "discord:00000000-0000-4000-8000-000000000062"
    );
    assert_eq!(
        serde_json::to_string(&parsed.summaries).expect("summaries"),
        serde_json::to_string(&dag.summaries).expect("summaries")
    );
    let unknown = get(
        &app,
        "/api/memory/conversations/00000000-0000-4000-8000-000000000064/summaries",
    );
    assert_eq!(unknown.status, 404);
    assert_eq!(
        get(&app, "/api/memory/conversations/not-an-id/summaries").status,
        400
    );
}

#[test]
fn summaries_route_reports_an_unavailable_store() {
    let app = common::failing_gateway().router(false);
    let response = get(&app, &format!("/api/memory/summaries/{SESSION}"));
    assert_eq!(response.status, 503);
    let response = block_on(answer(
        app,
        recorded("POST", "/api/memory/summaries/s-1", Vec::new()),
    ))
    .expect("in-process answer");
    assert_eq!(response.status, 405);
}

#[test]
fn export_route_downloads_the_gzip_dump_since_a_timestamp() {
    let (export, app) = exporting();
    let response = get(&app, "/api/memory/export?since=2026-10-01T00:00:00Z");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, DUMP);
    assert_eq!(header(&response, "content-type"), Some("application/gzip"));
    let disposition = header(&response, "content-disposition").expect("content-disposition");
    assert!(
        disposition.starts_with("attachment; filename=\"memory-"),
        "{disposition}"
    );
    assert!(disposition.ends_with(".sql.gz\""), "{disposition}");

    let response = get(&app, "/api/memory/export");
    assert_eq!(response.status, 200);
    assert_eq!(
        *export.calls.lock(),
        vec![Some(at("2026-10-01T00:00:00Z")), None]
    );

    let response = get(&app, "/api/memory/export?since=yesterday");
    assert_eq!(response.status, 400);
    assert_eq!(export.calls.lock().len(), 2);
}

#[test]
fn export_route_is_guarded_and_needs_a_configured_exporter() {
    let (export, app) = exporting();
    let mut remote = recorded("GET", "/api/memory/export", Vec::new());
    remote.via = Some("remote".to_owned());
    let response = block_on(answer(app.clone(), remote)).expect("in-process answer");
    assert_eq!(response.status, 401);
    let response = block_on(answer(
        app,
        recorded("POST", "/api/memory/export", Vec::new()),
    ))
    .expect("in-process answer");
    assert_eq!(response.status, 405);
    assert_eq!(export.calls.lock().len(), 0);

    let (gateway, _store) = common::gateway();
    let response = get(&gateway.router(false), "/api/memory/export");
    assert_eq!(response.status, 503);
    assert_eq!(error_of(&response).error, "memory export is not configured");
}
