use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use gateway::routes::v1::{BoundAgent, Service};
use kernel::agent::TurnRunner;
use kernel::model::ModelChunk;
use parking_lot::Mutex;
use testkit::{FakeMemoryStore, FakeProvider, Script};
use tokio_util::sync::CancellationToken;
use wire::gateway::v1::{Completion, Conversations, ErrorKind, ErrorResponse, Models};
use wire::message::Role;

use super::runner::FakeRunner;
use super::support::{Harness, REQUEST, bytes, json, request, settings};

const PROBE: &str = "script missing Done";

type LogLine = (tracing::Level, String);
type LogLines = Arc<Mutex<Vec<LogLine>>>;

struct Pair {
    service: Arc<Service>,
    gateway: Arc<gateway::Gateway>,
    alpha: Arc<FakeRunner>,
    beta: Arc<FakeRunner>,
    _dir: tempfile::TempDir,
}

impl Pair {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("scratch directory");
        let alpha = scripted("alpha-says");
        let beta = scripted("beta-says");
        let mut roster = BTreeMap::new();
        roster.insert("alpha".to_owned(), bound(&alpha, "alpha-model"));
        roster.insert("beta".to_owned(), bound(&beta, "beta-model"));
        let service = Service::open(
            "alpha-model".to_owned(),
            &dir.path().join("conversations"),
            CancellationToken::new(),
            roster,
        )
        .await
        .expect("service opens");
        let gateway = Arc::new(gateway::Gateway::new(
            Arc::new(FakeMemoryStore::default()),
            settings(dir.path()),
        ));
        Self {
            service,
            gateway,
            alpha,
            beta,
            _dir: dir,
        }
    }

    fn app(&self) -> axum::Router {
        self.service.router(&self.gateway, false)
    }

    async fn close(self) {
        tokio::time::timeout(Duration::from_secs(5), self.service.shutdown())
            .await
            .expect("service joins");
    }
}

fn scripted(text: &str) -> Arc<FakeRunner> {
    Arc::new(FakeRunner::new(Arc::new(FakeProvider::new(vec![
        Script::Stream(vec![
            ModelChunk::Text {
                delta: text.to_owned(),
            },
            ModelChunk::Done,
        ]),
    ]))))
}

fn bound(runner: &Arc<FakeRunner>, model: &str) -> BoundAgent {
    let cloned = Arc::clone(runner);
    let shared: Arc<dyn TurnRunner> = cloned;
    BoundAgent {
        runner: shared,
        model: model.to_owned(),
    }
}

