use kernel::filing::ConversationPlacement;
use kernel::filing::Correction;
use kernel::filing::FilingStore;
use kernel::filing::Target;
use kernel::memory::CaptureBatchRequest;
use kernel::memory::CaptureMessage;
use kernel::memory::MemoryStore;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::TopicId;
use kernel::topics::TopicName;
use kernel::topics::TopicStore;
use uuid::Uuid;
use wire::message::Role;

use crate::topics::Outcome;
use crate::topics::at;
use crate::topics::new_topic;
use crate::topics::same;
use crate::topics::session;

const CLAUDE: &str = "00000000-0000-4000-8000-000000000031";
const BARE: &str = "00000000-0000-4000-8000-000000000032";
const NODE_A: &str = "node-a";
const NODE_B: &str = "node-b";
const THREE_DAYS: u32 = 3 * 24 * 60;

pub async fn conversations_file_with_titles_and_corrections<S>(store: &S) -> Outcome
where
    S: TopicStore + FilingStore + MemoryStore,
{
    let desktop = store.create(new_topic("Desktop App", &[], None)?).await?;
    let rivetos = store.create(new_topic("RivetOS", &[], None)?).await?;
    let current = session(CLAUDE)?;
    let legacy = session(BARE)?;
    capture(
        store,
        current.as_str(),
        "claude-code",
        "Topic screen",
        Some(NODE_A),
    )
    .await?;
    capture(store, BARE, "claude-code", "Old claude", None).await?;
    let qwen = capture(store, "qwen-code:41", "qwen-code", "Qwen run", Some(NODE_B)).await?;
    for filed in [&current, &legacy] {
        store
            .place(Placement {
                session: filed.clone(),
                topic: desktop.id,
                by: PlacementSource::Categorizer,
                at: at(1),
            })
            .await?;
    }
    let filed = |topic: TopicId, by, minute| ConversationPlacement {
        conversation: qwen,
        topic,
        by,
        at: at(minute),
    };
    store
        .place_conversation(filed(rivetos.id, PlacementSource::Categorizer, 2))
        .await?;
    let yours = store
        .place_conversation(filed(desktop.id, PlacementSource::You, 3))
        .await?;
    same(
        "the categorizer leaves your filing",
        store
            .place_conversation(filed(rivetos.id, PlacementSource::Categorizer, 4))
            .await?,
        yours.clone(),
    )?;
    listed_with_titles(store, desktop.id).await?;
    corrections_follow_the_tree(store, qwen, desktop.id, rivetos.id).await
}

async fn listed_with_titles<S: FilingStore>(store: &S, desktop: TopicId) -> Outcome {
    let listed = store.filed(desktop).await?;
    let mut titles: Vec<(Option<String>, Option<String>)> = listed
        .sessions
        .iter()
        .map(|row| (row.title.clone(), row.node.clone()))
        .collect();
    titles.sort();
    same(
        "a legacy key still names its session, and each says where it was captured",
        titles,
        vec![
            (Some("Old claude".to_owned()), None),
            (Some("Topic screen".to_owned()), Some(NODE_A.to_owned())),
        ],
    )?;
    same(
        "the conversation row",
        listed
            .conversations
            .iter()
            .map(|row| {
                (
                    row.channel.as_str(),
                    row.title.as_deref(),
                    row.node.as_deref(),
                )
            })
            .collect::<Vec<_>>(),
        vec![("qwen-code", Some("Qwen run"), Some(NODE_B))],
    )?;
    Ok(())
}

async fn corrections_follow_the_tree<S: TopicStore + FilingStore>(
    store: &S,
    qwen: Uuid,
    desktop: TopicId,
    rivetos: TopicId,
) -> Outcome {
    store
        .correct(Correction {
            target: Target::Conversation(qwen),
            from: rivetos,
            to: Some(desktop),
            at: at(3),
        })
        .await?;
    let examples = store.corrections(8).await?;
    same(
        "the correction reads back with names",
        examples
            .iter()
            .map(|example| {
                (
                    example.title.as_deref(),
                    example.from.as_str(),
                    example.to.as_ref().map(TopicName::as_str),
                )
            })
            .collect::<Vec<_>>(),
        vec![(Some("Qwen run"), "RivetOS", Some("Desktop App"))],
    )?;
    store.merge(desktop, rivetos, at(5)).await?;
    same(
        "a merge moves the conversation",
        store
            .conversation_placement(qwen)
            .await?
            .map(|placed| placed.topic),
        Some(rivetos),
    )?;
    store.delete(rivetos).await?;
    same(
        "deleting the topic drops its filings and corrections",
        (
            store.conversation_placement(qwen).await?,
            store.corrections(8).await?.len(),
        ),
        (None, 0),
    )?;
    Ok(())
}

pub async fn a_task_files_under_the_conversation_that_started_it<S>(store: &S) -> Outcome
where
    S: FilingStore + MemoryStore,
{
    let task = Uuid::parse_str("00000000-0000-4000-8000-0000000000a1")?;
    let said = |role, content: String, result: Option<String>, minute| CaptureMessage {
        event_id: format!("task-mention-{minute}"),
        role,
        content,
        tool_name: None,
        tool_args: None,
        tool_result: result,
        metadata: None,
        created_at: Some(at(minute)),
    };
    record(
        store,
        "claude-code:00000000-0000-4000-8000-000000000051",
        said(Role::User, format!("Is task {task} done?"), None, 0),
    )
    .await?;
    let started = record(
        store,
        "claude-code:00000000-0000-4000-8000-000000000052",
        said(
            Role::Tool,
            "Delegated.".to_owned(),
            Some(format!("{{\"taskId\":\"{task}\"}}")),
            10,
        ),
    )
    .await?;
    let own = record(
        store,
        &format!("task:{task}"),
        said(
            Role::User,
            "Run the release checks.".to_owned(),
            None,
            THREE_DAYS,
        ),
    )
    .await?;
    same(
        "the conversation whose tool output names the task",
        store.task_parent(task, own).await?.map(|head| head.id),
        Some(started),
    )?;
    Ok(())
}

async fn record<S: MemoryStore>(
    store: &S,
    key: &str,
    message: CaptureMessage,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let created = message.created_at;
    let captured = store
        .capture(
            CaptureBatchRequest {
                session_key: key.to_owned(),
                agent: "rivet".to_owned(),
                channel: Some("claude-code".to_owned()),
                title: None,
                settings: None,
                task_id: None,
                finalize: None,
                created_at: created,
                updated_at: created,
                messages: vec![message],
            },
            None,
        )
        .await?;
    Ok(Uuid::parse_str(&captured.conversation_id)?)
}

pub async fn capture<S: MemoryStore>(
    store: &S,
    key: &str,
    channel: &str,
    title: &str,
    node: Option<&str>,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let captured = store
        .capture(
            CaptureBatchRequest {
                session_key: key.to_owned(),
                agent: "rivet".to_owned(),
                channel: Some(channel.to_owned()),
                title: Some(title.to_owned()),
                settings: None,
                task_id: None,
                finalize: None,
                created_at: Some(at(0)),
                updated_at: Some(at(0)),
                messages: vec![CaptureMessage {
                    event_id: format!("{key}-1"),
                    role: Role::User,
                    content: title.to_owned(),
                    tool_name: None,
                    tool_args: None,
                    tool_result: None,
                    metadata: None,
                    created_at: Some(at(0)),
                }],
            },
            node,
        )
        .await?;
    Ok(Uuid::parse_str(&captured.conversation_id)?)
}
