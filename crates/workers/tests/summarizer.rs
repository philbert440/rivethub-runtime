#![cfg(test)]

#[expect(
    unreachable_pub,
    reason = "the harness is a private module of each test binary"
)]
#[expect(
    dead_code,
    reason = "each test binary uses a different subset of the harness"
)]
mod common;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use common::Phase;
use common::Queue;
use kernel::memory::ConversationHead;
use kernel::memory::ConversationWindow;
use kernel::memory::NewSummary;
use kernel::memory::SummaryKind;
use kernel::memory::SummaryNode;
use kernel::memory::SummaryStore;
use kernel::memory::SummaryStoreError;
use kernel::memory::WindowMessage;
use kernel::model::Embedding;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use kernel::model::ModelRequest;
use kernel::model::ModelStreamItem;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::message::MessageContent;
use wire::message::Role;
use wire::stream::LlmResponse;
use wire::stream::LlmResponseKind;
use wire::stream::openai::FinishReason;
use workers::BRANCH_SYSTEM_PROMPT;
use workers::LEAF_SYSTEM_PROMPT;
use workers::ROOT_SYSTEM_PROMPT;
use workers::SummarizeEnv;
use workers::SummarizeOutcome;
use workers::SummaryHealth;
use workers::SummaryParts;
use workers::SummarySettings;
use workers::SummaryTrigger;
use workers::handle_enqueue_idle;
use workers::handle_enqueue_stale;
use workers::handle_reap;
use workers::handle_summarize;
use workers::summary_health;

const LEAF_TEXT: &str = "The team decided to deploy the AcmeKit service with a blue-green rollout. The release branch is cut on Mondays, the deploy script runs from the release branch, and a health check on staging.example.com gates the traffic switch. Rollback is a traffic switch back to the previous colour.";
const BRANCH_TEXT: &str = "Branch summary: several deploy sessions for AcmeKit settled on blue-green releases with a health-check gate.";
const ROOT_TEXT: &str = "Root summary: the whole AcmeKit deploy conversation, from the first release plan to the documented rollback.";

const REPLAYED: &[&str] = &[
    "leaf once a full window exists",
    "leaves → branches → root with small batches",
    "truncated answer shrinks the leaf batch",
    "failing LLM fails the job; heartbeat never summarized",
    "permanent failure → dead → rests an hour → revived",
    "below the floor until idle/stale",
];

