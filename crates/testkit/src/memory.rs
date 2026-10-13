use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::memory::BrowseFilter;
use kernel::memory::Limit;
use kernel::memory::MemoryStore;
use kernel::memory::MemoryStoreError;
use kernel::memory::SearchQuery;
use kernel::memory::TagAdd;
use kernel::memory::TagCountsQuery;
use kernel::memory::TagDecision;
use kernel::memory::TagEntity;
use kernel::memory::TagId;
use kernel::memory::TagListQuery;
use kernel::memory::TagLookup;
use kernel::memory::TagTaxonomyDecision;
use kernel::memory::TagTaxonomyQuery;
use kernel::memory::TagTaxonomyUpsert;
use tokio::sync::Mutex;
use wire::gateway::memory::MemoryHitSource;
use wire::gateway::memory::MemoryStatsResponse;
use wire::gateway::memory::PendingTagWire;
use wire::gateway::memory::TagCount;
use wire::gateway::memory::TagState;
use wire::gateway::memory::TagTaxonomyEntry;
use wire::gateway::memory::TagTaxonomyMerge;
use wire::gateway::memory::TagTaxonomyMergeResult;
use wire::gateway::memory::TagWire;
use wire::memory::MemoryEntry;
use wire::memory::MemorySearchResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryRequest {
    Append(Vec<MemoryEntry>),
    Search(SearchQuery),
    Browse(BrowseFilter),
    Stats,
    GetFull(String),
    Context {
        session: String,
        now: DateTime<Utc>,
    },
    ListTags(TagListQuery),
    PendingTags(Limit),
    TagCounts(TagCountsQuery),
    TagTaxonomy(TagTaxonomyQuery),
    DecideTags {
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    },
    DecideTagRemovals {
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    },
    AddTag(TagAdd),
    LookupTags(TagLookup),
    UpsertTagTaxonomy(TagTaxonomyUpsert),
    DecideTagTaxonomy(TagTaxonomyDecision),
    MergeTagTaxonomy(TagTaxonomyMerge),
}

#[derive(Debug, Default)]
pub struct FakeMemoryStore {
    entries: Mutex<Vec<MemoryEntry>>,
    requests: Mutex<Vec<MemoryRequest>>,
}

impl FakeMemoryStore {
    pub async fn requests(&self) -> Vec<MemoryRequest> {
        self.requests.lock().await.clone()
    }

    async fn record(&self, request: MemoryRequest) {
        self.requests.lock().await.push(request);
    }
}

#[async_trait]
impl MemoryStore for FakeMemoryStore {
    async fn append(&self, batch: Vec<MemoryEntry>) -> Result<u64, MemoryStoreError> {
        let count = batch.len() as u64;
        self.record(MemoryRequest::Append(batch.clone())).await;
        self.entries.lock().await.extend(batch);
        Ok(count)
    }

    async fn search(
        &self,
        query: SearchQuery,
    ) -> Result<Vec<MemorySearchResult>, MemoryStoreError> {
        self.record(MemoryRequest::Search(query.clone())).await;
        let needle = query.text.trim().to_owned();
        let entries = self.entries.lock().await;
        Ok(entries
            .iter()
            .filter(|e| e.content.contains(&needle))
            .filter(|e| query.agent.as_deref().is_none_or(|a| e.agent == a))
            .take(query.limit.get() as usize)
            .map(|e| MemorySearchResult {
                id: e.id.clone().unwrap_or_default(),
                source: MemoryHitSource::Message,
                content: e.content.clone(),
                role: Some(e.role),
                agent: Some(e.agent.clone()),
                relevance_score: 1.0,
                created_at: e.created_at.unwrap_or_default(),
            })
            .collect())
    }

    async fn browse(&self, filter: BrowseFilter) -> Result<Vec<MemoryEntry>, MemoryStoreError> {
        self.record(MemoryRequest::Browse(filter.clone())).await;
        let entries = self.entries.lock().await;
        Ok(entries
            .iter()
            .filter(|e| filter.agent.as_deref().is_none_or(|a| e.agent == a))
            .filter(|e| filter.role.is_none_or(|r| e.role == r))
            .filter(|e| {
                filter
                    .tool_name
                    .as_deref()
                    .is_none_or(|t| e.tool_name.as_deref() == Some(t))
            })
            .take(filter.limit.get() as usize)
            .cloned()
            .collect())
    }

