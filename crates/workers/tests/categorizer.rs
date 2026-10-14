#![cfg(test)]

use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::TimeZone;
use chrono::Utc;
use kernel::memory::TagJobContext;
use kernel::memory::TagOnEntity;
use kernel::memory::TagSuggestKind;
use kernel::memory::TagSuggestionWrite;
use kernel::memory::TagVocabulary;
use kernel::model::Embedding;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use kernel::model::ModelRequest;
use kernel::model::ModelStreamItem;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::memory::TagEntityType;
use wire::gateway::memory::TagState;
use wire::message::MessageContent;
use wire::stream::LlmResponse;
use wire::stream::LlmResponseKind;
use workers::CategorizeEnv;
use workers::SuggestOutcome;
use workers::TAG_SYSTEM_PROMPT;
use workers::TagSettings;
use workers::WorkerError;
use workers::handle_suggest;
use workers::suggest_tags_job;
use workers::tag_user_prompt;

const REPLAYED: &[&str] = &[
    "tagger suggestions (chat), acceptance, removals",
    "protected removals allowed",
    "failing tagger fails the job",
];

const SKIPPED: &[(&str, &str)] = &[
    (
        "tag store lifecycle",
        "tag reads and writes already live on MemoryStore",
    ),
    (
        "vocabulary: upsert, nest, decide, propose, merge",
        "taxonomy edits already live on MemoryStore",
    ),
    (
        "native tagger request shape",
        "the native POST /tag client is not ModelProvider",
    ),
];

const JOB_STEPS: &[&str] = &["drain", "drain second leaf"];

const OUTSIDE_JOB: &[&str] = &[
    "pending",
    "vocabulary suggested",
    "accept the summary project tag",
    "conversationIdsWithTag",
    "counts",
    "browse by tag",
    "search by tag",
    "accept the conversation topic tag",
    "a person adds a tag",
    "pending after removals",
    "list with removal fields",
];

const FIRST_ANSWER: &str = concat!(
    r#"[{"key":"project","value":"AcmeKit","confidence":0.9,"reason":"deploy scripts for it"},"#,
    r#"{"key":"topic","value":"blue-green deploys","confidence":0.7}]"#
);

const SECOND_ANSWER: &str = concat!(
    r#"[{"key":"bad-key","value":""},"#,
    r#"{"key":"topic","value":"release-notes","confidence":0.8},"#,
    r#"{"key":"project","value":"AcmeKit","confidence":0.9,"reason":"deploy scripts for it"},"#,
    r#"{"key":"topic","value":"blue-green deploys","confidence":0.7},"#,
    r#"{"key":"project","value":"AcmeKit"}]"#
);

const SUMMARY_ID: &str = "00000000-0000-4000-8000-000000000014";
const CONVERSATION_ID: &str = "00000000-0000-4000-8000-000000000002";

#[derive(Debug, Deserialize)]
struct TagsFile {
    scenarios: Vec<TagsScenario>,
}

#[derive(Debug, Deserialize)]
struct TagsScenario {
    name: String,
    #[serde(default)]
    steps: Vec<TagsStep>,
    #[serde(default)]
    chat_tag_calls: Vec<ChatTagCall>,
}

#[derive(Debug, Deserialize)]
struct TagsStep {
    label: String,
}

#[derive(Debug, Deserialize)]
struct ChatTagCall {
    prompt: String,
    user: String,
    max_tokens: u32,
    temperature: f64,
    model: String,
}

#[derive(Debug, Deserialize)]
struct SumFile {
    prompts_sha256: PromptHashes,
}

#[derive(Debug, Deserialize)]
struct PromptHashes {
    #[serde(rename = "TAG_SYSTEM_PROMPT")]
    tag: String,
}

struct Scripted {
    answers: Mutex<Vec<Result<String, u16>>>,
    requests: Mutex<Vec<ModelRequest>>,
}

struct Turn {
    outcome: SuggestOutcome,
    write: TagSuggestionWrite,
    request: ModelRequest,
}