const SKIPPED: &[(&str, &str)] = &[
    (
        "synthesize-tool-call",
        "no recording of this job; the summarizer leaves it unhandled",
    ),
    (
        "WIKI_EXTRACT_SYSTEM_PROMPT",
        "wiki extraction belongs to the wiki builder",
    ),
    (
        "WIKI_RECOMPILE_SYSTEM_PROMPT",
        "wiki recompile belongs to the wiki builder",
    ),
    (
        "TAG_SYSTEM_PROMPT",
        "tag suggestion belongs to the categorizer",
    ),
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptHashes {
    #[serde(rename = "LEAF_SYSTEM_PROMPT")]
    leaf: String,
    #[serde(rename = "BRANCH_SYSTEM_PROMPT")]
    branch: String,
    #[serde(rename = "ROOT_SYSTEM_PROMPT")]
    root: String,
    #[serde(rename = "WIKI_EXTRACT_SYSTEM_PROMPT")]
    wiki_extract: String,
    #[serde(rename = "WIKI_RECOMPILE_SYSTEM_PROMPT")]
    wiki_recompile: String,
    #[serde(rename = "TAG_SYSTEM_PROMPT")]
    tag: String,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    prompts_sha256: PromptHashes,
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    steps: Vec<Step>,
    #[serde(default)]
    chat_calls: Option<ChatCalls>,
    #[serde(default)]
    tables: Tables,
    #[serde(default)]
    logs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ChatCalls {
    Calls(Vec<ChatCall>),
    Count(u64),
}

#[derive(Debug, Clone, Deserialize)]
struct ChatCall {
    prompt: String,
    user: String,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Step {
    label: String,
    result: StepResult,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StepResult {
    Count(u64),
    Health(HealthSnap),
    Rows(Vec<wire::Extra>),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct HealthSnap {
    eligible: u64,
    #[serde(rename = "activeTail")]
    active_tail: u64,
    #[serde(rename = "belowFloor")]
    below_floor: u64,
}

#[derive(Debug, Default, Deserialize)]
struct Tables {
    #[serde(default, rename = "ros_summaries")]
    summaries: Vec<SummaryRow>,
    #[serde(default, rename = "ros_jobs")]
    jobs: Vec<JobRow>,
}

#[derive(Debug, Deserialize)]
struct SummaryRow {
    conversation_id: Uuid,
    parent_id: Option<Uuid>,
    depth: i32,
    content: String,
    kind: String,
    message_count: u32,
    earliest_at: DateTime<Utc>,
    latest_at: DateTime<Utc>,
    model: String,
    pipeline_version: i32,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct JobRow {
    task: String,
    attempts: u32,
    max_attempts: u32,
    state: String,
    last_error: String,
}

#[derive(Debug)]
struct Captured {
    kind: String,
    user: String,
    max_tokens: Option<u32>,
    model: String,
}

struct Scripted {
    failures: Mutex<Vec<u16>>,
    truncations: Mutex<u32>,
    calls: Mutex<Vec<Captured>>,
}

struct MemStore {
    windows: Mutex<Vec<ConversationWindow>>,
    written: Mutex<Vec<NewSummary>>,
}

struct Seen {
    jobs: Mutex<Vec<Job>>,
    dead: Mutex<Vec<(JobName, DateTime<Utc>)>>,
}

fn fixture() -> Fixture {
    let path = testkit::recording("memory-sqlite/summarizer.json");
    let text = std::fs::read_to_string(path).expect("summarizer fixture");
    serde_json::from_str(&text).expect("summarizer json")
}

fn recorded_name(index: usize) -> &'static str {
    REPLAYED.get(index).copied().expect("recorded scenario")
}

fn scenario<'a>(fixture: &'a Fixture, name: &str) -> &'a Scenario {
    fixture
        .scenarios
        .iter()
        .find(|scenario| scenario.name == name)
        .expect("scenario")
}

fn chat_calls(scenario: &Scenario) -> Vec<ChatCall> {
    match &scenario.chat_calls {
        Some(ChatCalls::Calls(calls)) => calls.clone(),
        Some(ChatCalls::Count(_)) | None => Vec::new(),
    }
}

fn health_steps(scenario: &Scenario) -> Vec<HealthSnap> {
    scenario
        .steps
        .iter()
        .filter_map(|step| match &step.result {
            StepResult::Health(health) => Some(health.clone()),
            StepResult::Count(_) | StepResult::Rows(_) => None,
        })
        .collect()
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("timestamp")
        .with_timezone(&Utc)
}

fn settings() -> SummarySettings {
    SummarySettings::standard("fake-chat").expect("settings")
}

fn small_settings() -> SummarySettings {
    SummarySettings::from_parts(SummaryParts {
        model: "fake-chat".to_owned(),
        leaf_batch: 5,
        branch_batch: 2,
        root_batch: 5,
        min_leaves_for_branch: 2,
        min_branches_for_root: 2,
        idle_minutes: 15,
        stale_minutes: 5_760,
        stale_min_batch: 2,
        max_leaf_rounds: 10,
        pipeline_version: 5,
        session_tagging: false,
    })
    .expect("small settings")
}

fn messages(start: DateTime<Utc>, count: usize) -> Vec<WindowMessage> {
    (0..count)
        .map(|index| WindowMessage {
            id: Uuid::new_v4(),
            agent: "rivet".to_owned(),
            role: if index % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            content: format!("message number {index} about deploying the AcmeKit service"),
            created_at: start
                .checked_add_signed(Duration::minutes(i64::try_from(index).expect("index")))
                .expect("message time"),
        })
        .collect()
}

fn window(
    id: Uuid,
    session: &str,
    title: &str,
    messages: Vec<WindowMessage>,
) -> ConversationWindow {
    let latest_message_at = messages.last().map(|message| message.created_at);
    ConversationWindow {
        conversation: ConversationHead {
            id,
            session_key: session.to_owned(),
            agent: "rivet".to_owned(),
            channel: "cli".to_owned(),
            title: Some(title.to_owned()),
        },
        messages,
        leaves: Vec::new(),
        branches: Vec::new(),
        latest_message_at,
    }
}

fn cancel() -> CancellationToken {
    CancellationToken::new()
}

fn payload_of(id: Uuid, trigger: SummaryTrigger) -> wire::JsonObject {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        conversation_id: Uuid,
        trigger_type: SummaryTrigger,
    }
    wire::JsonObject::from_serialize(&Body {
        conversation_id: id,
        trigger_type: trigger,
    })
    .expect("payload")
}

fn read_trigger(payload: &wire::JsonObject) -> SummaryTrigger {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        trigger_type: SummaryTrigger,
    }
    let value = serde_json::to_value(payload).expect("payload value");
    serde_json::from_value::<Body>(value)
        .expect("trigger")
        .trigger_type
}

#[test]
fn every_recorded_scenario_is_replayed_and_other_prompts_are_named() {
    let fixture = fixture();
    assert_ne!(fixture.prompts_sha256.leaf, "");
    assert_ne!(fixture.prompts_sha256.branch, "");
    assert_ne!(fixture.prompts_sha256.root, "");
    assert_ne!(fixture.prompts_sha256.wiki_extract, "");
    assert_ne!(fixture.prompts_sha256.wiki_recompile, "");
    assert_ne!(fixture.prompts_sha256.tag, "");
    let mut names: Vec<&str> = fixture
        .scenarios
        .iter()
        .map(|scenario| scenario.name.as_str())
        .collect();
    let mut replayed = REPLAYED.to_vec();
    names.sort_unstable();
    replayed.sort_unstable();
    assert_eq!(names, replayed);
    for (name, reason) in SKIPPED {
        assert_ne!(*reason, "", "{name}");
        assert!(!REPLAYED.contains(name), "{name}");
    }
    assert!(
        fixture
            .scenarios
            .iter()
            .all(|scenario| !scenario.steps.is_empty())
    );
    let mut counts = 0u64;
    let mut rows = 0usize;
    for scenario in &fixture.scenarios {
        for step in &scenario.steps {
            match &step.result {
                StepResult::Count(count) => counts = counts.saturating_add(*count),
                StepResult::Health(_) => {}
                StepResult::Rows(recorded) => rows = rows.saturating_add(recorded.len()),
            }
        }
        if let Some(ChatCalls::Count(recorded)) = &scenario.chat_calls {
            counts = counts.saturating_add(*recorded);
        }
    }
    assert!(counts > 0);
    assert!(rows > 0);
}

#[tokio::test]
async fn a_full_window_summarizes_once() {
    let fixture = fixture();
    let recorded = scenario(&fixture, recorded_name(0));
    let id = recorded
        .tables
        .summaries
        .first()
        .expect("summary")
        .conversation_id;
    let store = Arc::new(MemStore::new(vec![window(
        id,
        "cli:s1",
        "Session cli:s1",
        messages(at("2026-10-10T12:00:00Z"), 10),
    )]));
    let script = Arc::new(Scripted::success());
    let jobs = Arc::new(Seen::default());
    let settings = settings();
    let token = cancel();
    let now = at("2026-10-10T12:12:00Z");
    let env = bind(&store, &script, &jobs, &settings, now, &token);
    assert_eq!(handle_enqueue_idle(&env).await.expect("sweep"), 1);
    let job = jobs.only().await;
    assert_eq!(job.key.as_deref(), Some(format!("compact-{id}").as_str()));
    assert_eq!(job.key_mode, JobKeyMode::Replace);
    assert_eq!(job.priority, 0);
    assert_eq!(read_trigger(&job.payload), SummaryTrigger::Window);
    let outcome = handle_summarize(&env, &job.payload).await.expect("compact");
    assert_eq!(
        outcome,
        SummarizeOutcome {
            summaries: 1,
            reenqueued: false
        }
    );
    let again = handle_summarize(&env, &job.payload).await.expect("second");
    assert_eq!(again.summaries, 0);
    assert_calls(&script.calls.lock().await, &chat_calls(recorded));
    assert_rows(&store.written.lock().await, &recorded.tables.summaries);
    assert_eq!(summarized_counts(&recorded.logs), vec![1]);
}

#[tokio::test]
async fn small_batches_roll_one_branch_then_the_root_on_the_next_job() {
    let fixture = fixture();
    let recorded = scenario(&fixture, recorded_name(1));
    let id = recorded
        .tables
        .summaries
        .first()
        .expect("summary")
        .conversation_id;
    let seeded = messages(at("2026-10-10T12:00:00Z"), 20);
    let store = Arc::new(MemStore::new(vec![window(
        id,
        "cli:s1",
        "Session cli:s1",
        seeded.clone(),
    )]));
    let script = Arc::new(Scripted::success());
    let jobs = Arc::new(Seen::default());
    let settings = small_settings();
    let token = cancel();
    let env = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-10T12:20:00Z"),
        &token,
    );
    let first_payload = payload_of(id, SummaryTrigger::Window);
    let first = handle_summarize(&env, &first_payload)
        .await
        .expect("pass 1");
    assert_eq!(
        first,
        SummarizeOutcome {
            summaries: 5,
            reenqueued: true
        }
    );
    let second_job = jobs.only().await;
    let second = handle_summarize(&env, &second_job.payload)
        .await
        .expect("pass 2");
    assert_eq!(
        second,
        SummarizeOutcome {
            summaries: 2,
            reenqueued: false
        }
    );
    assert_eq!(jobs.jobs.lock().await.len(), 0);
    assert_calls(&script.calls.lock().await, &chat_calls(recorded));
    assert_rows(&store.written.lock().await, &recorded.tables.summaries);
    assert_sources(&store.written.lock().await, &seeded);
    assert_eq!(summarized_counts(&recorded.logs), vec![5, 2]);
    assert_eq!(
        recorded
            .steps
            .iter()
            .filter(|step| step.label.starts_with("pass"))
            .count(),
        6
    );
}

