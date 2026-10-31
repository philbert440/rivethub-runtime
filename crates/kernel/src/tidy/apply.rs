use chrono::DateTime;
use chrono::Utc;

use super::BatchId;
use super::Proposal;
use super::ProposalId;
use super::ProposalState;
use super::TidyError;
use super::TidyStore;
use super::TreeEdit;
use crate::filing::ConversationPlacement;
use crate::filing::FilingStore;
use crate::filing::Target;
use crate::owner::Owner;
use crate::topics::Change;
use crate::topics::NewTopic;
use crate::topics::Placement;
use crate::topics::PlacementSource;
use crate::topics::Topic;
use crate::topics::TopicChanges;
use crate::topics::TopicId;
use crate::topics::TopicKey;
use crate::topics::TopicKind;
use crate::topics::TopicName;
use crate::topics::TopicPageStore;
use crate::topics::TopicStore;
use crate::wiki::page::FoldMark;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Reject,
}

#[expect(
    missing_debug_implementations,
    reason = "the tidy, topic and filing ports are trait objects"
)]
pub struct TidyPorts<'a> {
    pub tidy: &'a dyn TidyStore,
    pub topics: &'a dyn TopicStore,
    pub filing: &'a dyn FilingStore,
    pub pages: Option<&'a dyn TopicPageStore>,
    pub owner: &'a Owner,
}

#[derive(Debug)]
pub struct BatchOutcome {
    pub decided: Vec<Proposal>,
    pub failed: Vec<(ProposalId, TidyError)>,
}

pub async fn approve(
    ports: &TidyPorts<'_>,
    id: ProposalId,
    now: DateTime<Utc>,
) -> Result<Proposal, TidyError> {
    let proposal = ports
        .tidy
        .proposal(id)
        .await?
        .ok_or(TidyError::UnknownProposal { id })?;
    approve_pending(ports, &proposal, now).await
}

pub async fn reject(
    tidy: &dyn TidyStore,
    id: ProposalId,
    now: DateTime<Utc>,
) -> Result<Proposal, TidyError> {
    tidy.decide(id, ProposalState::Rejected, now).await
}

pub async fn decide_batch(
    ports: &TidyPorts<'_>,
    id: BatchId,
    decision: Decision,
    now: DateTime<Utc>,
) -> Result<BatchOutcome, TidyError> {
    let batch = ports
        .tidy
        .batch(id)
        .await?
        .ok_or(TidyError::UnknownBatch { id })?;
    let mut outcome = BatchOutcome {
        decided: Vec::new(),
        failed: Vec::new(),
    };
    for proposal in batch
        .proposals
        .into_iter()
        .filter(|proposal| proposal.state == ProposalState::Pending)
    {
        let decided = match decision {
            Decision::Approve => approve_pending(ports, &proposal, now).await,
            Decision::Reject => reject(ports.tidy, proposal.id, now).await,
        };
        match decided {
            Ok(done) => outcome.decided.push(done),
            Err(error) => outcome.failed.push((proposal.id, error)),
        }
    }
    Ok(outcome)
}

async fn approve_pending(
    ports: &TidyPorts<'_>,
    proposal: &Proposal,
    now: DateTime<Utc>,
) -> Result<Proposal, TidyError> {
    proposal.pending()?;
    match apply(ports, &proposal.change, now).await {
        Ok(()) => {}
        Err(error) if stale(&error) => {
            ports
                .tidy
                .decide(proposal.id, ProposalState::Rejected, now)
                .await?;
            return Err(TidyError::Stale {
                id: proposal.id,
                reason: error.to_string(),
            });
        }
        Err(error) => return Err(error),
    }
    ports
        .tidy
        .decide(proposal.id, ProposalState::Approved, now)
        .await
}

const fn stale(error: &TidyError) -> bool {
    matches!(
        error,
        TidyError::Wiki(crate::wiki::builder::WikiStoreError::TopicMissing(_))
            | TidyError::Topic(crate::topics::TopicError::NotFound { .. })
    )
}