#[test]
fn every_tags_scenario_is_replayed_or_skipped() {
    let file = tags_file();
    let mut covered = Vec::new();
    for name in REPLAYED {
        assert!(file.scenarios.iter().any(|scenario| scenario.name == *name));
        covered.push(*name);
    }
    for (name, reason) in SKIPPED {
        assert_ne!(*reason, "");
        assert!(file.scenarios.iter().any(|scenario| scenario.name == *name));
        assert!(!REPLAYED.contains(name));
        covered.push(*name);
    }
    for scenario in &file.scenarios {
        assert!(
            covered.contains(&scenario.name.as_str()),
            "{}",
            scenario.name
        );
    }
}

#[test]
fn chat_steps_outside_the_job_are_named() {
    let file = tags_file();
    let chat = scenario(&file, "tagger suggestions (chat), acceptance, removals");
    let labels: Vec<&str> = chat.steps.iter().map(|step| step.label.as_str()).collect();
    for label in JOB_STEPS {
        assert!(labels.contains(label));
    }
    for label in OUTSIDE_JOB {
        assert!(labels.contains(label));
        assert!(!JOB_STEPS.contains(label));
    }
}

#[test]
fn the_recorded_system_prompt_hash_is_present() {
    let path = testkit::recording("memory-sqlite/summarizer.json");
    let text = std::fs::read_to_string(&path).expect("summarizer recording");
    let file: SumFile = serde_json::from_str(&text).expect("summarizer json");
    assert_ne!(file.prompts_sha256.tag, "");
    let tags = tags_file();
    let chat = scenario(&tags, "tagger suggestions (chat), acceptance, removals");
    let call = chat.chat_tag_calls.first().expect("call");
    assert_eq!(call.prompt, "TAG_SYSTEM_PROMPT");
}

#[tokio::test]
async fn recorded_chat_prompts_match_the_user_text() {
    let file = tags_file();
    let chat = scenario(&file, "tagger suggestions (chat), acceptance, removals");
    let first = chat.chat_tag_calls.first().expect("first call");
    let second = chat.chat_tag_calls.get(1).expect("second call");
    assert_eq!(first.max_tokens, 400);
    assert_eq!(first.temperature.to_bits(), 0.3_f64.to_bits());
    assert_eq!(first.model, "tagger-v1");
    assert_eq!(second.max_tokens, 400);
    assert_eq!(second.temperature.to_bits(), 0.3_f64.to_bits());
    let summary = between(&first.user, "Summary:\n", "\n\nTags (JSON array only):");
    assert_eq!(tag_user_prompt(&first_context(summary)), first.user);
    assert_eq!(tag_user_prompt(&second_context(summary)), second.user);
}

#[tokio::test]
async fn the_first_chat_call_stores_four_suggestions() {
    let summary = recorded_summary();
    let turn = drive(&first_context(&summary), FIRST_ANSWER, false, 1)
        .await
        .expect("suggest");
    assert_eq!(turn.outcome.proposed, 2);
    assert_eq!(turn.outcome.fresh, 4);
    assert_eq!(turn.outcome.rejections, Vec::<String>::new());
    assert!(!turn.outcome.swallowed);
    assert_eq!(system_text(&turn.request), TAG_SYSTEM_PROMPT);
    assert_eq!(turn.request.max_tokens, Some(400));
    assert_eq!(turn.request.tools, Vec::new());
    assert!(turn.request.thinking.is_none());
    assert_eq!(turn.request.model, "tagger-v1");
    assert_eq!(
        confidence_bits(&turn.write, "project"),
        Some(0.9_f64.to_bits())
    );
    assert_eq!(
        confidence_bits(&turn.write, "topic"),
        Some(0.7_f64.to_bits())
    );
    assert_eq!(turn.write.proposed_by, "tagger-v1");
    assert_eq!(turn.write.rows.len(), 4);
    assert_eq!(turn.write.vocabulary.len(), 2);
    assert!(
        turn.write
            .rows
            .iter()
            .all(|row| row.kind == TagSuggestKind::Add)
    );
    assert!(has_add(
        &turn.write,
        TagEntityType::Summary,
        "project",
        "acmekit",
        "AcmeKit"
    ));
    assert!(has_add(
        &turn.write,
        TagEntityType::Conversation,
        "project",
        "acmekit",
        "AcmeKit"
    ));
    assert!(has_add(
        &turn.write,
        TagEntityType::Summary,
        "topic",
        "blue-green-deploys",
        "blue-green deploys"
    ));
    assert!(has_add(
        &turn.write,
        TagEntityType::Conversation,
        "topic",
        "blue-green-deploys",
        "blue-green deploys"
    ));
    assert!(has_vocab(&turn.write, "project", "acmekit", "AcmeKit"));
    assert!(has_vocab(
        &turn.write,
        "topic",
        "blue-green-deploys",
        "blue-green deploys"
    ));
}

