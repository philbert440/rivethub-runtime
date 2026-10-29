mod job;
mod parse;
mod prompt;

use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use kernel::filing::Answer;
use kernel::filing::ConversationPlacement;
use kernel::filing::Evidence;
use kernel::filing::Filable;
use kernel::filing::FilingStore;
use kernel::filing::Refusal;
use kernel::filing::Rule;
use kernel::filing::Step;
use kernel::filing::Subject;
use kernel::filing::Target;
use kernel::memory::ConversationHead;
use kernel::model::ModelOptions;
use kernel::model::ModelProvider;
use kernel::model::ModelRequest;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Slug;
use kernel::topics::Suggestion;
use kernel::topics::TopicId;
use kernel::topics::TopicListing;
use kernel::topics::TopicName;
use kernel::topics::TopicStore;
use kernel::transcript::SessionLister;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::JsonObject;
use wire::message::Message;
use wire::message::MessageContent;
use wire::message::Role;

pub use job::file_conversation_job;
pub use prompt::FILING_SYSTEM_PROMPT;

use crate::error::WorkerError;
use crate::summarize::llm_error;

const MAX_FILING_TOKENS: u32 = 200;
const CORRECTION_EXAMPLES: u32 = 8;

#[expect(
    missing_debug_implementations,
    reason = "the topic, filing and lineage stores are trait objects"
)]
#[derive(Clone)]
pub struct SessionTopics {
    pub store: Arc<dyn TopicStore>,
    pub filing: Arc<dyn FilingStore>,
    pub lineage: Arc<dyn SessionLister>,
}

#[expect(
    missing_debug_implementations,
    reason = "the topic ports and the model provider are trait objects"
)]
pub struct FilingEnv<'a> {
    pub topics: &'a SessionTopics,
    pub provider: &'a dyn ModelProvider,
    pub model: &'a str,
    pub now: DateTime<Utc>,
    pub cancel: &'a CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilingPlan {
    Never,
    Placed(PlacementSource),
    Inherits,
    File {
        target: Target,
        topic: TopicId,
        rule: Rule,
    },
    Suggest {
        target: Target,
        name: TopicName,
        under: Option<TopicId>,
    },
    Refused {
        name: TopicName,
        refusal: Refusal,
    },
    Unfiled,
}

pub fn file_target(payload: &JsonObject) -> Result<Uuid, WorkerError> {
    Ok(job::read_payload(payload)?.conversation_id)
}

pub async fn handle_file(
    env: &FilingEnv<'_>,
    conversation: Uuid,
) -> Result<FilingPlan, WorkerError> {
    let plan = plan_filing(env, conversation).await?;
    apply_filing(env, &plan).await?;
    tracing::info!("filing: {} {plan:?}", short(conversation));
    Ok(plan)
}

pub async fn plan_filing(
    env: &FilingEnv<'_>,
    conversation: Uuid,
) -> Result<FilingPlan, WorkerError> {
    cancelled(env)?;
    let subject = env
        .topics
        .filing
        .subject(conversation)
        .await?
        .ok_or(WorkerError::ConversationMissing { id: conversation })?;
    let filable = Filable::of(&subject.head);
    let Some(target) = filable.target(conversation) else {
        return Ok(FilingPlan::Never);
    };
    let placed = filed_topic(env, &target).await?;
    let child = child_session(env, &filable).await?;
    let parent_topic = task_topic(env, &filable, conversation).await?;
    let listings = env.topics.store.list().await?;
    let redirects = env.topics.store.redirects().await?;
    let evidence = Evidence {
        placed: placed.map(|(by, _)| by),
        child,
        parent_topic,
        cwd: subject.cwd.as_deref(),
        title: subject.head.title.as_deref(),
        summary: subject.summary.as_deref(),
    };
    match evidence.decide(&listings, &redirects) {
        Step::Placed(by) => Ok(FilingPlan::Placed(by)),
        Step::Inherits => Ok(FilingPlan::Inherits),
        Step::File { topic, rule } => Ok(FilingPlan::File {
            target,
            topic,
            rule,
        }),
        Step::Ask => ask(env, &subject, target, &listings).await,
    }
}

pub async fn apply_filing(env: &FilingEnv<'_>, plan: &FilingPlan) -> Result<(), WorkerError> {
    match plan {
        FilingPlan::File {
            target: Target::Session(session),
            topic,
            ..
        } => {
            env.topics
                .store
                .place(Placement {
                    session: session.clone(),
                    topic: *topic,
                    by: PlacementSource::Categorizer,
                    at: env.now,
                })
                .await?;
        }
        FilingPlan::File {
            target: Target::Conversation(conversation),
            topic,
            ..
        } => {
            env.topics
                .filing
                .place_conversation(ConversationPlacement {
                    conversation: *conversation,
                    topic: *topic,
                    by: PlacementSource::Categorizer,
                    at: env.now,
                })
                .await?;
        }
        FilingPlan::Suggest {
            target: Target::Session(session),
            name,
            under,
        } => {
            env.topics
                .store
                .suggest(Suggestion {
                    session: session.clone(),
                    name: name.clone(),
                    topic: None,
                    parent: *under,
                })
                .await?;
        }
        FilingPlan::Suggest {
            target: Target::Conversation(_),
            ..
        }
        | FilingPlan::Never
        | FilingPlan::Placed(_)
        | FilingPlan::Inherits
        | FilingPlan::Refused { .. }
        | FilingPlan::Unfiled => {}
    }
    Ok(())
}

