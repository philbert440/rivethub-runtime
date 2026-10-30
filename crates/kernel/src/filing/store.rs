use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use uuid::Uuid;

use super::Correction;
use super::CorrectionExample;
use super::Target;
use crate::memory::ConversationHead;
use crate::topics::Placement;
use crate::topics::PlacementSource;
use crate::topics::TopicError;
use crate::topics::TopicId;

const TOPIC_LINE_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub head: ConversationHead,
    pub cwd: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationPlacement {
    pub conversation: Uuid,
    pub topic: TopicId,
    pub by: PlacementSource,
    pub at: DateTime<Utc>,
}

impl ConversationPlacement {
    #[must_use]
    pub fn admit(self, existing: Option<Self>) -> Self {
        match existing {
            Some(current) if current.by.keeps(self.by) => current,
            Some(_) | None => self,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiledSession {
    pub placement: Placement,
    pub title: Option<String>,
    pub last_active: Option<DateTime<Utc>>,
    pub node: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiledConversation {
    pub placement: ConversationPlacement,
    pub title: Option<String>,
    pub channel: String,
    pub last_active: Option<DateTime<Utc>>,
    pub node: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filed {
    pub sessions: Vec<FiledSession>,
    pub conversations: Vec<FiledConversation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicLine {
    pub topic: TopicId,
    pub line: String,
}

impl TopicLine {
    #[must_use]
    pub fn from_page(topic: TopicId, page: &str) -> Option<Self> {
        let line = page
            .lines()
            .map(|line| line.trim().trim_start_matches(['#', '-', '*', '>']).trim())
            .find(|line| !line.is_empty())?;
        Some(Self {
            topic,
            line: line.chars().take(TOPIC_LINE_CHARS).collect(),
        })
    }
}

#[async_trait]
pub trait FilingStore: Send + Sync {
    async fn subject(&self, conversation: Uuid) -> Result<Option<Subject>, TopicError>;

    async fn task_parent(
        &self,
        task: Uuid,
        conversation: Uuid,
    ) -> Result<Option<ConversationHead>, TopicError>;

    async fn conversation_placement(
        &self,
        conversation: Uuid,
    ) -> Result<Option<ConversationPlacement>, TopicError>;

    async fn place_conversation(
        &self,
        placement: ConversationPlacement,
    ) -> Result<ConversationPlacement, TopicError>;

    async fn unplace_conversation(
        &self,
        conversation: Uuid,
    ) -> Result<Option<ConversationPlacement>, TopicError>;

    async fn filed(&self, topic: TopicId) -> Result<Filed, TopicError>;

    async fn correct(&self, correction: Correction) -> Result<(), TopicError>;

    async fn corrections(&self, limit: u32) -> Result<Vec<CorrectionExample>, TopicError>;

    async fn corrected(&self, target: &Target) -> Result<bool, TopicError>;

    async fn topic_lines(&self) -> Result<Vec<TopicLine>, TopicError>;
}