#[tokio::test]
async fn the_second_chat_call_adds_release_notes_and_skips_duplicates() {
    let summary = recorded_summary();
    let turn = drive(&second_context(&summary), SECOND_ANSWER, false, 1)
        .await
        .expect("suggest");
    assert_eq!(turn.outcome.proposed, 4);
    assert_eq!(turn.outcome.fresh, 2);
    assert_eq!(
        turn.outcome.rejections,
        vec!["rejected — empty value for key \"bad-key\"".to_owned()]
    );
    assert_eq!(turn.write.rows.len(), 2);
    assert!(
        turn.write
            .rows
            .iter()
            .all(|row| row.value == "release-notes")
    );
    assert!(
        turn.write
            .vocabulary
            .iter()
            .all(|row| row.value == "release-notes" && row.display.is_empty())
    );
    assert!(
        turn.write
            .vocabulary
            .iter()
            .any(|row| row.reason == "proposed by tagger-v1")
    );
    assert!(turn.write.rows.iter().all(|row| {
        row.confidence
            .is_some_and(|value| value.to_bits() == 0.8_f64.to_bits())
            && row.reason.is_none()
    }));
}

#[tokio::test]
async fn protected_removals_follow_the_flag() {
    let summary = recorded_summary();
    let context = manual_context(&summary);
    let answer = r#"[{"action":"remove","key":"topic","value":"manual","reason":"allowed now"}]"#;
    let refused = drive(&context, answer, false, 1).await.expect("refused");
    assert_eq!(refused.outcome.proposed, 0);
    assert_eq!(refused.outcome.fresh, 0);
    assert_eq!(refused.write.rows, Vec::new());
    assert!(tag_user_prompt(&context).contains("- topic:manual"));
    let allowed = drive(&context, answer, true, 1).await.expect("allowed");
    assert_eq!(allowed.outcome.proposed, 1);
    assert_eq!(allowed.outcome.fresh, 1);
    let row = allowed.write.rows.first().expect("removal");
    assert_eq!(row.kind, TagSuggestKind::Remove);
    assert_eq!(row.entity_type, TagEntityType::Conversation);
    assert_eq!(row.value, "manual");
    assert_eq!(row.reason.as_deref(), Some("allowed now"));
}

#[tokio::test]
async fn attempt_one_keeps_the_http_400_and_attempt_two_completes() {
    let summary = recorded_summary();
    let context = first_context(&summary);
    let error = drive_status(&context, 400, 1).await.expect_err("retry");
    assert_eq!(error.to_string(), "LLM HTTP 400 (not retrying)");
    let swallowed = drive_status(&context, 400, 2).await.expect("swallowed");
    assert!(swallowed.swallowed);
    assert_eq!(swallowed.proposed, 0);
}

#[test]
fn suggest_tags_job_uses_the_recorded_key_and_priority() {
    let job = suggest_tags_job(id(SUMMARY_ID), id(CONVERSATION_ID)).expect("job");
    assert_eq!(
        job.key.as_deref(),
        Some("tags-00000000-0000-4000-8000-000000000014")
    );
    assert_eq!(job.priority, 6);
    assert!(job.run_at.is_none());
    assert_eq!(job.key_mode, kernel::queue::JobKeyMode::Replace);
    assert_eq!(
        job.name,
        kernel::queue::JobName::Fixed(kernel::queue::FixedJob::SuggestTags)
    );
}

