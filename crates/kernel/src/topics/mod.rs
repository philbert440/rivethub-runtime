mod categorize;
mod page;
mod placement;
mod rules;
mod search;
mod values;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use wire::gateway::wiki::WikiSlug;
use wire::session_id::SessionId;

use crate::BoxError;

pub use categorize::FirstLeaf;
pub use categorize::Lineage;
pub use categorize::Verdict;
pub use page::PageSection;
pub use page::SyncTopicPage;
pub use page::TopicPage;
pub use page::TopicPageError;
pub use page::TopicPageStore;
pub use page::bound_page;
pub(crate) use page::root_page;
pub use page::sync_topic_page;
pub use placement::PlacedSession;
pub use placement::Placement;
pub use placement::PlacementOutcome;
pub use placement::PlacementSource;
pub use placement::Suggestion;
pub use rules::MergePlan;
pub use rules::check_parent;
pub use search::TopicKey;
pub use search::TopicQuery;
pub use search::by_activity;
pub use search::rank;
pub use values::Alias;
pub use values::Change;
pub use values::NewTopic;
pub use values::Redirect;
pub use values::Root;
pub use values::RootKind;
pub use values::Slug;
pub use values::Topic;
pub use values::TopicChanges;
pub use values::TopicId;
pub use values::TopicListing;
pub use values::TopicName;
pub use values::distinct_aliases;

#[derive(Debug, Error)]
pub enum TopicError {
    #[error("a topic name must not be empty")]
    EmptyName,
    #[error("an alias must not be empty")]
    EmptyAlias,
    #[error("\"{text}\" has no letters or digits to make a slug from")]
    EmptySlug { text: String },
    #[error("a topic with slug \"{slug}\" already exists")]
    SlugTaken { slug: Slug },
    #[error("alias \"{alias}\" belongs to another topic")]
    AliasTaken { alias: String },
    #[error("unknown topic {key}")]
    NotFound { key: String },
    #[error("a topic cannot be its own parent")]
    OwnParent,
    #[error("topic \"{parent}\" is a sub-topic and cannot have sub-topics")]
    ParentIsSubtopic { parent: Slug },
    #[error("a topic with sub-topics cannot become a sub-topic")]
    HasSubtopics,
    #[error("a topic cannot be merged into itself")]
    SelfMerge,
    #[error("topic \"{into}\" is a sub-topic and cannot take over sub-topics")]
    MergeWouldNest { into: Slug },
    #[error("topic store is unavailable")]
    Unavailable(#[source] BoxError),
}

#[async_trait]
pub trait TopicStore: Send + Sync {
    async fn create(&self, topic: Topic) -> Result<Topic, TopicError>;

    async fn update(
        &self,
        id: TopicId,
        changes: TopicChanges,
        now: DateTime<Utc>,
    ) -> Result<Topic, TopicError>;

    async fn merge(
        &self,
        source: TopicId,
        into: TopicId,
        now: DateTime<Utc>,
    ) -> Result<Topic, TopicError>;

    async fn find(&self, key: &TopicKey) -> Result<Option<Topic>, TopicError>;

    async fn list(&self) -> Result<Vec<TopicListing>, TopicError>;

    async fn redirects(&self) -> Result<Vec<Redirect>, TopicError>;

    async fn place(&self, placement: Placement) -> Result<PlacementOutcome, TopicError>;

    async fn unplace(&self, session: &SessionId) -> Result<Option<Placement>, TopicError>;

    async fn placement(&self, session: &SessionId) -> Result<Option<Placement>, TopicError>;

    async fn sessions(&self, topic: TopicId) -> Result<Vec<PlacedSession>, TopicError>;

    async fn suggest(&self, suggestion: Suggestion) -> Result<(), TopicError>;

    async fn suggestion(&self, session: &SessionId) -> Result<Option<Suggestion>, TopicError>;

    async fn page_slug(&self, topic: TopicId) -> Result<Option<WikiSlug>, TopicError>;

    async fn search(
        &self,
        query: &TopicQuery,
        limit: usize,
    ) -> Result<Vec<TopicListing>, TopicError> {
        let listings = self.list().await?;
        let redirects = self.redirects().await?;
        Ok(rank(listings, &redirects, query, limit))
    }
}