    async fn stats(&self) -> Result<MemoryStatsResponse, MemoryStoreError> {
        self.record(MemoryRequest::Stats).await;
        let count = self.entries.lock().await.len() as u64;
        Ok(MemoryStatsResponse {
            conversations: 1,
            messages: count,
            tool_calls: 0,
            summaries: 0,
            embed_queue_depth: 0,
            embedded_messages: 0,
            failed_embeddings: 0,
            top_tools: Vec::new(),
            recent_sessions: Vec::new(),
        })
    }

    async fn get_full(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryStoreError> {
        self.record(MemoryRequest::GetFull(id.to_owned())).await;
        Ok(self
            .entries
            .lock()
            .await
            .iter()
            .find(|e| e.id.as_deref() == Some(id))
            .cloned())
    }

    async fn context(&self, session: &str, now: DateTime<Utc>) -> Result<String, MemoryStoreError> {
        self.record(MemoryRequest::Context {
            session: session.to_owned(),
            now,
        })
        .await;
        let entries = self.entries.lock().await;
        Ok(entries
            .iter()
            .filter(|e| e.session_id == session)
            .map(|e| e.content.clone())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn list_tags(&self, query: TagListQuery) -> Result<Vec<TagWire>, MemoryStoreError> {
        self.record(MemoryRequest::ListTags(query)).await;
        Ok(Vec::new())
    }

    async fn pending_tags(&self, limit: Limit) -> Result<Vec<PendingTagWire>, MemoryStoreError> {
        self.record(MemoryRequest::PendingTags(limit)).await;
        Ok(Vec::new())
    }

    async fn tag_counts(&self, query: TagCountsQuery) -> Result<Vec<TagCount>, MemoryStoreError> {
        self.record(MemoryRequest::TagCounts(query)).await;
        Ok(Vec::new())
    }

    async fn list_tag_taxonomy(
        &self,
        query: TagTaxonomyQuery,
    ) -> Result<Vec<TagTaxonomyEntry>, MemoryStoreError> {
        self.record(MemoryRequest::TagTaxonomy(query)).await;
        Ok(Vec::new())
    }

    async fn decide_tags(
        &self,
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError> {
        self.record(MemoryRequest::DecideTags {
            ids,
            decision,
            decided_by,
        })
        .await;
        Ok(0)
    }

    async fn decide_tag_removals(
        &self,
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError> {
        self.record(MemoryRequest::DecideTagRemovals {
            ids,
            decision,
            decided_by,
        })
        .await;
        Ok(0)
    }

    async fn add_tag(&self, add: TagAdd) -> Result<TagWire, MemoryStoreError> {
        self.record(MemoryRequest::AddTag(add.clone())).await;
        Ok(TagWire {
            id: String::new(),
            entity_type: add.entity_type,
            entity_id: match &add.entity {
                TagEntity::Id(id) => id.to_string(),
                TagEntity::SessionKey(_) => String::new(),
            },
            key: add.tag.key,
            value: add.tag.value,
            display: add.display.unwrap_or_default(),
            source: "user".to_owned(),
            state: TagState::Accepted,
            confidence: None,
            proposed_by: String::new(),
            reason: add.reason.unwrap_or_default(),
            decided_by: add.decided_by,
            decided_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    async fn lookup_tags(
        &self,
        lookup: TagLookup,
    ) -> Result<BTreeMap<String, Vec<TagWire>>, MemoryStoreError> {
        self.record(MemoryRequest::LookupTags(lookup)).await;
        Ok(BTreeMap::new())
    }

    async fn upsert_tag_taxonomy(
        &self,
        upsert: TagTaxonomyUpsert,
    ) -> Result<TagTaxonomyEntry, MemoryStoreError> {
        self.record(MemoryRequest::UpsertTagTaxonomy(upsert.clone()))
            .await;
        Ok(TagTaxonomyEntry {
            key: upsert.key,
            value: upsert.value,
            display: upsert.display.unwrap_or_default(),
            parent_value: upsert.parent_value,
            aliases: upsert.aliases,
            state: TagState::Accepted,
            source: "user".to_owned(),
            reason: upsert.reason.unwrap_or_default(),
            decided_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    async fn decide_tag_taxonomy(
        &self,
        decision: TagTaxonomyDecision,
    ) -> Result<u64, MemoryStoreError> {
        self.record(MemoryRequest::DecideTagTaxonomy(decision))
            .await;
        Ok(0)
    }

    async fn merge_tag_taxonomy(
        &self,
        merge: TagTaxonomyMerge,
    ) -> Result<TagTaxonomyMergeResult, MemoryStoreError> {
        self.record(MemoryRequest::MergeTagTaxonomy(merge.clone()))
            .await;
        Ok(TagTaxonomyMergeResult {
            moved: 0,
            dropped: 0,
            into: merge.into,
        })
    }
}