#[tokio::test]
async fn a_truncated_leaf_shrinks_the_batch_and_sticks() {
    let fixture = fixture();
    let recorded = scenario(&fixture, recorded_name(2));
    let id = recorded
        .tables
        .summaries
        .first()
        .expect("summary")
        .conversation_id;
    let seeded = messages(at("2026-10-10T12:00:00Z"), 10);
    let store = Arc::new(MemStore::new(vec![window(
        id,
        "cli:s1",
        "Session cli:s1",
        seeded.clone(),
    )]));
    let script = Arc::new(Scripted::truncate_once());
    let jobs = Arc::new(Seen::default());
    let settings = settings();
    let token = cancel();
    let env = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-10T12:10:00Z"),
        &token,
    );
    let outcome = handle_summarize(&env, &payload_of(id, SummaryTrigger::Window))
        .await
        .expect("compact");
    assert_eq!(
        outcome,
        SummarizeOutcome {
            summaries: 2,
            reenqueued: false
        }
    );
    assert_calls(&script.calls.lock().await, &chat_calls(recorded));
    assert_rows(&store.written.lock().await, &recorded.tables.summaries);
    assert_sources(&store.written.lock().await, &seeded);
    assert_eq!(summarized_counts(&recorded.logs), vec![2]);
}

#[tokio::test]
async fn a_failing_model_fails_the_job_and_skips_heartbeats() {
    let fixture = fixture();
    let recorded = scenario(&fixture, recorded_name(3));
    assert!(matches!(recorded.chat_calls, Some(ChatCalls::Count(_))));
    let id = Uuid::parse_str("00000000-0000-4000-8000-000000000002").expect("id");
    let store = Arc::new(MemStore::new(vec![
        window(
            id,
            "cli:s1",
            "Session cli:s1",
            messages(at("2026-10-04T12:00:00Z"), 10),
        ),
        window(
            Uuid::new_v4(),
            "heartbeat:pulse",
            "heartbeat",
            messages(at("2026-10-04T12:00:00Z"), 10),
        ),
    ]));
    let script = Arc::new(Scripted::fail_with(vec![503]));
    let jobs = Arc::new(Seen::default());
    let settings = settings();
    let token = cancel();
    let env = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T12:22:00Z"),
        &token,
    );
    let error = handle_summarize(&env, &payload_of(id, SummaryTrigger::SessionIdle))
        .await
        .expect_err("503");
    assert_eq!(error.to_string(), "LLM HTTP 503");
    let job = recorded.tables.jobs.first().expect("job");
    assert_eq!(job.last_error, "LLM HTTP 503");
    assert_eq!(job.state, "queued");
    assert_eq!(job.attempts, 1);
    assert_eq!(job.max_attempts, 3);
    assert_eq!(job.task, "compact-conversation");
    assert_eq!(store.written.lock().await.len(), 0);
    assert_eq!(script.calls.lock().await.len(), 1);
    let heartbeat = store
        .windows
        .lock()
        .await
        .iter()
        .find(|window| window.conversation.session_key.starts_with("heartbeat:"))
        .expect("heartbeat")
        .conversation
        .id;
    let skipped = handle_summarize(&env, &payload_of(heartbeat, SummaryTrigger::Window))
        .await
        .expect("heartbeat");
    assert_eq!(skipped.summaries, 0);
    assert_eq!(script.calls.lock().await.len(), 1);
    assert_eq!(store.pending().await.expect("pending").len(), 1);

    let queue = Queue::new();
    let queued = queue
        .push(
            JobName::Fixed(FixedJob::CompactConversation),
            payload_of(id, SummaryTrigger::SessionIdle),
            3,
        )
        .await;
    let claimed = queue
        .claim(WorkerRole::Summarizer)
        .await
        .expect("claim")
        .expect("job");
    queue.fail(claimed.id, "LLM HTTP 503").await.expect("fail");
    let view = queue.view(queued).await;
    assert_eq!(view.phase, Phase::Backoff);
    assert_eq!(view.attempt, 1);
    assert_eq!(view.error.as_deref(), Some("LLM HTTP 503"));
}

