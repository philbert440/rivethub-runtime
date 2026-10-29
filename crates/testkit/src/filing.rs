use kernel::filing::ConversationPlacement;
use kernel::filing::Correction;
use kernel::filing::FilingStore;
use kernel::filing::Target;
use kernel::memory::MemoryStore;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::TopicId;
use kernel::topics::TopicName;
use kernel::topics::TopicStore;
use uuid::Uuid;
use wire::capture::CaptureBatchRequest;
use wire::capture::CaptureMessage;
use wire::message::Role;

use crate::topics::Outcome;
use crate::topics::at;
use crate::topics::new_topic;
use crate::topics::same;
use crate::topics::session;

const CLAUDE: &str = "00000000-0000-4000-8000-000000000031";
const BARE: &str = "00000000-0000-4000-8000-000000000032";

pub async fn conversations_file_with_titles_and_corrections<S>(store: &S) -> Outcome
where
    S: TopicStore + FilingStore + MemoryStore,
{
    let desktop = store.create(new_topic("Desktop App", &[], None)?).await?;
    let rivetos = store.create(new_topic("RivetOS", &[], None)?).await?;
    let current = session(CLAUDE)?;
    let legacy = session(BARE)?;
    capture(store, current.as_str(), "claude-code", "Topic screen").await?;
    capture(store, BARE, "claude-code", "Old claude").await?;
    let qwen = capture(store, "qwen-code:41", "qwen-code", "Qwen run").await?;
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
    let titles: Vec<Option<String>> = listed
        .sessions
        .iter()
        .map(|row| row.title.clone())
        .collect();
    same("both sessions are listed", titles.len(), 2)?;
    same(
        "a legacy key still names its session",
        titles.contains(&Some("Old claude".to_owned()))
            && titles.contains(&Some("Topic screen".to_owned())),
        true,
    )?;
    same(
        "the conversation row",
        listed
            .conversations
            .iter()
            .map(|row| (row.channel.as_str(), row.title.as_deref()))
            .collect::<Vec<_>>(),
        vec![("qwen-code", Some("Qwen run"))],
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

async fn capture<S: MemoryStore>(
    store: &S,
    key: &str,
    channel: &str,
    title: &str,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let captured = store
        .capture(CaptureBatchRequest {
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
        })
        .await?;
    Ok(Uuid::parse_str(&captured.conversation_id)?)
}