fn recorded_summary() -> String {
    let file = tags_file();
    let chat = scenario(&file, "tagger suggestions (chat), acceptance, removals");
    let user = &chat.chat_tag_calls.first().expect("call").user;
    between(user, "Summary:\n", "\n\nTags (JSON array only):").to_owned()
}

fn first_context(summary: &str) -> TagJobContext {
    TagJobContext {
        summary_id: id(SUMMARY_ID),
        conversation_id: id(CONVERSATION_ID),
        session_key: "cli:s1".to_owned(),
        title: None,
        agent: "rivet".to_owned(),
        summary: summary.to_owned(),
        vocabulary: vec![vocab("topic", "memory-compaction", "", TagState::Accepted)],
        tags: Vec::new(),
    }
}

fn second_context(summary: &str) -> TagJobContext {
    let mut context = first_context(summary);
    context
        .vocabulary
        .push(vocab("project", "acmekit", "AcmeKit", TagState::Suggested));
    context.vocabulary.push(vocab(
        "topic",
        "blue-green-deploys",
        "blue-green deploys",
        TagState::Suggested,
    ));
    context.tags = vec![
        on(
            TagEntityType::Summary,
            "project",
            "acmekit",
            "AcmeKit",
            "model",
            TagState::Accepted,
        ),
        on(
            TagEntityType::Summary,
            "topic",
            "blue-green-deploys",
            "blue-green deploys",
            "model",
            TagState::Suggested,
        ),
        on(
            TagEntityType::Conversation,
            "project",
            "acmekit",
            "AcmeKit",
            "model",
            TagState::Suggested,
        ),
        on(
            TagEntityType::Conversation,
            "topic",
            "blue-green-deploys",
            "blue-green deploys",
            "model",
            TagState::Accepted,
        ),
        on(
            TagEntityType::Conversation,
            "topic",
            "manual",
            "manual",
            "user",
            TagState::Accepted,
        ),
    ];
    context
}

fn manual_context(summary: &str) -> TagJobContext {
    let mut context = first_context(summary);
    context.tags = vec![on(
        TagEntityType::Conversation,
        "topic",
        "manual",
        "manual",
        "user",
        TagState::Accepted,
    )];
    context
}

fn vocab(key: &str, value: &str, display: &str, state: TagState) -> TagVocabulary {
    TagVocabulary {
        key: key.to_owned(),
        value: value.to_owned(),
        display: display.to_owned(),
        aliases: Vec::new(),
        state,
    }
}

fn on(
    entity_type: TagEntityType,
    key: &str,
    value: &str,
    display: &str,
    source: &str,
    state: TagState,
) -> TagOnEntity {
    TagOnEntity {
        entity_type,
        key: key.to_owned(),
        value: value.to_owned(),
        display: display.to_owned(),
        source: source.to_owned(),
        state,
    }
}

async fn drive(
    context: &TagJobContext,
    answer: &str,
    protected: bool,
    attempt: u32,
) -> Result<Turn, WorkerError> {
    let store = Arc::new(testkit::FakeMemoryStore::default());
    store.script_tag_context(context.clone()).await;
    let provider = Arc::new(Scripted::text(answer));
    let outcome = suggest(&store, provider.as_ref(), protected, attempt, context).await?;
    Ok(Turn {
        outcome,
        write: taken(&store).await,
        request: provider
            .requests
            .lock()
            .await
            .first()
            .expect("request")
            .clone(),
    })
}

async fn drive_status(
    context: &TagJobContext,
    status: u16,
    attempt: u32,
) -> Result<SuggestOutcome, WorkerError> {
    let store = Arc::new(testkit::FakeMemoryStore::default());
    store.script_tag_context(context.clone()).await;
    let provider = Scripted::status(status);
    suggest(&store, &provider, false, attempt, context).await
}

