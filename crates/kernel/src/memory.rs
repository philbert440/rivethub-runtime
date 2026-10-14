use std::collections::BTreeMap;
use std::fmt::Display;
use std::fmt::Formatter;
use std::ops::RangeInclusive;
use std::str::FromStr;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use uuid::Uuid;
use wire::gateway::memory::BrowseWindow;
use wire::gateway::memory::MemoryBrowseOrder;
use wire::gateway::memory::MemoryScope;
use wire::gateway::memory::MemoryStatsResponse;
use wire::gateway::memory::PendingTagWire;
use wire::gateway::memory::TagCount;
use wire::gateway::memory::TagEntityType;
use wire::gateway::memory::TagEntryPair;
use wire::gateway::memory::TagState;
use wire::gateway::memory::TagTaxonomyEntry;
use wire::gateway::memory::TagTaxonomyMerge;
use wire::gateway::memory::TagTaxonomyMergeResult;
use wire::gateway::memory::TagWire;
use wire::memory::MemoryEntry;
use wire::memory::MemorySearchResult;
use wire::message::Role;

use crate::BoxError;

#[derive(Debug, Error)]
pub enum MemoryStoreError {
    #[error("memory store is unavailable")]
    Unavailable(#[source] BoxError),
    #[error("memory query failed: {message}")]
    Query {
        message: String,
        #[source]
        source: BoxError,
    },
    #[error("{message}")]
    InvalidRequest { message: String },
    #[error("tag tables are missing: apply migration 0019 (rivetos db migrate)")]
    TagTablesMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TagId(Uuid);

impl TagId {
    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn uuid(self) -> Uuid {
        self.0
    }
}

impl FromStr for TagId {
    type Err = uuid::Error;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Ok(Self(raw.parse()?))
    }
}

impl Display for TagId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TagLiteral {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TagLiteralError {
    #[error("tag must be key:value (got \"{raw}\")")]
    Filter { raw: String },
    #[error("invalid tag literal \"{raw}\" (want key:value)")]
    Literal { raw: String },
}

impl TagLiteral {
    pub fn parse_add(raw: &str) -> Result<Self, TagLiteralError> {
        Self::split(raw, |raw| TagLiteralError::Literal { raw })
    }

    fn split(
        raw: &str,
        error: impl Fn(String) -> TagLiteralError,
    ) -> Result<Self, TagLiteralError> {
        let Some((key, value)) = raw.split_once(':') else {
            return Err(error(raw.to_owned()));
        };
        if key.is_empty() || value.is_empty() {
            return Err(error(raw.to_owned()));
        }
        Ok(Self {
            key: key.to_owned(),
            value: value.to_owned(),
        })
    }
}

impl FromStr for TagLiteral {
    type Err = TagLiteralError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::split(raw, |raw| TagLiteralError::Filter { raw })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    value: u32,
}

impl Limit {
    #[must_use]
    pub fn clamped(raw: Option<u32>, default: u32, range: RangeInclusive<u32>) -> Self {
        Self {
            value: raw.unwrap_or(default).clamp(*range.start(), *range.end()),
        }
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.value
    }
}

#[derive(
    serde::Serialize,
    schemars::JsonSchema,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    strum::Display,
    strum::EnumString,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Hybrid,
    Fts,
    Vector,
    Trigram,
    Regex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub mode: SearchMode,
    pub scope: MemoryScope,
    pub agent: Option<String>,
    pub tag: Option<TagLiteral>,
    pub limit: Limit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseFilter {
    pub conversation_id: Option<Uuid>,
    pub agent: Option<String>,
    pub role: Option<Role>,
    pub tool_name: Option<String>,
    pub tag: Option<TagLiteral>,
    pub window: Option<BrowseWindow>,
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub include_tools: bool,
    pub order: MemoryBrowseOrder,
    pub limit: Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagDecision {
    Accept,
    Reject,
}

impl From<TagDecision> for TagState {
    fn from(decision: TagDecision) -> Self {
        match decision {
            TagDecision::Accept => Self::Accepted,
            TagDecision::Reject => Self::Rejected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TagEntity {
    Id(Uuid),
    SessionKey(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagListQuery {
    pub entity_type: Option<TagEntityType>,
    pub entity_id: Option<Uuid>,
    pub key: Option<String>,
    pub value: Option<String>,
    pub states: Vec<TagState>,
    pub limit: Limit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagCountsQuery {
    pub key: Option<String>,
    pub limit: Limit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTaxonomyQuery {
    pub key: Option<String>,
    pub state: Option<TagState>,
    pub limit: Limit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagAdd {
    pub entity_type: TagEntityType,
    pub entity: TagEntity,
    pub agent: Option<String>,
    pub tag: TagLiteral,
    pub display: Option<String>,
    pub reason: Option<String>,
    pub decided_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagLookup {
    pub session_keys: Vec<String>,
    pub states: Vec<TagState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTaxonomyUpsert {
    pub key: String,
    pub value: String,
    pub display: Option<String>,
    pub parent_value: Option<String>,
    pub aliases: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTaxonomyDecision {
    pub entries: Vec<TagEntryPair>,
    pub decision: TagDecision,
    pub decided_by: Option<String>,
}

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn append(&self, batch: Vec<MemoryEntry>) -> Result<u64, MemoryStoreError>;

    async fn search(&self, query: SearchQuery)
    -> Result<Vec<MemorySearchResult>, MemoryStoreError>;

    async fn browse(&self, filter: BrowseFilter) -> Result<Vec<MemoryEntry>, MemoryStoreError>;

    async fn stats(&self) -> Result<MemoryStatsResponse, MemoryStoreError>;

    async fn get_full(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryStoreError>;

    async fn context(&self, session: &str, now: DateTime<Utc>) -> Result<String, MemoryStoreError>;

    async fn list_tags(&self, query: TagListQuery) -> Result<Vec<TagWire>, MemoryStoreError>;

    async fn pending_tags(&self, limit: Limit) -> Result<Vec<PendingTagWire>, MemoryStoreError>;

    async fn tag_counts(&self, query: TagCountsQuery) -> Result<Vec<TagCount>, MemoryStoreError>;

    async fn list_tag_taxonomy(
        &self,
        query: TagTaxonomyQuery,
    ) -> Result<Vec<TagTaxonomyEntry>, MemoryStoreError>;

    async fn decide_tags(
        &self,
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError>;

    async fn decide_tag_removals(
        &self,
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError>;

    async fn add_tag(&self, add: TagAdd) -> Result<TagWire, MemoryStoreError>;

    async fn lookup_tags(
        &self,
        lookup: TagLookup,
    ) -> Result<BTreeMap<String, Vec<TagWire>>, MemoryStoreError>;

    async fn upsert_tag_taxonomy(
        &self,
        upsert: TagTaxonomyUpsert,
    ) -> Result<TagTaxonomyEntry, MemoryStoreError>;

    async fn decide_tag_taxonomy(
        &self,
        decision: TagTaxonomyDecision,
    ) -> Result<u64, MemoryStoreError>;

    async fn merge_tag_taxonomy(
        &self,
        merge: TagTaxonomyMerge,
    ) -> Result<TagTaxonomyMergeResult, MemoryStoreError>;
}
