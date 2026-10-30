use crate::capture::CaptureOptions;
use crate::capture::CaptureWriter;
use crate::capture::ConversationUpsert;
use crate::capture::MessageRow;
use crate::capture::prepare_capture_message;
use crate::error::Error;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::memory::BrowseFilter;
use kernel::memory::CaptureBatchRequest;
use kernel::memory::CaptureMessage;
use kernel::memory::CaptureResult;
use kernel::memory::MemoryEntry;
use kernel::memory::MemoryStore;
use kernel::memory::MemoryStoreError;
use kernel::memory::PendingTagWire;
use kernel::memory::SearchQuery;
use kernel::memory::TagAdd;
use kernel::memory::TagCount;
use kernel::memory::TagCountsQuery;
use kernel::memory::TagDecision;
use kernel::memory::TagId;
use kernel::memory::TagListQuery;
use kernel::memory::TagLookup;
use kernel::memory::TagTaxonomyDecision;
use kernel::memory::TagTaxonomyEntry;
use kernel::memory::TagTaxonomyMerge;
use kernel::memory::TagTaxonomyMergeResult;
use kernel::memory::TagTaxonomyQuery;
use kernel::memory::TagTaxonomyUpsert;
use kernel::memory::TagWire;
use kernel::memory::spoken_title;
use serde::Deserialize as _;
use uuid::Uuid;

const UNKNOWN_CHANNEL: &str = "unknown";