async fn suggest(
    store: &testkit::FakeMemoryStore,
    provider: &Scripted,
    protected: bool,
    attempt: u32,
    context: &TagJobContext,
) -> Result<SuggestOutcome, WorkerError> {
    let settings = TagSettings::new("tagger-v1", protected).expect("settings");
    let cancel = CancellationToken::new();
    let env = CategorizeEnv {
        store,
        provider,
        settings: &settings,
        now: at(),
        cancel: &cancel,
        attempt,
    };
    let payload = suggest_tags_job(context.summary_id, context.conversation_id)
        .expect("payload")
        .payload;
    handle_suggest(&env, &payload).await
}

async fn taken(store: &testkit::FakeMemoryStore) -> TagSuggestionWrite {
    store
        .requests()
        .await
        .into_iter()
        .find_map(|request| {
            if let testkit::MemoryRequest::StoreTagSuggestions(write) = request {
                Some(write)
            } else {
                None
            }
        })
        .expect("stored suggestions")
}

fn confidence_bits(write: &TagSuggestionWrite, key: &str) -> Option<u64> {
    write
        .rows
        .iter()
        .find(|row| row.key == key)
        .and_then(|row| row.confidence)
        .map(f64::to_bits)
}

fn has_add(
    write: &TagSuggestionWrite,
    entity: TagEntityType,
    key: &str,
    value: &str,
    display: &str,
) -> bool {
    write.rows.iter().any(|row| {
        row.kind == TagSuggestKind::Add
            && row.entity_type == entity
            && row.key == key
            && row.value == value
            && row.display == display
            && row.reason.as_deref()
                == if key == "project" {
                    Some("deploy scripts for it")
                } else {
                    None
                }
    })
}

fn has_vocab(write: &TagSuggestionWrite, key: &str, value: &str, display: &str) -> bool {
    write.vocabulary.iter().any(|row| {
        row.key == key
            && row.value == value
            && row.display == display
            && row.reason == "proposed by tagger-v1"
    })
}

fn system_text(request: &ModelRequest) -> &str {
    match &request.messages.first().expect("system").content {
        MessageContent::Text(text) => text,
        MessageContent::Parts(_) => "",
    }
}

fn tags_file() -> TagsFile {
    let path = testkit::recording("memory-sqlite/tags.json");
    let text = std::fs::read_to_string(&path).expect("tags recording");
    serde_json::from_str(&text).expect("tags json")
}

fn scenario<'a>(file: &'a TagsFile, name: &str) -> &'a TagsScenario {
    file.scenarios
        .iter()
        .find(|scenario| scenario.name == name)
        .expect("scenario")
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let head = text.split_once(start).expect("start").1;
    head.split_once(end).expect("end").0
}

fn id(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("uuid")
}

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 10, 12, 10, 0)
        .single()
        .expect("time")
}

impl Scripted {
    fn text(answer: &str) -> Self {
        Self {
            answers: Mutex::new(vec![Ok(answer.to_owned())]),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn status(status: u16) -> Self {
        Self {
            answers: Mutex::new(vec![Err(status)]),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat(&self, request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        self.requests.lock().await.push(request);
        let next = self.answers.lock().await.pop();
        match next {
            Some(Ok(content)) => Ok(LlmResponse {
                r#type: LlmResponseKind::Text,
                content: Some(content),
                tool_calls: None,
                usage: None,
                finish_reason: None,
            }),
            Some(Err(status)) => Err(ModelProviderError::Request {
                message: format!("model server returned {status} synthetic"),
                source: Box::new(std::io::Error::other("synthetic status")),
            }),
            None => Err(ModelProviderError::Request {
                message: "the tag script is empty".to_owned(),
                source: Box::new(std::io::Error::other("empty script")),
            }),
        }
    }

    async fn stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Receiver<ModelStreamItem>, ModelProviderError> {
        Err(ModelProviderError::Request {
            message: "the tag script has no stream".to_owned(),
            source: Box::new(std::io::Error::other("no stream")),
        })
    }

    async fn embed(&self, _texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        Err(ModelProviderError::Request {
            message: "the tag script has no embeddings".to_owned(),
            source: Box::new(std::io::Error::other("no embeddings")),
        })
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Ok(Vec::new())
    }
}
