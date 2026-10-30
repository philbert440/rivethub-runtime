mod entity;
mod filing;
mod fold;
mod names;
mod snapshot;
mod tree;

use async_trait::async_trait;
use thiserror::Error;

use crate::BoxError;

pub use entity::EntityType;
pub use filing::Desk;
pub use filing::FiledHow;
pub use filing::Filing;
pub use filing::Grouped;
pub use filing::Turn;
pub use filing::filed_counts;
pub use filing::group;
pub use fold::Fate;
pub use fold::FoldRule;
pub use fold::Folded;
pub use fold::Settled;
pub use fold::cited_topics;
pub use fold::settle_pages;
pub use names::Junk;
pub use names::JunkReason;
pub use names::JunkStage;
pub use names::NameGuard;
pub use names::near_duplicate;
pub use snapshot::EntitySection;
pub use snapshot::KnowledgePage;
pub use snapshot::Project;
pub use snapshot::Snapshot;
pub use snapshot::SnapshotConversation;
pub use snapshot::SnapshotCorrection;
pub use snapshot::SnapshotHead;
pub use snapshot::SnapshotPlacement;
pub use snapshot::SnapshotRedirect;
pub use snapshot::SnapshotRoot;
pub use snapshot::SnapshotTopic;
pub use tree::DraftTopic;
pub use tree::Lift;
pub use tree::MIN_SESSIONS;
pub use tree::Origin;
pub use tree::ProposedTree;
pub use tree::TreeMerge;
pub use tree::TreeTopic;

#[derive(Debug, Error)]
pub enum CleanupError {
    #[error("the memory snapshot could not be read: {0}")]
    Snapshot(#[source] BoxError),
}

#[async_trait]
pub trait CleanupSource: Send + Sync {
    async fn snapshot(&self) -> Result<Snapshot, CleanupError>;
}
