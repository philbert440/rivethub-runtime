use std::str::FromStr;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use kernel::memory::ConversationHead;
use kernel::topics::FirstLeaf;
use kernel::topics::Lineage;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Suggestion;
use kernel::topics::TopicError;
use kernel::topics::TopicStore;
use kernel::topics::Verdict;
use kernel::transcript::SessionLister;
use kernel::transcript::TranscriptReaderError;
use wire::session_id::SessionId;

#[expect(
    missing_debug_implementations,
    reason = "the topic store and the session lister are trait objects"
)]
#[derive(Clone)]
pub struct SessionTopics {
    pub store: Arc<dyn TopicStore>,
    pub lineage: Arc<dyn SessionLister>,
}

#[derive(Debug, thiserror::Error)]
enum CategorizeError {
    #[error(transparent)]
    Topics(#[from] TopicError),
    #[error(transparent)]
    Lineage(#[from] TranscriptReaderError),
}

pub(super) async fn best_effort_categorize(
    topics: &SessionTopics,
    head: &ConversationHead,
    summary: &str,
    now: DateTime<Utc>,
) {
    #[cfg_attr(
        dylint_lib = "rr_lints",
        expect(
            fallback,
            reason = "PATTERNS FALLBACK: a conversation whose session key is not a canonical session id has no topic placement"
        )
    )]
    let Ok(session) = SessionId::from_str(&head.session_key) else {
        return;
    };
    match categorize(topics, &session, head.title.as_deref(), summary, now).await {
        Ok(verdict) => tracing::debug!("topics: {session} {verdict:?}"),
        Err(error) => tracing::warn!("topics: {session} was not categorized: {error}"),
    }
}

async fn categorize(
    topics: &SessionTopics,
    session: &SessionId,
    title: Option<&str>,
    summary: &str,
    now: DateTime<Utc>,
) -> Result<Verdict, CategorizeError> {
    let placement = topics.store.placement(session).await?;
    let described = topics
        .lineage
        .describe(session.harness(), session.native(), None)
        .await?;
    let lineage = match described.and_then(|row| row.parent_session_id) {
        Some(_) => Lineage::Child,
        None => Lineage::Root,
    };
    let listings = topics.store.list().await?;
    let redirects = topics.store.redirects().await?;
    let verdict = FirstLeaf {
        placement: placement.as_ref(),
        lineage,
        title,
        summary,
    }
    .categorize(&listings, &redirects)?;
    match &verdict {
        Verdict::Place(topic) => {
            topics
                .store
                .place(Placement {
                    session: session.clone(),
                    topic: *topic,
                    by: PlacementSource::Categorizer,
                    at: now,
                })
                .await?;
        }
        Verdict::Suggest { name, topic } => {
            topics
                .store
                .suggest(Suggestion {
                    session: session.clone(),
                    name: name.clone(),
                    topic: *topic,
                })
                .await?;
        }
        Verdict::AlreadyPlaced | Verdict::InheritsParent | Verdict::Untitled => {}
    }
    Ok(verdict)
}