fn body(model: &str) -> String {
    format!(r#"{{"model":"{model}","messages":[{{"role":"user","content":"Hi"}}]}}"#)
}

async fn completion(pair: &Pair, model: &str) -> Completion {
    let response = request(
        pair.app(),
        "POST",
        "/v1/chat/completions",
        &body(model),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "{model}");
    json(response).await
}

fn assistant(completion: &Completion) -> &str {
    completion
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .expect("assistant text")
}

fn assert_routed(runner: &FakeRunner, model: &str, agent: &str) {
    let turns = std::mem::take(&mut *runner.turns.lock());
    let [turn] = turns.as_slice() else {
        panic!("one turn");
    };
    assert_eq!(turn.request.model, model);
    assert_eq!(turn.context.agent_id.as_deref(), Some(agent));
    assert_eq!(turn.tool_context.agent_id.as_deref(), Some(agent));
    assert_eq!(
        turn.request.messages.first().map(|message| message.role),
        Some(Role::User)
    );
}

#[tokio::test]
async fn two_agents_reach_their_own_providers_and_are_listed() {
    let pair = Pair::new().await;
    let alpha = completion(&pair, "alpha").await;
    assert_eq!(alpha.model, "alpha");
    assert_eq!(assistant(&alpha), "alpha-says");
    assert_routed(&pair.alpha, "alpha-model", "alpha");
    assert!(pair.beta.turns.lock().is_empty());
    let beta = completion(&pair, "beta").await;
    assert_eq!(beta.model, "beta");
    assert_eq!(assistant(&beta), "beta-says");
    assert_routed(&pair.beta, "beta-model", "beta");
    assert!(pair.alpha.turns.lock().is_empty());
    let models: Models = json(request(pair.app(), "GET", "/v1/models", "", &[]).await).await;
    assert_eq!(
        models
            .data
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    assert!(
        models
            .data
            .iter()
            .all(|model| model.created == 0 && model.owned_by == "rivetos")
    );
    pair.close().await;
}

#[tokio::test]
async fn unknown_model_names_the_registered_agents_and_stores_nothing() {
    let pair = Pair::new().await;
    for model in ["nope", "alpha-model"] {
        let response = request(
            pair.app(),
            "POST",
            "/v1/chat/completions",
            &body(model),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{model}");
        let error: ErrorResponse = json(response).await;
        assert_eq!(error.error.kind, ErrorKind::InvalidRequestError);
        assert_eq!(
            error.error.message,
            format!("Unknown agent: \"{model}\". Registered: [alpha,beta]")
        );
    }
    let records: Conversations =
        json(request(pair.app(), "GET", "/api/conversations", "", &[]).await).await;
    assert_eq!(records.conversations, Vec::new());
    assert!(pair.alpha.turns.lock().is_empty());
    assert!(pair.beta.turns.lock().is_empty());
    pair.close().await;
}

struct Capture {
    events: LogLines,
}

struct LineVisitor<'a>(&'a mut String);

impl tracing::field::Visit for LineVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        self.0.push_str(field.name());
        self.0.push('=');
        self.0.push_str(&rendered);
        self.0.push(';');
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push_str(field.name());
        self.0.push('=');
        self.0.push_str(value);
        self.0.push(';');
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_non_zero_u64(std::num::NonZeroU64::MIN)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut LineVisitor(&mut line));
        self.events.lock().push((*event.metadata().level(), line));
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

fn install() -> (tracing::subscriber::DefaultGuard, LogLines) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(Capture {
        events: Arc::clone(&events),
    });
    (guard, events)
}

fn assert_logged(events: &LogLines) {
    let recorded = events.lock().clone();
    let errors: Vec<_> = recorded
        .iter()
        .filter(|(level, _)| *level == tracing::Level::ERROR)
        .collect();
    let [(_, line)] = errors.as_slice() else {
        panic!("one error: {recorded:?}");
    };
    assert!(line.contains(PROBE), "{line}");
    assert!(
        recorded
            .iter()
            .all(|(level, _)| *level != tracing::Level::INFO),
        "{recorded:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_completion_logs_the_underlying_error_once() {
    let harness = Harness::new(vec![ModelChunk::Text {
        delta: "partial".to_owned(),
    }])
    .await;
    let (_guard, events) = install();
    let response = request(harness.app(), "POST", "/v1/chat/completions", REQUEST, &[]).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let raw = String::from_utf8(bytes(response).await.to_vec()).expect("UTF-8");
    let error: ErrorResponse = serde_json::from_str(&raw).expect("typed error");
    assert_eq!(error.error.message, "agent turn failed");
    assert!(!raw.contains(PROBE));
    assert_logged(&events);
    harness.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_stream_logs_the_underlying_error_once() {
    let harness = Harness::new(vec![ModelChunk::Text {
        delta: "partial".to_owned(),
    }])
    .await;
    let (_guard, events) = install();
    let response = request(
        harness.app(),
        "POST",
        "/v1/chat/completions",
        r#"{"model":"test-model","messages":[{"role":"user","content":"Hi"}],"stream":true}"#,
        &[],
    )
    .await;
    let raw = String::from_utf8(bytes(response).await.to_vec()).expect("UTF-8");
    assert!(raw.contains("agent turn failed"), "{raw}");
    assert!(!raw.contains(PROBE), "{raw}");
    assert!(!raw.contains("[DONE]"), "{raw}");
    assert_logged(&events);
    harness.close().await;
}
