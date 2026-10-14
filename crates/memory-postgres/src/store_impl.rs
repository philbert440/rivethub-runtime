use crate::capture::CaptureOptions;
use crate::capture::CaptureWriter;
use crate::capture::ConversationUpsert;
use crate::capture::MessageRow;
use crate::capture::prepare_capture_message;
use crate::error::Error;
use crate::project;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::memory::BrowseFilter;
use kernel::memory::MemoryStore;
use kernel::memory::MemoryStoreError;
use kernel::memory::SearchQuery;
use kernel::memory::TagAdd;
use kernel::memory::TagCountsQuery;
use kernel::memory::TagDecision;
use kernel::memory::TagId;
use kernel::memory::TagListQuery;
use kernel::memory::TagLookup;
use kernel::memory::TagTaxonomyDecision;
use kernel::memory::TagTaxonomyQuery;
use kernel::memory::TagTaxonomyUpsert;
use wire::JsonObject;
use wire::capture::CaptureBatchRequest;
use wire::capture::CaptureMessage;
use wire::capture::CaptureResult;
use wire::gateway::memory::PendingTagWire;
use wire::gateway::memory::TagCount;
use wire::gateway::memory::TagTaxonomyEntry;
use wire::gateway::memory::TagTaxonomyMerge;
use wire::gateway::memory::TagTaxonomyMergeResult;
use wire::gateway::memory::TagWire;
use wire::memory::MemoryEntry;

impl crate::PostgresMemory {
    pub async fn capture_batch(
        &self,
        batch: &CaptureBatchRequest,
        options: &CaptureOptions,
    ) -> Result<CaptureResult, Error> {
        let cwd = batch
            .settings
            .as_ref()
            .and_then(|settings| settings.get("cwd"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let tag = project::resolve_project_tag_blocking(cwd, options.allow_filesystem).await?;
        let owner = options.owner_user_id.as_deref();
        let mut writer = CaptureWriter::begin(&self.pool, &batch.session_key).await?;
        let conversation_id = writer
            .upsert_conversation(&ConversationUpsert {
                session_key: &batch.session_key,
                agent: &batch.agent,
                channel: Some(batch.channel.as_deref().unwrap_or("unknown")),
                title: batch.title.as_deref(),
                settings: batch.settings.as_ref(),
                task_id: batch.task_id.as_deref(),
                owner_user_id: owner,
            })
            .await?;
        let event_ids: Vec<String> = batch
            .messages
            .iter()
            .map(|message| message.event_id.clone())
            .collect();
        let seen = writer.seen_event_ids(conversation_id, &event_ids).await?;
        let mut inserted: u64 = 0;
        for message in &batch.messages {
            if seen.contains(&message.event_id) {
                continue;
            }
            let prepared = prepare_capture_message(message)?;
            writer
                .insert_message(&MessageRow {
                    conversation_id,
                    agent: &batch.agent,
                    channel: batch.channel.as_deref().unwrap_or("unknown"),
                    role: message.role,
                    content: &prepared.content,
                    tool_name: message.tool_name.as_deref(),
                    tool_args: message.tool_args.as_ref(),
                    tool_result: prepared.tool_result.as_deref(),
                    metadata: &prepared.metadata,
                    owner_user_id: owner,
                    created_at: prepared.created_at,
                })
                .await?;
            inserted += 1;
        }
        if let Some(tag) = &tag {
            writer.apply_project_tag(conversation_id, tag).await?;
        }
        if batch.finalize == Some(true) {
            writer.finalize_conversation(conversation_id).await?;
        }
        writer.commit().await?;
        Ok(CaptureResult {
            ok: true,
            conversation_id: conversation_id.to_string(),
            inserted,
            skipped: batch.messages.len() as u64 - inserted,
        })
    }
}

struct EntryGroup {
    session_key: String,
    agent: String,
    channel: Option<String>,
    entries: Vec<MemoryEntry>,
}

fn group_entries(batch: Vec<MemoryEntry>) -> Vec<EntryGroup> {
    let mut groups: Vec<EntryGroup> = Vec::new();
    for entry in batch {
        match groups
            .iter_mut()
            .find(|group| (&group.session_key, &group.agent) == (&entry.session_id, &entry.agent))
        {
            Some(group) => group.entries.push(entry),
            None => groups.push(EntryGroup {
                session_key: entry.session_id.clone(),
                agent: entry.agent.clone(),
                channel: Some(entry.channel.clone()),
                entries: vec![entry],
            }),
        }
    }
    groups
}

fn capture_request(group: &EntryGroup) -> Result<CaptureBatchRequest, MemoryStoreError> {
    let mut messages = Vec::with_capacity(group.entries.len());
    for entry in &group.entries {
        let event_id = entry_event_id(entry).ok_or_else(|| MemoryStoreError::InvalidRequest {
            message: "memory entry is missing an event id".to_string(),
        })?;
        let mut metadata = entry.metadata.clone().unwrap_or_default();
        if metadata.get("event_id").is_none() {
            metadata = merge_event_id(&metadata, &event_id)?;
        }
        messages.push(CaptureMessage {
            event_id,
            role: entry.role,
            content: entry.content.clone(),
            tool_name: entry.tool_name.clone(),
            tool_args: entry.tool_args.clone(),
            tool_result: entry.tool_result.clone(),
            metadata: Some(metadata),
            created_at: entry.created_at.map(|at| at.to_rfc3339()),
        });
    }
    Ok(CaptureBatchRequest {
        session_key: group.session_key.clone(),
        agent: group.agent.clone(),
        channel: group.channel.clone(),
        title: None,
        settings: None,
        task_id: None,
        finalize: None,
        created_at: None,
        updated_at: None,
        messages,
    })
}

fn entry_event_id(entry: &MemoryEntry) -> Option<String> {
    entry
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("event_id"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| Some(append_event_id(entry)))
}

fn append_event_id(entry: &MemoryEntry) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(
        format!(
            "append\0{}\0{}\0{}\0{}\0{}",
            entry.session_id,
            entry.agent,
            entry.role,
            entry.content,
            entry.tool_name.as_deref().unwrap_or("")
        )
        .as_bytes(),
    );
    hex::encode(digest)
}

fn merge_event_id(metadata: &JsonObject, event_id: &str) -> Result<JsonObject, Error> {
    let mut json =
        serde_json::to_value(metadata).map_err(|source| Error::protocol(source.to_string()))?;
    if let Some(object) = json.as_object_mut() {
        let member =
            serde_json::to_value(event_id).map_err(|source| Error::protocol(source.to_string()))?;
        object.insert("event_id".to_string(), member);
    }
    serde_json::from_value(json).map_err(|source| Error::protocol(source.to_string()))
}

#[async_trait]
impl MemoryStore for crate::PostgresMemory {
    async fn wiki_topics(
        &self,
        filter: kernel::wiki::WikiFilter,
    ) -> Result<wire::gateway::wiki::WikiTopicList, MemoryStoreError> {
        self.read_wiki_topics(filter).await.map_err(Into::into)
    }