async fn apply(
    ports: &TidyPorts<'_>,
    change: &TreeEdit,
    now: DateTime<Utc>,
) -> Result<(), TidyError> {
    let topics = ports.topics;
    match change {
        TreeEdit::Merge { source, into } => {
            if !merged(topics, *source, *into).await? {
                topics.merge(*source, *into, now).await?;
            }
        }
        TreeEdit::Move { topic, parent } => {
            let changes = TopicChanges {
                parent: parent.map_or(Change::Clear, Change::Set),
                ..TopicChanges::default()
            };
            topics.update(*topic, changes, now).await?;
        }
        TreeEdit::Rename { topic, name } => {
            let changes = TopicChanges {
                name: Some(name.clone()),
                ..TopicChanges::default()
            };
            topics.update(*topic, changes, now).await?;
        }
        TreeEdit::Close { topic } => {
            topics.set_closed(*topic, Some(now), now).await?;
        }
        TreeEdit::Delete { topic } => {
            if exists(topics, *topic).await? {
                topics.delete(*topic).await?;
            }
        }
        TreeEdit::CreateTopic {
            topic,
            name,
            parent,
            aliases,
            members,
        } => {
            let new = NewTopic {
                kind: TopicKind::Work,
                name: name.clone(),
                aliases: aliases.clone(),
                parent: *parent,
                directory: None,
                owner: Some(ports.owner.clone()),
            };
            if !exists(topics, *topic).await? {
                topics.create(Topic::create(*topic, new, now)?).await?;
            }
            for member in members {
                file(ports, member, *topic, now).await?;
            }
        }
        TreeEdit::FileInto { topic, members } => {
            for member in members {
                file(ports, member, *topic, now).await?;
            }
        }
        TreeEdit::MergePage { page, into } => {
            ports
                .pages
                .ok_or(TidyError::PagesUnavailable)?
                .fold_page(page, into, FoldMark::Merged, now)
                .await?;
        }
        TreeEdit::RemovePage { page } => {
            ports
                .pages
                .ok_or(TidyError::PagesUnavailable)?
                .remove_page(page)
                .await?;
        }
        TreeEdit::AddType {
            topic,
            name,
            parent,
        } => create(topics, *topic, TopicKind::Type, name, *parent, now).await?,
    }
    Ok(())
}

async fn exists(topics: &dyn TopicStore, id: TopicId) -> Result<bool, TidyError> {
    Ok(topics.find(&TopicKey::Id(id)).await?.is_some())
}

async fn merged(
    topics: &dyn TopicStore,
    source: TopicId,
    into: TopicId,
) -> Result<bool, TidyError> {
    if exists(topics, source).await? || !exists(topics, into).await? {
        return Ok(false);
    }
    Ok(topics
        .redirects()
        .await?
        .iter()
        .any(|redirect| redirect.topic == into))
}

async fn create(
    topics: &dyn TopicStore,
    id: TopicId,
    kind: TopicKind,
    name: &TopicName,
    parent: Option<TopicId>,
    now: DateTime<Utc>,
) -> Result<(), TidyError> {
    if exists(topics, id).await? {
        return Ok(());
    }
    let new = NewTopic {
        kind,
        name: name.clone(),
        aliases: Vec::new(),
        parent,
        directory: None,
        owner: None,
    };
    topics.create(Topic::create(id, new, now)?).await?;
    Ok(())
}

async fn file(
    ports: &TidyPorts<'_>,
    member: &Target,
    topic: TopicId,
    now: DateTime<Utc>,
) -> Result<(), TidyError> {
    match member {
        Target::Session(session) => {
            ports
                .topics
                .place(Placement {
                    session: session.clone(),
                    topic,
                    by: PlacementSource::Categorizer,
                    at: now,
                    owner: Some(ports.owner.clone()),
                })
                .await?;
        }
        Target::Conversation(conversation) => {
            ports
                .filing
                .place_conversation(ConversationPlacement {
                    conversation: *conversation,
                    topic,
                    by: PlacementSource::Categorizer,
                    at: now,
                })
                .await?;
        }
    }
    Ok(())
}
