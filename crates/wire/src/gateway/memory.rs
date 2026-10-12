use crate::JsonObject;
use crate::message::{Role, ToolResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryToolName {
    MemorySearch,
    MemoryBrowse,
    MemoryStats,
    MemoryGetFull,
    MemoryAppend,
    MemoryIngestSession,
    MemoryTags,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySearchToolArgs {
    pub query: String,
    pub mode: Option<String>,
    pub scope: Option<MemoryScope>,
    pub limit: Option<u32>,
    pub agent: Option<String>,
    pub since: Option<String>,
    pub before: Option<String>,
    pub window: Option<String>,
    pub tag: Option<String>,
    pub expand: Option<bool>,
    pub synthesize: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryScope {
    Messages,
    Summaries,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryBrowseOrder {
    Asc,
    Desc,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryBrowseToolArgs {
    pub conversation_id: Option<String>,
    pub include_tools: Option<bool>,
    pub since: Option<String>,
    pub before: Option<String>,
    pub window: Option<String>,
    pub agent: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<u32>,
    pub order: Option<MemoryBrowseOrder>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAppendToolArgs {
    pub session_id: String,
    pub content: String,
    pub role: Role,
    pub tool_name: Option<String>,
    pub tool_args: Option<JsonObject>,
    pub tool_result: Option<String>,
    pub event_id: Option<String>,
    pub agent: Option<String>,
    pub persona: Option<String>,
    pub source: Option<String>,
    pub channel: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryIngestMessage {
    pub role: Role,
    pub content: String,
    pub created_at: Option<String>,
    pub tool_calls: Option<Vec<MemoryIngestToolCall>>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryIngestToolCall {
    pub id: Option<String>,
    pub name: String,
    pub input: Option<JsonObject>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryIngestSessionToolArgs {
    pub session_id: String,
    pub messages: Vec<MemoryIngestMessage>,
    pub agent: Option<String>,
    pub persona: Option<String>,
    pub source: Option<String>,
    pub channel: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTagsToolArgs {
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub session_key: Option<String>,
    pub agent: Option<String>,
    pub key: Option<String>,
    pub value: Option<String>,
    pub tag: Option<String>,
    pub display: Option<String>,
    pub state: Option<TagState>,
    pub ids: Option<Vec<String>>,
    pub session_keys: Option<Vec<String>>,
    pub entries: Option<Vec<TagEntryPair>>,
    pub parent_value: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub from: Option<String>,
    pub into: Option<String>,
    pub reason: Option<String>,
    pub decided_by: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagEntryPair {
    pub key: String,
    pub value: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryToolResponse {
    pub ok: bool,
    pub result: ToolResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryHitSource {
    Message,
    Summary,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchHit {
    pub id: String,
    pub source: MemoryHitSource,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub score: f64,
    pub role: Option<Role>,
    pub agent: Option<String>,
    pub kind: Option<String>,
    pub tool_name: Option<String>,
    pub conversation_id: Option<String>,
    pub session_id: Option<String>,
    pub fallback: Option<MemoryFallback>,
    pub tags: Option<Vec<TagWire>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryFallback {
    Trigram,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDegraded {
    pub reason: String,
    pub effect: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySearchResponse {
    pub query: String,
    pub scope: MemoryScope,
    pub degraded: Option<MemoryDegraded>,
    pub fallback: Option<MemoryFallback>,
    pub results: Vec<MemorySearchHit>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBrowseMessage {
    pub id: String,
    pub role: Role,
    pub agent: Option<String>,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub conversation_id: Option<String>,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub tags: Option<Vec<TagWire>>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBrowseResponse {
    pub messages: Vec<MemoryBrowseMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopTool {
    pub tool: String,
    pub count: u64,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentSession {
    pub session_id: String,
    pub title: Option<String>,
    pub agent: String,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub last_active: DateTime<Utc>,
    pub messages: u64,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatsResponse {
    pub conversations: u64,
    pub messages: u64,
    pub tool_calls: u64,
    pub summaries: u64,
    pub embed_queue_depth: u64,
    pub embedded_messages: u64,
    pub failed_embeddings: u64,
    pub top_tools: Vec<TopTool>,
    pub recent_sessions: Vec<RecentSession>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryHealthStatus {
    Ok,
    Degraded,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmbedStatus {
    Ok,
    Unavailable,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedHealth {
    pub status: EmbedStatus,
    pub error: Option<String>,
    pub impact: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHealthResponse {
    pub observed_at: Option<DateTime<Utc>>,
    pub queue_status: Option<QueueStatus>,
    pub queues: Option<QueueHealth>,
    pub compaction: Option<CompactionHealth>,
    pub failed_embeddings: Option<u64>,
    pub skipped_embeddings: Option<u64>,
    pub capture: Option<CaptureHealth>,
    pub status: MemoryHealthStatus,
    pub embeddings: EmbedHealth,
    pub embed_queue_depth: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueStatus {
    Available,
    Unavailable,
    Restricted,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueHealth {
    pub task: String,
    pub pending: u64,
    pub running: u64,
    pub scheduled: u64,
    pub dead: u64,
    pub oldest_pending_minutes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionHealth {
    pub eligible: bool,
    pub active_tail: bool,
    pub below_floor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureHealth {
    pub status: String,
    pub impact: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TagEntityType {
    Conversation,
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TagState {
    Suggested,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TagAction {
    Add,
    Remove,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWire {
    pub id: String,
    pub entity_type: TagEntityType,
    pub entity_id: String,
    pub key: String,
    pub value: String,
    pub display: String,
    pub source: String,
    pub state: TagState,
    pub confidence: Option<f64>,
    pub proposed_by: String,
    pub reason: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTagWire {
    #[serde(flatten)]
    pub tag: TagWire,
    pub action: Option<TagAction>,
    pub session_key: Option<String>,
    pub title: Option<String>,
    pub agent: Option<String>,
    pub conversation_id: Option<String>,
    pub excerpt: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTagsDecideRequest {
    pub ids: Vec<String>,
    pub state: TagState,
    pub decided_by: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTagsAddRequest {
    pub entity_type: String,
    pub entity_id: Option<String>,
    pub session_key: Option<String>,
    pub agent: Option<String>,
    pub tag: Option<String>,
    pub key: Option<String>,
    pub value: Option<String>,
    pub display: Option<String>,
    pub reason: Option<String>,
    pub decided_by: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTagsLookupRequest {
    pub session_keys: Vec<String>,
    pub states: Option<Vec<TagState>>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagTaxonomyEntry {
    pub key: String,
    pub value: String,
    pub display: String,
    pub parent_value: Option<String>,
    pub aliases: Vec<String>,
    pub state: TagState,
    pub source: String,
    pub reason: String,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagTaxonomyWire {
    pub key: String,
    pub entries: Vec<TagTaxonomyEntry>,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagTaxonomyUpsertRequest {
    pub parent_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagTaxonomyMerge {
    pub key: String,
    pub from: String,
    pub into: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagTaxonomyMergeResult {
    pub moved: u64,
    pub dropped: u64,
    pub into: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub key: String,
    pub value: String,
    pub display: String,
    pub conversations: u64,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuleResult {
    pub key: String,
    pub rule: ProjectRule,
    pub git_root: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectRule {
    GitRemote,
    GitRoot,
    CwdBasename,
}
