use kernel::filing::ConversationPlacement;
use kernel::filing::Filed;
use kernel::filing::FilingStore;
use kernel::memory::MemoryStore;
use kernel::topics::HistoryMark;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Suggestion;
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

const CURRENT: &str = "00000000-0000-4000-8000-000000000091";
const LEGACY: &str = "00000000-0000-4000-8000-000000000092";
const FORMING: &str = "00000000-0000-4000-8000-000000000093";
const LATER: &str = "00000000-0000-4000-8000-000000000094";

pub async fn every_topic_s_filing_reads_at_once<S>(store: &S) -> Outcome
where
    S: TopicStore + FilingStore + MemoryStore,
{
    let desktop = store.create(new_topic("Desktop App", &[], None)?).await?;
    let rivetos = store.create(new_topic("RivetOS", &[], None)?).await?;
    let current = session(CURRENT)?;
    let legacy = session(LEGACY)?;
    captured(store, current.as_str(), "claude-code", "Rail polish", 3).await?;
    captured(store, LEGACY, "claude-code", "Older capture", 1).await?;
    let qwen = captured(store, "qwen-code:91", "qwen-code", "Quantised bench", 2).await?;
    for (placed, topic, by) in [
        (&current, desktop.id, PlacementSource::Categorizer),
        (&legacy, rivetos.id, PlacementSource::You),
    ] {
        store
            .place(Placement {
                session: placed.clone(),
                topic,
                by,
                at: at(4),
                owner: Some(crate::owner()?),
            })
            .await?;
    }
    store
        .place_conversation(ConversationPlacement {
            conversation: qwen,
            topic: desktop.id,
            by: PlacementSource::Categorizer,
            at: at(5),
        })
        .await?;
    for (marked, minute) in [(&current, 6), (&legacy, 7)] {
        store
            .mark_history(HistoryMark {
                session: marked.clone(),
                at: at(minute),
            })
            .await?;
    }
    let all = store.all_filed().await?;
    let mut each = Filed::default();
    for topic in [desktop.id, rivetos.id] {
        let filed = store.filed(topic).await?;
        each.sessions.extend(filed.sessions);
        each.conversations.extend(filed.conversations);
    }
    each.sessions
        .sort_by_key(|filed| std::cmp::Reverse(filed.last_active));
    same("every filed session", all.sessions, each.sessions)?;
    same(
        "every filed conversation",
        all.conversations,
        each.conversations,
    )?;
    same("the history marks", all.history, store.history().await?)?;
    suggestions_of_unplaced_sessions(store).await
}

async fn suggestions_of_unplaced_sessions<S>(store: &S) -> Outcome
where
    S: TopicStore,
{
    let forming = session(FORMING)?;
    let later = session(LATER)?;
    for (suggested, name) in [(&later, "Release Tooling"), (&forming, "Mesh Certificates")] {
        store
            .suggest(Suggestion {
                session: suggested.clone(),
                name: TopicName::new(name)?,
                topic: None,
                parent: None,
            })
            .await?;
    }
    let mut each = Vec::new();
    for suggested in [&forming, &later] {
        each.extend(store.suggestion(suggested).await?);
    }
    same(
        "every unplaced session's suggestion",
        store.suggestions().await?,
        each,
    )?;
    Ok(())
}

async fn captured<S: MemoryStore>(
    store: &S,
    key: &str,
    channel: &str,
    title: &str,
    minute: u32,
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
                created_at: Some(at(minute)),
                updated_at: Some(at(minute)),
                messages: vec![CaptureMessage {
                    event_id: format!("{key}-1"),
                    role: Role::User,
                    content: title.to_owned(),
                    tool_name: None,
                    tool_args: None,
                    tool_result: None,
                    metadata: None,
                    created_at: Some(at(minute)),
                }],
            },
            &crate::owner()?,
            Some("node-a"),
        )
        .await?;
    Ok(Uuid::parse_str(&captured.conversation_id)?)
}