#[tokio::test]
async fn a_permanent_status_still_retries_until_the_rest_revives_it() {
    let fixture = fixture();
    let recorded = scenario(&fixture, recorded_name(4));
    let row = recorded.tables.summaries.first().expect("summary");
    let store = Arc::new(MemStore::new(vec![window(
        row.conversation_id,
        "cli:s1",
        "Session cli:s1",
        messages(row.earliest_at, 10),
    )]));
    let script = Arc::new(Scripted::fail_with(vec![400, 400, 400]));
    let jobs = Arc::new(Seen::with_dead(vec![
        (
            JobName::Fixed(FixedJob::CompactConversation),
            at("2026-10-04T12:30:00Z"),
        ),
        (
            JobName::Fixed(FixedJob::EmbedTarget),
            at("2026-10-04T12:30:00Z"),
        ),
    ]));
    let settings = settings();
    let token = cancel();
    let payload = payload_of(row.conversation_id, SummaryTrigger::Window);
    for _ in 0..3 {
        let env = bind(
            &store,
            &script,
            &jobs,
            &settings,
            at("2026-10-04T12:10:00Z"),
            &token,
        );
        let error = handle_summarize(&env, &payload).await.expect_err("400");
        assert_eq!(error.to_string(), "LLM HTTP 400 (not retrying)");
    }
    assert!(
        recorded
            .logs
            .iter()
            .any(|line| line.contains("LLM HTTP 400 (not retrying)"))
    );
    assert_eq!(store.written.lock().await.len(), 0);
    let within = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T12:50:00Z"),
        &token,
    );
    assert_eq!(handle_reap(&within).await.expect("within rest"), 0);
    assert_eq!(jobs.dead_count(FixedJob::CompactConversation).await, 1);
    let after = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T13:51:00Z"),
        &token,
    );
    assert_eq!(handle_reap(&after).await.expect("after rest"), 1);
    assert_eq!(jobs.dead_count(FixedJob::CompactConversation).await, 0);
    assert_eq!(jobs.dead_count(FixedJob::EmbedTarget).await, 1);
    let outcome = handle_summarize(&after, &payload).await.expect("revived");
    assert_eq!(outcome.summaries, 1);
    assert_rows(&store.written.lock().await, &recorded.tables.summaries);
}