    async fn wiki_page(
        &self,
        slug: &wire::gateway::wiki::WikiSlug,
    ) -> Result<kernel::wiki::WikiLookup, MemoryStoreError> {
        self.read_wiki_page(slug).await.map_err(Into::into)
    }

    async fn wiki_gaps(
        &self,
        limit: u32,
    ) -> Result<wire::gateway::wiki::WikiGapsResponse, MemoryStoreError> {
        self.read_wiki_gaps(limit).await.map_err(Into::into)
    }

    async fn append(&self, batch: Vec<MemoryEntry>) -> Result<u64, MemoryStoreError> {
        let mut inserted = 0u64;
        for group in group_entries(batch) {
            let request = capture_request(&group)?;
            let result = self
                .capture_batch(&request, &CaptureOptions::default())
                .await
                .map_err(MemoryStoreError::from)?;
            inserted += result.inserted;
        }
        Ok(inserted)
    }

    async fn search(
        &self,
        query: SearchQuery,
    ) -> Result<kernel::memory::SearchOutcome, MemoryStoreError> {
        self.complete_search(query).await
    }

    async fn browse(&self, filter: BrowseFilter) -> Result<Vec<MemoryEntry>, MemoryStoreError> {
        self.browse_port(&filter)
            .await
            .map_err(MemoryStoreError::from)
    }

    async fn stats(&self) -> Result<wire::gateway::memory::MemoryStatsResponse, MemoryStoreError> {
        self.memory_stats().await.map_err(MemoryStoreError::from)
    }

    async fn get_full(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryStoreError> {
        self.memory_get_full(id)
            .await
            .map_err(MemoryStoreError::from)
    }

    async fn context(&self, session: &str, now: DateTime<Utc>) -> Result<String, MemoryStoreError> {
        self.context_for_turn(session, now)
            .await
            .map_err(MemoryStoreError::from)
    }

    async fn list_tags(&self, query: TagListQuery) -> Result<Vec<TagWire>, MemoryStoreError> {
        self.tags_list(&query).await
    }

    async fn pending_tags(
        &self,
        limit: kernel::memory::Limit,
    ) -> Result<Vec<PendingTagWire>, MemoryStoreError> {
        self.tags_pending(limit).await
    }

    async fn tag_counts(&self, query: TagCountsQuery) -> Result<Vec<TagCount>, MemoryStoreError> {
        self.tags_counts(&query).await
    }

    async fn list_tag_taxonomy(
        &self,
        query: TagTaxonomyQuery,
    ) -> Result<Vec<TagTaxonomyEntry>, MemoryStoreError> {
        self.tags_taxonomy(&query).await
    }

    async fn decide_tags(
        &self,
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError> {
        self.tags_decide(&ids, decision, decided_by).await
    }

    async fn decide_tag_removals(
        &self,
        ids: Vec<TagId>,
        decision: TagDecision,
        decided_by: Option<String>,
    ) -> Result<u64, MemoryStoreError> {
        self.tags_decide_removals(&ids, decision, decided_by).await
    }

    async fn add_tag(&self, add: TagAdd) -> Result<TagWire, MemoryStoreError> {
        self.tags_add(&add).await
    }

    async fn lookup_tags(
        &self,
        lookup: TagLookup,
    ) -> Result<std::collections::BTreeMap<String, Vec<TagWire>>, MemoryStoreError> {
        self.tags_lookup(&lookup).await
    }

    async fn upsert_tag_taxonomy(
        &self,
        upsert: TagTaxonomyUpsert,
    ) -> Result<TagTaxonomyEntry, MemoryStoreError> {
        self.tags_taxonomy_upsert(&upsert).await
    }

    async fn decide_tag_taxonomy(
        &self,
        decision: TagTaxonomyDecision,
    ) -> Result<u64, MemoryStoreError> {
        self.tags_taxonomy_decide(&decision).await
    }

    async fn merge_tag_taxonomy(
        &self,
        merge: TagTaxonomyMerge,
    ) -> Result<TagTaxonomyMergeResult, MemoryStoreError> {
        self.tags_taxonomy_merge(&merge).await
    }
}
