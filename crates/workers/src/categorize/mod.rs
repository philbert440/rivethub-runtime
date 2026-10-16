mod job;
mod parse;
mod prompt;
mod rules;
mod settings;

use chrono::DateTime;
use chrono::Utc;
use kernel::memory::MemoryStore;
use kernel::memory::TagJobContext;
use kernel::memory::TagSuggestionWrite;
use kernel::model::ModelProvider;
use kernel::model::ModelRequest;
use tokio_util::sync::CancellationToken;
use wire::JsonObject;
use wire::message::Message;
use wire::message::MessageContent;
use wire::message::Role;

use self::prompt::MAX_TAG_TOKENS;
use crate::error::WorkerError;
use crate::summarize::llm_error;

pub use job::suggest_tags_job;
pub use prompt::TAG_SYSTEM_PROMPT;
pub use prompt::tag_user_prompt;
pub use settings::TagSettings;

const MAX_ATTEMPTS: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestOutcome {
    pub proposed: u32,
    pub fresh: u32,
    pub rejections: Vec<String>,
    pub swallowed: bool,
}

#[expect(
    missing_debug_implementations,
    reason = "the memory store and the model provider are trait objects"
)]
pub struct CategorizeEnv<'a> {
    pub store: &'a dyn MemoryStore,
    pub provider: &'a dyn ModelProvider,
    pub settings: &'a TagSettings,
    pub now: DateTime<Utc>,
    pub cancel: &'a CancellationToken,
    pub attempt: u32,
}

pub async fn handle_suggest(
    env: &CategorizeEnv<'_>,
    payload: &JsonObject,
) -> Result<SuggestOutcome, WorkerError> {
    match suggest(env, payload).await {
        Ok(outcome) => Ok(outcome),
        Err(error) if env.attempt >= MAX_ATTEMPTS => Ok(swallow(env, &error)),
        Err(error) => Err(error),
    }
}

fn swallow(env: &CategorizeEnv<'_>, error: &WorkerError) -> SuggestOutcome {
    tracing::warn!("suggest-tags attempt {} swallowed: {error}", env.attempt);
    SuggestOutcome {
        proposed: 0,
        fresh: 0,
        rejections: Vec::new(),
        swallowed: true,
    }
}

async fn suggest(
    env: &CategorizeEnv<'_>,
    payload: &JsonObject,
) -> Result<SuggestOutcome, WorkerError> {
    cancelled(env)?;
    let request = job::read_payload(payload)?;
    let Some(context) = env.store.tag_job_context(request.summary_id).await? else {
        return Err(WorkerError::SummaryMissing {
            id: request.summary_id,
        });
    };
    if context.conversation_id != request.conversation_id {
        return Err(WorkerError::ConversationMismatch {
            payload: request.conversation_id,
            summary: context.conversation_id,
        });
    }
    let reply = ask(env, &context).await?;
    let items = parse::parse_suggestions(&reply)?;
    let planned = rules::plan(
        &context,
        &items,
        &env.settings.model,
        env.settings.allow_protected_removals,
    )?;
    finish(env, &context, planned).await
}

async fn finish(
    env: &CategorizeEnv<'_>,
    context: &TagJobContext,
    planned: rules::SuggestionPlan,
) -> Result<SuggestOutcome, WorkerError> {
    let stored = env
        .store
        .store_tag_suggestions(write_of(env, &planned))
        .await?;
    let fresh = fresh_count(stored.tags, stored.removals)?;
    log_outcome(context, planned.proposed, fresh, &planned.rejections);
    Ok(SuggestOutcome {
        proposed: planned.proposed,
        fresh,
        rejections: planned.rejections,
        swallowed: false,
    })
}

async fn ask(env: &CategorizeEnv<'_>, context: &TagJobContext) -> Result<String, WorkerError> {
    cancelled(env)?;
    let request = ModelRequest {
        options: kernel::model::ModelOptions::default(),
        model: env.settings.model.clone(),
        messages: vec![
            text_message(Role::System, TAG_SYSTEM_PROMPT),
            text_message(Role::User, &tag_user_prompt(context)),
        ],
        tools: Vec::new(),
        thinking: None,
        max_tokens: Some(MAX_TAG_TOKENS),
    };
    let response = tokio::select! {
        biased;
        () = env.cancel.cancelled() => return Err(WorkerError::Cancelled),
        result = env.provider.chat(request) => result.map_err(llm_error)?,
    };
    match response.content {
        Some(text) if !text.is_empty() => Ok(text),
        Some(_) | None => Err(WorkerError::TagSuggestions {
            message: "the model returned no tag suggestions".to_owned(),
        }),
    }
}

fn write_of(env: &CategorizeEnv<'_>, planned: &rules::SuggestionPlan) -> TagSuggestionWrite {
    TagSuggestionWrite {
        proposed_by: env.settings.model.clone(),
        at: env.now,
        rows: planned.rows.clone(),
        vocabulary: planned.vocabulary.clone(),
    }
}

fn fresh_count(tags: u64, removals: u64) -> Result<u32, WorkerError> {
    let sum = tags.checked_add(removals).ok_or(WorkerError::Count)?;
    u32::try_from(sum).map_err(|_| WorkerError::Count)
}

fn log_outcome(context: &TagJobContext, proposed: u32, fresh: u32, rejections: &[String]) {
    for note in rejections {
        tracing::info!("tags: {note}");
    }
    tracing::info!(
        "tags: {} — {proposed} proposed, {fresh} new",
        id_prefix(context.conversation_id)
    );
}

fn id_prefix(id: uuid::Uuid) -> String {
    let text = id.to_string();
    match text.get(..8) {
        Some(prefix) => prefix.to_owned(),
        None => text,
    }
}

fn cancelled(env: &CategorizeEnv<'_>) -> Result<(), WorkerError> {
    if env.cancel.is_cancelled() {
        Err(WorkerError::Cancelled)
    } else {
        Ok(())
    }
}

fn text_message(role: Role, text: &str) -> Message {
    Message {
        role,
        content: MessageContent::Text(text.to_owned()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
    }
}