fn floor_windows(recorded: &Scenario) -> Vec<ConversationWindow> {
    let floor = recorded
        .tables
        .summaries
        .iter()
        .find(|row| row.message_count == 6)
        .expect("floor");
    let few = recorded
        .tables
        .summaries
        .iter()
        .find(|row| row.message_count == 3)
        .expect("few");
    vec![
        window(
            floor.conversation_id,
            "cli:floor",
            "Session cli:floor",
            messages(floor.earliest_at, 6),
        ),
        window(
            few.conversation_id,
            "cli:few",
            "Session cli:few",
            messages(few.earliest_at, 3),
        ),
    ]
}

#[tokio::test]
async fn idle_and_stale_floors_follow_the_recording() {
    let fixture = fixture();
    let recorded = scenario(&fixture, recorded_name(5));
    let store = Arc::new(MemStore::new(floor_windows(recorded)));
    let script = Arc::new(Scripted::success());
    let jobs = Arc::new(Seen::default());
    let settings = settings();
    let token = cancel();
    let early = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T12:09:00Z"),
        &token,
    );
    assert_eq!(handle_enqueue_idle(&early).await.expect("early idle"), 0);
    assert_eq!(handle_enqueue_stale(&early).await.expect("early stale"), 0);
    assert_health(
        &store,
        &settings,
        health_steps(recorded).first().expect("early health"),
    )
    .await;
    let idle = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-04T12:25:00Z"),
        &token,
    );
    assert_eq!(handle_enqueue_idle(&idle).await.expect("idle"), 1);
    let job = jobs.only().await;
    assert_eq!(read_trigger(&job.payload), SummaryTrigger::SessionIdle);
    let outcome = handle_summarize(&idle, &job.payload)
        .await
        .expect("idle compact");
    assert_eq!(outcome.summaries, 1);
    let stale = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-08T12:25:00Z"),
        &token,
    );
    assert_eq!(handle_enqueue_idle(&stale).await.expect("later idle"), 0);
    assert_eq!(handle_enqueue_stale(&stale).await.expect("stale"), 1);
    let job = jobs.only().await;
    assert_eq!(read_trigger(&job.payload), SummaryTrigger::Stale);
    let outcome = handle_summarize(&stale, &job.payload)
        .await
        .expect("stale compact");
    assert_eq!(outcome.summaries, 1);
    assert_calls(&script.calls.lock().await, &chat_calls(recorded));
    assert_rows(&store.written.lock().await, &recorded.tables.summaries);
    assert_health(
        &store,
        &settings,
        health_steps(recorded).get(1).expect("later health"),
    )
    .await;
    assert_eq!(summarized_counts(&recorded.logs), vec![1, 1]);
}