async fn filed_topic(
    env: &FilingEnv<'_>,
    target: &Target,
) -> Result<Option<(PlacementSource, TopicId)>, WorkerError> {
    Ok(match target {
        Target::Session(session) => env
            .topics
            .store
            .placement(session)
            .await?
            .map(|placement| (placement.by, placement.topic)),
        Target::Conversation(conversation) => env
            .topics
            .filing
            .conversation_placement(*conversation)
            .await?
            .map(|placement| (placement.by, placement.topic)),
    })
}

async fn child_session(env: &FilingEnv<'_>, filable: &Filable) -> Result<bool, WorkerError> {
    let Filable::Session(session) = filable else {
        return Ok(false);
    };
    let described = env
        .topics
        .lineage
        .describe(session.harness(), session.native(), None)
        .await?;
    Ok(described.is_some_and(|row| row.parent_session_id.is_some()))
}

async fn task_topic(
    env: &FilingEnv<'_>,
    filable: &Filable,
    conversation: Uuid,
) -> Result<Option<TopicId>, WorkerError> {
    let Filable::Task(task) = filable else {
        return Ok(None);
    };
    let Some(parent) = env.topics.filing.task_parent(*task, conversation).await? else {
        return Ok(None);
    };
    parent_topic(env, &parent).await
}

async fn parent_topic(
    env: &FilingEnv<'_>,
    parent: &ConversationHead,
) -> Result<Option<TopicId>, WorkerError> {
    let Some(target) = Filable::of(parent).target(parent.id) else {
        return Ok(None);
    };
    Ok(filed_topic(env, &target).await?.map(|(_, topic)| topic))
}

async fn ask(
    env: &FilingEnv<'_>,
    subject: &Subject,
    target: Target,
    listings: &[TopicListing],
) -> Result<FilingPlan, WorkerError> {
    let lines = env.topics.filing.topic_lines().await?;
    let corrections = env.topics.filing.corrections(CORRECTION_EXAMPLES).await?;
    let tree = prompt::Tree::of(listings, &lines);
    let reply = chat(env, &prompt::user_prompt(&tree, &corrections, subject)).await?;
    Ok(match parse::answer(&reply, &tree)? {
        Answer::File(topic) => FilingPlan::File {
            target,
            topic,
            rule: Rule::Model,
        },
        Answer::Suggest { name, under } => match existing(&name, listings) {
            Some(topic) => FilingPlan::File {
                target,
                topic,
                rule: Rule::Model,
            },
            None => match Refusal::of(&name, subject.head.title.as_deref()) {
                Some(refusal) => FilingPlan::Refused { name, refusal },
                None => FilingPlan::Suggest {
                    target,
                    name,
                    under,
                },
            },
        },
        Answer::Nothing => FilingPlan::Unfiled,
    })
}

fn existing(name: &TopicName, listings: &[TopicListing]) -> Option<TopicId> {
    let wanted = Slug::of_heading(name.as_str())?;
    listings
        .iter()
        .find(|listing| {
            listing.topic.slug == wanted
                || listing
                    .topic
                    .aliases
                    .iter()
                    .any(|alias| Slug::of_heading(alias.as_str()).as_ref() == Some(&wanted))
        })
        .map(|listing| listing.topic.id)
}

async fn chat(env: &FilingEnv<'_>, user: &str) -> Result<String, WorkerError> {
    cancelled(env)?;
    let request = ModelRequest {
        options: ModelOptions::default(),
        model: env.model.to_owned(),
        messages: vec![
            text_message(Role::System, FILING_SYSTEM_PROMPT),
            text_message(Role::User, user),
        ],
        tools: Vec::new(),
        thinking: None,
        max_tokens: Some(MAX_FILING_TOKENS),
    };
    let response = tokio::select! {
        biased;
        () = env.cancel.cancelled() => return Err(WorkerError::Cancelled),
        result = env.provider.chat(request) => result.map_err(llm_error)?,
    };
    match response.content {
        Some(text) if !text.trim().is_empty() => Ok(text),
        Some(_) | None => Err(WorkerError::Filing {
            message: "the model returned no filing answer".to_owned(),
        }),
    }
}

fn cancelled(env: &FilingEnv<'_>) -> Result<(), WorkerError> {
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

fn short(conversation: Uuid) -> String {
    conversation.simple().to_string().chars().take(8).collect()
}
