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
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::memory::TagEntityType;
use wire::gateway::memory::TagState;
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

const SUMMARY_ID: &str = "00000000-0000-4000-8000-000000000014";
const CONVERSATION_ID: &str = "00000000-0000-4000-8000-000000000002";

struct Scripted {
    answer: String,
    requests: Mutex<Vec<ModelRequest>>,
}

#[derive(Debug)]
struct Turn {
    outcome: SuggestOutcome,
    write: TagSuggestionWrite,
    requests: usize,
}

#[tokio::test]
async fn an_alias_resolves_to_the_accepted_canonical() {
    let mut context = context();
    context.vocabulary = vec![
        vocab("project", "acme-app", "", &[], TagState::Rejected),
        vocab("project", "acme", "Acme", &["acme-app"], TagState::Accepted),
    ];
    let turn = drive(&context, r#"[{"key":"project","value":"acme-app"}]"#)
        .await
        .expect("suggest");
    assert_eq!(turn.outcome.proposed, 1);
    assert_eq!(turn.outcome.fresh, 2);
    assert_eq!(turn.outcome.rejections, Vec::<String>::new());
    assert_eq!(turn.write.vocabulary, Vec::new());
    assert_eq!(turn.write.rows.len(), 2);
    assert!(turn.write.rows.iter().all(|row| {
        row.kind == TagSuggestKind::Add && row.value == "acme" && row.display == "Acme"
    }));
}

#[tokio::test]
async fn an_in_use_display_value_is_reused() {
    let mut context = context();
    context.vocabulary = vec![vocab(
        "project",
        "Widget Hub",
        "Widget Hub",
        &[],
        TagState::Accepted,
    )];
    let turn = drive(&context, r#"[{"key":"project","value":"widget-hub"}]"#)
        .await
        .expect("suggest");
    assert_eq!(turn.outcome.proposed, 1);
    assert_eq!(turn.write.vocabulary, Vec::new());
    assert!(turn.write.rows.iter().all(|row| row.value == "Widget Hub"));
}

#[test]
fn the_vocabulary_shows_aliases_and_hides_rejected_entries() {
    let mut context = context();
    context.vocabulary = vec![
        vocab(
            "project",
            "Widget Hub",
            "Widget Hub",
            &["widgets", "wh"],
            TagState::Accepted,
        ),
        vocab("topic", "storage", "", &[], TagState::Accepted),
        vocab("project", "scratch", "", &[], TagState::Rejected),
    ];
    let prompt = tag_user_prompt(&context);
    assert!(prompt.contains("- project:Widget Hub (also: widgets, wh)\n"));
    assert!(prompt.contains("- topic:storage\n"));
    assert!(!prompt.contains("scratch"));
}

#[test]
fn the_prompt_asks_for_at_most_three_reused_tags() {
    assert!(TAG_SYSTEM_PROMPT.contains("An empty array [] is a good answer"));
    assert!(TAG_SYSTEM_PROMPT.contains("Suggest at most 3 tags."));
    assert!(TAG_SYSTEM_PROMPT.contains("including when the conversation names one of its aliases"));
}

#[tokio::test]
async fn a_rejected_taxonomy_entry_is_not_proposed() {
    let mut context = context();
    context.vocabulary = vec![vocab("project", "secret", "", &[], TagState::Rejected)];
    let turn = drive(&context, r#"[{"key":"project","value":"secret"}]"#)
        .await
        .expect("suggest");
    assert_eq!(turn.outcome.proposed, 0);
    assert_eq!(turn.outcome.fresh, 0);
    assert_eq!(turn.write.rows, Vec::new());
    assert_eq!(
        turn.outcome.rejections,
        vec!["rejected — taxonomy rejects \"project:secret\"".to_owned()]
    );
}

#[tokio::test]
async fn a_model_tag_can_be_removed_without_the_protected_flag() {
    let mut context = context();
    context.tags = vec![on(
        TagEntityType::Conversation,
        "topic",
        "manual",
        "manual",
        "model",
        TagState::Accepted,
    )];
    let turn = drive(
        &context,
        r#"[{"action":"remove","key":"topic","value":"manual","reason":"stale"}]"#,
    )
    .await
    .expect("suggest");
    assert_eq!(turn.outcome.proposed, 1);
    assert_eq!(turn.outcome.fresh, 1);
    assert_eq!(turn.outcome.rejections, Vec::<String>::new());
    let row = turn.write.rows.first().expect("removal");
    assert_eq!(row.kind, TagSuggestKind::Remove);
    assert_eq!(row.entity_type, TagEntityType::Conversation);
    assert_eq!(row.value, "manual");
    assert_eq!(row.reason.as_deref(), Some("stale"));
    assert_eq!(turn.write.rows.len(), 1);
}

#[tokio::test]
async fn shape_rejects_do_not_consume_the_cap() {
    let long_key = "k".repeat(65);
    let long_value = "v".repeat(129);
    let mut items = vec![
        r#"{"key":"","value":"x"}"#.to_owned(),
        r#"{"key":"bad","value":""}"#.to_owned(),
        r#"{"key":"a:b","value":"x"}"#.to_owned(),
        format!(r#"{{"key":"{long_key}","value":"x"}}"#),
        format!(r#"{{"key":"topic","value":"{long_value}"}}"#),
        r#"{"action":"remove","key":"topic","value":"missing"}"#.to_owned(),
    ];
    for index in 1..=4 {
        items.push(format!(r#"{{"key":"topic","value":"item-{index}"}}"#));
    }
    let turn = drive(&context(), &format!("[{}]", items.join(",")))
        .await
        .expect("suggest");
    assert_eq!(turn.outcome.proposed, 3);
    assert_eq!(turn.outcome.fresh, 6);
    assert_eq!(turn.write.rows.len(), 6);
    assert!(
        turn.write
            .rows
            .iter()
            .all(|row| row.value.starts_with("item-"))
    );
    assert!(!turn.write.rows.iter().any(|row| row.value == "item-4"));
    assert!(
        turn.outcome
            .rejections
            .iter()
            .any(|note| note == "rejected — empty value for key \"\"")
    );
    assert!(
        turn.outcome
            .rejections
            .iter()
            .any(|note| note == "rejected — empty value for key \"bad\"")
    );
    assert!(
        turn.outcome
            .rejections
            .iter()
            .any(|note| note == "rejected — key \"a:b\" contains a colon")
    );
    assert!(
        turn.outcome
            .rejections
            .iter()
            .any(|note| note == "rejected — key is longer than 64 characters")
    );
    assert!(turn.outcome.rejections.iter().any(|note| {
        note == "rejected — value for key \"topic\" is longer than 128 characters"
    }));
    assert!(turn.outcome.rejections.iter().any(|note| {
        note == "rejected — remove target \"topic:missing\" is not an accepted conversation tag"
    }));
    assert!(
        turn.outcome
            .rejections
            .iter()
            .any(|note| note == "rejected — suggestion cap is 3")
    );
}

#[tokio::test]
async fn confidence_outside_zero_to_one_is_rejected() {
    let answer = concat!(
        r#"[{"key":"topic","value":"zero","confidence":0},"#,
        r#"{"key":"topic","value":"one","confidence":1},"#,
        r#"{"key":"topic","value":"high","confidence":1.5},"#,
        r#"{"key":"topic","value":"low","confidence":-0.1}]"#
    );
    let turn = drive(&context(), answer).await.expect("suggest");
    assert_eq!(turn.outcome.proposed, 2);
    assert_eq!(turn.write.rows.len(), 4);
    assert!(turn.write.rows.iter().any(|row| {
        row.value == "zero"
            && row
                .confidence
                .is_some_and(|value| value.to_bits() == 0.0_f64.to_bits())
    }));
    assert!(turn.write.rows.iter().any(|row| {
        row.value == "one"
            && row
                .confidence
                .is_some_and(|value| value.to_bits() == 1.0_f64.to_bits())
    }));
    assert_eq!(
        turn.outcome.rejections,
        vec![
            "rejected — confidence for key \"topic\" is outside 0..=1".to_owned(),
            "rejected — confidence for key \"topic\" is outside 0..=1".to_owned(),
        ]
    );
}

#[tokio::test]
async fn duplicate_adds_in_one_reply_insert_once() {
    let answer = r#"[{"key":"topic","value":"notes"},{"key":"topic","value":"notes"}]"#;
    let turn = drive(&context(), answer).await.expect("suggest");
    assert_eq!(turn.outcome.proposed, 2);
    assert_eq!(turn.outcome.fresh, 2);
    assert_eq!(turn.write.rows.len(), 2);
    assert_eq!(turn.write.vocabulary.len(), 1);
    assert_eq!(turn.outcome.rejections, Vec::<String>::new());
}

#[tokio::test]
async fn an_empty_reply_errors_and_an_empty_array_stores_nothing() {
    let error = drive(&context(), "").await.expect_err("empty");
    assert_eq!(error.to_string(), "the model returned no tag suggestions");
    let turn = drive(&context(), "[]").await.expect("empty array");
    assert_eq!(turn.outcome.proposed, 0);
    assert_eq!(turn.outcome.fresh, 0);
    assert_eq!(turn.write.rows, Vec::new());
    assert_eq!(turn.write.vocabulary, Vec::new());
    assert_eq!(turn.requests, 1);
}

#[tokio::test]
async fn the_payload_must_name_the_summaries_conversation() {
    let summary_id = id(SUMMARY_ID);
    let missing = drive_job(&context(), summary_id, id(CONVERSATION_ID), false)
        .await
        .expect_err("missing");
    assert!(matches!(missing, WorkerError::SummaryMissing { id } if id == summary_id));
    let context = context();
    let other = id("00000000-0000-4000-8000-000000000099");
    let mismatch = drive_job(&context, context.summary_id, other, true)
        .await
        .expect_err("mismatch");
    assert!(matches!(
        mismatch,
        WorkerError::ConversationMismatch { payload, summary }
            if payload == other && summary == context.conversation_id
    ));
}

fn context() -> TagJobContext {
    TagJobContext {
        summary_id: id(SUMMARY_ID),
        conversation_id: id(CONVERSATION_ID),
        session_key: "cli:s1".to_owned(),
        title: None,
        agent: "rivet".to_owned(),
        summary: "A deploy summary.".to_owned(),
        vocabulary: Vec::new(),
        tags: Vec::new(),
    }
}

fn vocab(
    key: &str,
    value: &str,
    display: &str,
    aliases: &[&str],
    state: TagState,
) -> TagVocabulary {
    TagVocabulary {
        key: key.to_owned(),
        value: value.to_owned(),
        display: display.to_owned(),
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
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

async fn drive(context: &TagJobContext, answer: &str) -> Result<Turn, WorkerError> {
    run(
        context,
        answer,
        context.summary_id,
        context.conversation_id,
        true,
    )
    .await
}

async fn drive_job(
    context: &TagJobContext,
    summary_id: Uuid,
    conversation_id: Uuid,
    scripted: bool,
) -> Result<Turn, WorkerError> {
    run(context, "[]", summary_id, conversation_id, scripted).await
}

async fn run(
    context: &TagJobContext,
    answer: &str,
    summary_id: Uuid,
    conversation_id: Uuid,
    scripted: bool,
) -> Result<Turn, WorkerError> {
    let store = Arc::new(testkit::FakeMemoryStore::default());
    if scripted {
        store.script_tag_context(context.clone()).await;
    }
    let provider = Scripted {
        answer: answer.to_owned(),
        requests: Mutex::new(Vec::new()),
    };
    let settings = TagSettings::new("tagger-v1", false).expect("settings");
    let cancel = CancellationToken::new();
    let env = CategorizeEnv {
        store: store.as_ref(),
        provider: &provider,
        settings: &settings,
        now: at(),
        cancel: &cancel,
        attempt: 1,
    };
    let payload = suggest_tags_job(summary_id, conversation_id)
        .expect("payload")
        .payload;
    let outcome = handle_suggest(&env, &payload).await?;
    let requests = provider.requests.lock().await.len();
    Ok(Turn {
        outcome,
        write: taken(&store).await,
        requests,
    })
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

fn id(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("uuid")
}

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 10, 12, 10, 0)
        .single()
        .expect("time")
}

#[async_trait]
impl ModelProvider for Scripted {
    async fn chat(&self, request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        self.requests.lock().await.push(request);
        Ok(LlmResponse {
            r#type: LlmResponseKind::Text,
            content: Some(self.answer.clone()),
            tool_calls: None,
            usage: None,
            finish_reason: None,
        })
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