#[test]
fn summary_settings_reject_an_empty_batch() {
    let error = SummarySettings::from_parts(SummaryParts {
        model: "fake-chat".to_owned(),
        leaf_batch: 0,
        branch_batch: 8,
        root_batch: 5,
        min_leaves_for_branch: 5,
        min_branches_for_root: 3,
        idle_minutes: 15,
        stale_minutes: 5_760,
        stale_min_batch: 2,
        max_leaf_rounds: 10,
        pipeline_version: 5,
        session_tagging: false,
    })
    .expect_err("empty batch");
    assert!(error.to_string().contains("leaf batch"));
    assert_eq!(settings().idle_floor(), 5);
}

#[tokio::test]
async fn a_leaf_enqueues_suggest_tags_when_tagging_is_on() {
    let id = Uuid::parse_str("00000000-0000-4000-8000-000000000002").expect("id");
    let store = Arc::new(MemStore::new(vec![window(
        id,
        "cli:s1",
        "Session cli:s1",
        messages(at("2026-10-10T12:00:00Z"), 10),
    )]));
    let script = Arc::new(Scripted::success());
    let jobs = Arc::new(Seen::default());
    let settings = tagging_settings();
    let token = cancel();
    let env = bind(
        &store,
        &script,
        &jobs,
        &settings,
        at("2026-10-10T12:10:00Z"),
        &token,
    );
    let outcome = handle_summarize(&env, &payload_of(id, SummaryTrigger::Window))
        .await
        .expect("compact");
    assert_eq!(outcome.summaries, 1);
    assert!(!outcome.reenqueued);
    let job = jobs.only().await;
    let summary_id = store.written.lock().await.first().expect("leaf").id;
    let key = format!("tags-{summary_id}");
    assert_eq!(job.name, JobName::Fixed(FixedJob::SuggestTags));
    assert_eq!(job.key.as_deref(), Some(key.as_str()));
    assert_eq!(job.key_mode, JobKeyMode::Replace);
    assert_eq!(job.priority, 6);
    assert!(job.run_at.is_none());
    assert_tag_payload(&job, summary_id, id);
}

fn tagging_settings() -> SummarySettings {
    SummarySettings::from_parts(SummaryParts {
        model: "fake-chat".to_owned(),
        leaf_batch: 10,
        branch_batch: 8,
        root_batch: 5,
        min_leaves_for_branch: 5,
        min_branches_for_root: 3,
        idle_minutes: 15,
        stale_minutes: 5_760,
        stale_min_batch: 2,
        max_leaf_rounds: 10,
        pipeline_version: 5,
        session_tagging: true,
    })
    .expect("tagging settings")
}

fn assert_tag_payload(job: &Job, summary_id: Uuid, conversation_id: Uuid) {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        summary_id: Uuid,
        conversation_id: Uuid,
    }
    let value = serde_json::to_value(&job.payload).expect("payload");
    let body: Body = serde_json::from_value(value).expect("suggest payload");
    assert_eq!(body.summary_id, summary_id);
    assert_eq!(body.conversation_id, conversation_id);
}

fn bind<'a>(
    store: &'a MemStore,
    script: &'a Scripted,
    jobs: &'a Seen,
    settings: &'a SummarySettings,
    now: DateTime<Utc>,
    token: &'a CancellationToken,
) -> SummarizeEnv<'a> {
    SummarizeEnv {
        store,
        provider: script,
        queue: jobs,
        settings,
        now,
        cancel: token,
    }
}

fn assert_calls(calls: &[Captured], expected: &[ChatCall]) {
    assert_eq!(calls.len(), expected.len());
    for (call, recorded) in calls.iter().zip(expected) {
        assert_eq!(call.kind, recorded.prompt);
        assert_eq!(call.user, recorded.user);
        assert_eq!(call.model, recorded.model.as_deref().unwrap_or("fake-chat"));
        if let Some(tokens) = recorded.max_tokens {
            assert_eq!(call.max_tokens, Some(tokens));
        }
    }
}

fn assert_rows(written: &[NewSummary], rows: &[SummaryRow]) {
    assert_eq!(written.len(), rows.len());
    for summary in written {
        let row = rows
            .iter()
            .find(|row| {
                row.kind == summary.kind.as_str()
                    && row.earliest_at == summary.earliest_at
                    && row.latest_at == summary.latest_at
                    && row.message_count == summary.message_count
            })
            .expect("matching summary");
        assert_eq!(summary.depth, row.depth);
        assert_eq!(summary.content, row.content);
        assert_eq!(summary.model, row.model);
        assert_eq!(summary.pipeline_version, row.pipeline_version);
        assert_eq!(summary.created_at, row.created_at);
        assert_eq!(summary.conversation_id, row.conversation_id);
        let linked = written
            .iter()
            .any(|parent| parent.child_ids.contains(&summary.id));
        assert_eq!(linked, row.parent_id.is_some());
    }
}