impl crate::PostgresMemory {
    pub async fn capture_batch(
        &self,
        batch: &CaptureBatchRequest,
        options: &CaptureOptions,
    ) -> Result<CaptureResult, Error> {
        let channel = batch.channel.as_deref().ok_or(Error::MissingChannel)?;
        let owner = options.owner_user_id.as_deref();
        let mut writer = CaptureWriter::begin(&self.pool, &batch.session_key).await?;
        if let Some(dropped) = dropped_when_deleted(&mut writer, batch).await? {
            return Ok(dropped);
        }
        let conversation_id = writer
            .upsert_conversation(&ConversationUpsert {
                session_key: &batch.session_key,
                agent: &batch.agent,
                channel: Some(channel),
                title: batch.title.as_deref().and_then(spoken_title),
                settings: batch.settings.as_ref(),
                task_id: batch.task_id.as_deref(),
                owner_user_id: owner,
                node: options.node.as_deref(),
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
                    channel,
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
        if batch.finalize == Some(true) {
            writer.finalize_conversation(conversation_id).await?;
        }
        writer.commit().await?;
        Ok(CaptureResult {
            ok: true,
            conversation_id: conversation_id.to_string(),
            inserted,
            skipped: u64::try_from(batch.messages.len())? - inserted,
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
        let event_id = entry_event_id(entry)?;
        let mut metadata = entry.metadata.clone();
        metadata
            .get_or_insert_default()
            .insert("event_id", &event_id)
            .map_err(Error::from)?;
        messages.push(CaptureMessage {
            event_id,
            role: entry.role,
            content: entry.content.clone(),
            tool_name: entry.tool_name.clone(),
            tool_args: entry.tool_args.clone(),
            tool_result: entry.tool_result.clone(),
            metadata,
            created_at: entry.created_at,
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

#[derive(serde::Deserialize)]
struct EntryMetadata {
    event_id: Option<String>,
}

fn entry_event_id(entry: &MemoryEntry) -> Result<String, Error> {
    if let Some(metadata) = &entry.metadata {
        let metadata = EntryMetadata::deserialize(serde::de::value::MapDeserializer::<
            _,
            serde_json::Error,
        >::new(
            metadata
                .as_map()
                .iter()
                .map(|(key, value)| (key.as_str(), value)),
        ))?;
        if let Some(event_id) = metadata.event_id {
            return Ok(event_id);
        }
    }
    append_event_id(entry)
}

fn append_event_id(entry: &MemoryEntry) -> Result<String, Error> {
    use sha2::Digest as _;
    let input = (
        &entry.session_id,
        &entry.agent,
        entry.role,
        &entry.content,
        &entry.tool_name,
    );
    Ok(hex::encode(sha2::Sha256::digest(serde_json::to_vec(
        &input,
    )?)))
}

async fn dropped_when_deleted(
    writer: &mut CaptureWriter<'_>,
    batch: &CaptureBatchRequest,
) -> Result<Option<CaptureResult>, Error> {
    let Some(deleted_at) =
        crate::deletion::tombstone_at(&mut writer.tx, &batch.session_key).await?
    else {
        return Ok(None);
    };
    let stamps = batch.messages.iter().map(|message| message.created_at);
    if kernel::memory::used_since(deleted_at, stamps) {
        crate::deletion::lift(&mut writer.tx, &batch.session_key).await?;
        return Ok(None);
    }
    tracing::debug!(
        session_key = %batch.session_key,
        "capture dropped for a deleted session"
    );
    Ok(Some(CaptureResult {
        ok: true,
        conversation_id: String::new(),
        inserted: 0,
        skipped: u64::try_from(batch.messages.len())?,
    }))
}

#[async_trait]
impl MemoryStore for crate::PostgresMemory {
    async fn close(&self) -> Result<(), MemoryStoreError> {
        self.access
            .settled()
            .await
            .map_err(|error| MemoryStoreError::Unavailable(Box::new(error)))
    }

    async fn probe(&self) -> Result<(), MemoryStoreError> {
        self.ping().await.map_err(Into::into)
    }

    fn embeddings_armed(&self) -> bool {
        self.embed.is_some()
    }

    async fn embed_model_mismatch(
        &self,
    ) -> Result<Option<kernel::memory::EmbedModelMismatch>, MemoryStoreError> {
        Ok(None)
    }

    async fn wiki_topics(
        &self,
        filter: kernel::wiki::WikiFilter,
    ) -> Result<kernel::wiki::WikiTopicList, MemoryStoreError> {
        self.read_wiki_topics(filter).await.map_err(Into::into)
    }

    async fn wiki_page(
        &self,
        slug: &kernel::wiki::page::WikiSlug,
    ) -> Result<kernel::wiki::WikiLookup, MemoryStoreError> {
        self.read_wiki_page(slug).await.map_err(Into::into)
    }

    async fn wiki_gaps(
        &self,
        limit: u32,
    ) -> Result<kernel::wiki::WikiGapsResponse, MemoryStoreError> {
        self.read_wiki_gaps(limit).await.map_err(Into::into)
    }

    async fn capture(
        &self,
        mut batch: CaptureBatchRequest,
        node: Option<&str>,
    ) -> Result<CaptureResult, MemoryStoreError> {
        batch
            .channel
            .get_or_insert_with(|| UNKNOWN_CHANNEL.to_owned());
        let options = CaptureOptions {
            node: node.map(str::to_owned),
            ..CaptureOptions::default()
        };
        self.capture_batch(&batch, &options)
            .await
            .map_err(MemoryStoreError::from)
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

    async fn stats(&self) -> Result<kernel::memory::MemoryStatsResponse, MemoryStoreError> {
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

    async fn summary_dag(
        &self,
        owner: &kernel::memory::SummaryOwner,
    ) -> Result<kernel::memory::SummaryDag, MemoryStoreError> {
        self.read_summary_dag(owner).await
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

    async fn tag_job_context(
        &self,
        summary_id: Uuid,
    ) -> Result<Option<kernel::memory::TagJobContext>, MemoryStoreError> {
        self.tags_job_context(summary_id).await
    }

    async fn store_tag_suggestions(
        &self,
        write: kernel::memory::TagSuggestionWrite,
    ) -> Result<kernel::memory::TagSuggestionStored, MemoryStoreError> {
        self.tags_store_suggestions(&write).await
    }
}
