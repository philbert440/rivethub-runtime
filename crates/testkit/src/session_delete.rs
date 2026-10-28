use std::collections::BTreeSet;

use kernel::memory::MemoryStore;
use kernel::memory::SessionActivity;
use kernel::memory::SessionDeletion;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::TopicStore;
use wire::capture::CaptureBatchRequest;
use wire::capture::CaptureMessage;
use wire::memory::MemoryEntry;
use wire::message::Role;
use wire::session_id::SessionId;

use crate::topics::Outcome;
use crate::topics::at;
use crate::topics::new_topic;
use crate::topics::same;
use crate::topics::session;

pub const DELETED: &str = "00000000-0000-4000-8000-0000000000d1";
pub const SIBLING: &str = "00000000-0000-4000-8000-0000000000d2";

#[must_use]
pub fn message(event_id: &str, role: Role, content: &str) -> CaptureMessage {
    CaptureMessage {
        event_id: event_id.to_owned(),
        role,
        content: content.to_owned(),
        tool_name: None,
        tool_args: None,
        tool_result: None,
        metadata: None,
        created_at: Some(at(1)),
    }
}

#[must_use]
pub fn batch(
    session: &SessionId,
    agent: &str,
    messages: Vec<CaptureMessage>,
) -> CaptureBatchRequest {
    CaptureBatchRequest {
        session_key: session.as_str().to_owned(),
        agent: agent.to_owned(),
        channel: Some("cli".to_owned()),
        title: Some("A session".to_owned()),
        settings: None,
        task_id: None,
        finalize: None,
        created_at: Some(at(1)),
        updated_at: Some(at(2)),
        messages,
    }
}

#[must_use]
pub fn exchange(prefix: &str) -> Vec<CaptureMessage> {
    vec![
        message(
            &format!("{prefix}-1"),
            Role::User,
            "deploy the release branch",
        ),
        message(
            &format!("{prefix}-2"),
            Role::Assistant,
            "Deployed build 42.",
        ),
    ]
}

pub async fn seed(
    memory: &dyn MemoryStore,
) -> Result<(SessionId, SessionId), Box<dyn std::error::Error>> {
    let deleted = session(DELETED)?;
    let sibling = session(SIBLING)?;
    memory
        .capture(batch(&deleted, "opus", exchange("a")))
        .await?;
    memory
        .capture(batch(&deleted, "sonnet", exchange("b")))
        .await?;
    memory
        .capture(batch(&sibling, "opus", exchange("c")))
        .await?;
    Ok((deleted, sibling))
}

pub async fn capture_for_a_deleted_session_writes_nothing(
    memory: &dyn MemoryStore,
    deletions: &dyn SessionDeletion,
) -> Outcome {
    let (deleted, sibling) = seed(memory).await?;
    let erased = deletions
        .delete_session(&deleted, Some("user-1"), at(5))
        .await?;
    same("conversations across agents", erased.conversations, 2)?;
    same("messages across agents", erased.messages, 4)?;
    same(
        "tombstoned",
        deletions
            .tombstoned(&[deleted.clone(), sibling.clone()])
            .await?,
        BTreeSet::from([deleted.clone()]),
    )?;
    let resent = memory
        .capture(batch(&deleted, "opus", exchange("again")))
        .await?;
    same("capture inserts nothing", resent.inserted, 0)?;
    same("capture reports the batch skipped", resent.skipped, 2)?;
    let appended = memory
        .append(vec![MemoryEntry {
            id: Some("append-1".to_owned()),
            session_id: deleted.as_str().to_owned(),
            agent: "opus".to_owned(),
            channel: "cli".to_owned(),
            role: Role::User,
            content: "one more".to_owned(),
            tool_name: None,
            tool_args: None,
            tool_result: None,
            metadata: None,
            created_at: Some(at(6)),
        }])
        .await?;
    same("append inserts nothing", appended, 0)?;
    let activity = deletions
        .session_activity(&[deleted.clone(), sibling.clone()])
        .await?;
    same(
        "deleted session has no rows",
        activity.get(&deleted).cloned(),
        Some(SessionActivity::default()),
    )?;
    same(
        "sibling keeps its rows",
        activity.get(&sibling).map(|entry| entry.messages),
        Some(2),
    )?;
    let sibling_more = memory
        .capture(batch(&sibling, "opus", exchange("d")))
        .await?;
    same("sibling still captures", sibling_more.inserted, 2)?;
    Ok(())
}

pub async fn activity_counts_turns_tools_and_placements(
    memory: &dyn MemoryStore,
    deletions: &dyn SessionDeletion,
    topics: &dyn TopicStore,
) -> Outcome {
    let quiet = session("00000000-0000-4000-8000-0000000000a1")?;
    let busy = session("00000000-0000-4000-8000-0000000000a2")?;
    let placed = session("00000000-0000-4000-8000-0000000000a3")?;
    let absent = session("00000000-0000-4000-8000-0000000000a4")?;
    memory
        .capture(batch(
            &quiet,
            "opus",
            vec![
                message("q-1", Role::User, "hi"),
                message("q-0", Role::User, "  \n "),
                message("q-2", Role::Assistant, "Hello!"),
            ],
        ))
        .await?;
    let mut tool = message("b-2", Role::Tool, "");
    tool.tool_name = Some("Bash".to_owned());
    tool.tool_result = Some("ok".to_owned());
    memory
        .capture(batch(
            &busy,
            "opus",
            vec![message("b-1", Role::User, "list files"), tool],
        ))
        .await?;
    memory
        .capture(batch(&placed, "opus", exchange("p")))
        .await?;
    let topic = topics.create(new_topic("Kept", &[], None)?).await?;
    topics
        .place(Placement {
            session: placed.clone(),
            topic: topic.id,
            by: PlacementSource::You,
            at: at(3),
        })
        .await?;
    let activity = deletions
        .session_activity(&[quiet.clone(), busy.clone(), placed.clone(), absent.clone()])
        .await?;
    same(
        "quiet",
        activity.get(&quiet).cloned(),
        Some(SessionActivity {
            conversations: 1,
            messages: 3,
            user_messages: 1,
            assistant_messages: 1,
            reply: Some("Hello!".to_owned()),
            ..SessionActivity::default()
        }),
    )?;
    same(
        "busy has a tool row",
        activity.get(&busy).map(|entry| entry.tool_rows),
        Some(1),
    )?;
    same(
        "placed",
        activity.get(&placed).map(|entry| entry.placed),
        Some(true),
    )?;
    same(
        "absent",
        activity.get(&absent).cloned(),
        Some(SessionActivity::default()),
    )?;
    Ok(())
}