fn assert_sources(written: &[NewSummary], messages: &[WindowMessage]) {
    let mut cursor = 0usize;
    for summary in written
        .iter()
        .filter(|summary| summary.kind == SummaryKind::Leaf)
    {
        for (ordinal, source) in summary.sources.iter().enumerate() {
            assert_eq!(i32::try_from(ordinal).expect("ordinal"), source.ordinal);
            let message = messages.get(cursor).expect("source message");
            assert_eq!(source.message_id, message.id);
            cursor = cursor.saturating_add(1);
        }
    }
    assert_eq!(cursor, messages.len());
}

async fn assert_health(store: &MemStore, settings: &SummarySettings, expected: &HealthSnap) {
    let health =
        summary_health(&store.pending().await.expect("pending"), settings).expect("health");
    assert_eq!(
        health,
        SummaryHealth {
            eligible: expected.eligible,
            active_tail: expected.active_tail,
            below_floor: expected.below_floor,
        }
    );
}

fn summarized_counts(logs: &[String]) -> Vec<u32> {
    logs.iter()
        .filter_map(|line| summarized_count(line))
        .collect()
}

fn summarized_count(line: &str) -> Option<u32> {
    let tail = line.rsplit_once(": ").map(|(_, tail)| tail)?;
    let number = tail.strip_suffix(" summaries")?;
    number.parse().ok()
}

impl MemStore {
    fn new(windows: Vec<ConversationWindow>) -> Self {
        Self {
            windows: Mutex::new(windows),
            written: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SummaryStore for MemStore {
    async fn conversation(
        &self,
        id: Uuid,
    ) -> Result<Option<ConversationWindow>, SummaryStoreError> {
        Ok(self
            .windows
            .lock()
            .await
            .iter()
            .find(|window| window.conversation.id == id)
            .cloned())
    }

    async fn pending(&self) -> Result<Vec<ConversationWindow>, SummaryStoreError> {
        Ok(self
            .windows
            .lock()
            .await
            .iter()
            .filter(|window| {
                !window.messages.is_empty()
                    && !window
                        .conversation
                        .session_key
                        .starts_with(kernel::memory::HEARTBEAT_SESSION_PREFIX)
            })
            .cloned()
            .collect())
    }

    async fn write_summary(&self, summary: NewSummary) -> Result<(), SummaryStoreError> {
        let node = SummaryNode {
            id: summary.id,
            kind: summary.kind,
            content: summary.content.clone(),
            message_count: summary.message_count,
            earliest_at: summary.earliest_at,
            latest_at: summary.latest_at,
            depth: summary.depth,
        };
        let mut windows = self.windows.lock().await;
        let window = windows
            .iter_mut()
            .find(|window| window.conversation.id == summary.conversation_id)
            .ok_or_else(|| SummaryStoreError::Missing {
                id: summary.conversation_id.to_string(),
            })?;
        match summary.kind {
            SummaryKind::Leaf => {
                for source in &summary.sources {
                    window
                        .messages
                        .retain(|message| message.id != source.message_id);
                }
                window.leaves.push(node);
            }
            SummaryKind::Branch => {
                window
                    .leaves
                    .retain(|leaf| !summary.child_ids.contains(&leaf.id));
                window.branches.push(node);
            }
            SummaryKind::Root => {
                window
                    .branches
                    .retain(|branch| !summary.child_ids.contains(&branch.id));
            }
        }
        drop(windows);
        self.written.lock().await.push(summary);
        Ok(())
    }
}

impl Scripted {
    fn success() -> Self {
        Self {
            failures: Mutex::new(Vec::new()),
            truncations: Mutex::new(0),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn truncate_once() -> Self {
        Self {
            failures: Mutex::new(Vec::new()),
            truncations: Mutex::new(1),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn fail_with(statuses: Vec<u16>) -> Self {
        Self {
            failures: Mutex::new(statuses),
            truncations: Mutex::new(0),
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat(&self, request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        let kind = kind_name(system_text(&request)?);
        let user = user_text(&request)?;
        self.calls.lock().await.push(Captured {
            kind: kind.to_owned(),
            user: user.to_owned(),
            max_tokens: request.max_tokens,
            model: request.model.clone(),
        });
        let status = {
            let mut failures = self.failures.lock().await;
            if failures.is_empty() {
                None
            } else {
                Some(failures.remove(0))
            }
        };
        if let Some(status) = status {
            return Err(status_error(status));
        }
        let mut truncations = self.truncations.lock().await;
        if *truncations > 0 {
            *truncations -= 1;
            return Ok(response(String::new(), Some(FinishReason::Length)));
        }
        drop(truncations);
        Ok(response(answer(kind), Some(FinishReason::Stop)))
    }

    async fn stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Receiver<ModelStreamItem>, ModelProviderError> {
        Err(plain_error("the summarizer script has no stream"))
    }

    async fn embed(&self, _texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        Err(plain_error("the summarizer script has no embeddings"))
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Ok(Vec::new())
    }
}

fn system_text(request: &ModelRequest) -> Result<&str, ModelProviderError> {
    let message = request
        .messages
        .first()
        .ok_or_else(|| plain_error("missing system message"))?;
    match &message.content {
        MessageContent::Text(text) => Ok(text),
        MessageContent::Parts(_) => Err(plain_error("system content is not text")),
    }
}

fn user_text(request: &ModelRequest) -> Result<&str, ModelProviderError> {
    let message = request
        .messages
        .get(1)
        .ok_or_else(|| plain_error("missing user message"))?;
    match &message.content {
        MessageContent::Text(text) => Ok(text),
        MessageContent::Parts(_) => Err(plain_error("user content is not text")),
    }
}

fn kind_name(system: &str) -> &'static str {
    if system == LEAF_SYSTEM_PROMPT {
        "LEAF_SYSTEM_PROMPT"
    } else if system == BRANCH_SYSTEM_PROMPT {
        "BRANCH_SYSTEM_PROMPT"
    } else if system == ROOT_SYSTEM_PROMPT {
        "ROOT_SYSTEM_PROMPT"
    } else {
        "OTHER"
    }
}

fn answer(kind: &str) -> String {
    match kind {
        "BRANCH_SYSTEM_PROMPT" => BRANCH_TEXT.to_owned(),
        "ROOT_SYSTEM_PROMPT" => ROOT_TEXT.to_owned(),
        _ => LEAF_TEXT.to_owned(),
    }
}

fn response(content: String, finish: Option<FinishReason>) -> LlmResponse {
    LlmResponse {
        r#type: LlmResponseKind::Text,
        content: if content.is_empty() {
            None
        } else {
            Some(content)
        },
        tool_calls: None,
        usage: None,
        finish_reason: finish,
    }
}

fn status_error(status: u16) -> ModelProviderError {
    let message = format!("model server returned {status} synthetic");
    ModelProviderError::Request {
        message,
        source: Box::new(std::io::Error::other("synthetic status")),
    }
}

fn plain_error(message: &str) -> ModelProviderError {
    ModelProviderError::Request {
        message: message.to_owned(),
        source: Box::new(std::io::Error::other(message.to_owned())),
    }
}

impl Default for Seen {
    fn default() -> Self {
        Self {
            jobs: Mutex::new(Vec::new()),
            dead: Mutex::new(Vec::new()),
        }
    }
}

impl Seen {
    fn with_dead(dead: Vec<(JobName, DateTime<Utc>)>) -> Self {
        Self {
            jobs: Mutex::new(Vec::new()),
            dead: Mutex::new(dead),
        }
    }

    async fn only(&self) -> Job {
        let mut jobs = self.jobs.lock().await;
        assert_eq!(jobs.len(), 1);
        jobs.remove(0)
    }

    async fn dead_count(&self, job: FixedJob) -> usize {
        self.dead
            .lock()
            .await
            .iter()
            .filter(|(name, _)| *name == JobName::Fixed(job))
            .count()
    }
}

#[async_trait]
impl JobQueue for Seen {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError> {
        let key = job.key.clone();
        {
            let mut jobs = self.jobs.lock().await;
            if let Some(key) = key
                && let Some(existing) = jobs
                    .iter_mut()
                    .find(|queued| queued.key.as_ref() == Some(&key))
            {
                *existing = job;
            } else {
                jobs.push(job);
            }
        }
        Ok(())
    }

    async fn claim(
        &self,
        _role: WorkerRole,
    ) -> Result<Option<kernel::queue::ClaimedJob>, JobQueueError> {
        Ok(None)
    }

    async fn complete(&self, _id: JobId) -> Result<(), JobQueueError> {
        Ok(())
    }

    async fn fail(&self, _id: JobId, _error: &str) -> Result<(), JobQueueError> {
        Ok(())
    }

    async fn revive_dead(
        &self,
        name: &JobName,
        dead_before: DateTime<Utc>,
    ) -> Result<u64, JobQueueError> {
        let revived = {
            let mut dead = self.dead.lock().await;
            let mut revived = 0u64;
            dead.retain(|(job, dead_at)| {
                let due = job == name && *dead_at < dead_before;
                if due {
                    revived = revived.saturating_add(1);
                }
                !due
            });
            revived
        };
        Ok(revived)
    }
}
