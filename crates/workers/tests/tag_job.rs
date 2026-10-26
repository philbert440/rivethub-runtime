#![cfg(test)]

#[expect(
    unreachable_pub,
    reason = "the harness is a private module of each test binary"
)]
#[expect(
    dead_code,
    reason = "each test binary uses a different subset of the harness"
)]
mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::Phase;
use common::Queue;
use kernel::memory::SummaryStore;
use kernel::memory::SummaryStoreError;
use kernel::memory::TagJobContext;
use kernel::memory::TagVocabulary;
use kernel::model::Embedding;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use kernel::model::ModelRequest;
use kernel::model::ModelStreamItem;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::memory::TagState;
use wire::stream::LlmResponse;
use workers::ProcessRole;
use workers::Runner;
use workers::RunnerConfig;
use workers::TagSettings;
use workers::WorkerDeps;
use workers::suggest_tags_job;

const SUMMARY_ID: &str = "00000000-0000-4000-8000-000000000014";
const CONVERSATION_ID: &str = "00000000-0000-4000-8000-000000000002";

struct Status;

struct ClosedSummaries;

#[tokio::test]
async fn a_failing_tagger_stays_queued_after_attempt_one() {
    let summary_id = Uuid::parse_str(SUMMARY_ID).expect("summary");
    let conversation_id = Uuid::parse_str(CONVERSATION_ID).expect("conversation");
    let memory = Arc::new(testkit::FakeMemoryStore::default());
    memory
        .script_tag_context(context(summary_id, conversation_id))
        .await;
    let queue = Arc::new(Queue::new());
    let job = suggest_tags_job(summary_id, conversation_id).expect("job");
    let id = queue.push(job.name, job.payload, 2).await;
    let runner = Runner::new(
        Arc::clone(&queue),
        RunnerConfig::new(ProcessRole::Categorizer, 1).expect("role"),
        deps(memory),
    );
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let run = tokio::spawn(async move { runner.run(token).await });
    queue.wait_phase(id, Phase::Backoff).await;
    let failed = queue.view(id).await;
    assert_eq!(failed.attempt, 1);
    assert_eq!(failed.name, "suggest-tags");
    assert_eq!(failed.error.as_deref(), Some("LLM HTTP 400 (not retrying)"));
    cancel.cancel();
    run.await.expect("join").expect("runner");
}

fn context(summary_id: Uuid, conversation_id: Uuid) -> TagJobContext {
    TagJobContext {
        summary_id,
        conversation_id,
        session_key: "cli:s1".to_owned(),
        title: None,
        agent: "rivet".to_owned(),
        summary: "A deploy summary.".to_owned(),
        vocabulary: vec![TagVocabulary {
            key: "topic".to_owned(),
            value: "memory-summarizer".to_owned(),
            display: String::new(),
            aliases: Vec::new(),
            state: TagState::Accepted,
        }],
        tags: Vec::new(),
    }
}

fn deps(memory: Arc<testkit::FakeMemoryStore>) -> WorkerDeps {
    WorkerDeps {
        targets: Arc::new(common::Store::new()),
        summaries: Arc::new(ClosedSummaries),
        memory,
        provider: Arc::new(Status),
        embed: common::settings(),
        summary: workers::SummarySettings::standard("fake-chat").expect("summary"),
        tags: TagSettings::new("tagger-v1", false).expect("tags"),
    }
}

#[async_trait]
impl SummaryStore for ClosedSummaries {
    async fn conversation(
        &self,
        _id: Uuid,
    ) -> Result<Option<kernel::memory::ConversationWindow>, SummaryStoreError> {
        Err(SummaryStoreError::Closed)
    }

    async fn pending(&self) -> Result<Vec<kernel::memory::ConversationWindow>, SummaryStoreError> {
        Err(SummaryStoreError::Closed)
    }

    async fn write_summary(
        &self,
        _summary: kernel::memory::NewSummary,
    ) -> Result<kernel::memory::SummaryWrite, SummaryStoreError> {
        Err(SummaryStoreError::Closed)
    }
}

#[async_trait]
impl ModelProvider for Status {
    async fn chat(&self, _request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        Err(ModelProviderError::Request {
            message: "model server returned 400 synthetic".to_owned(),
            source: Box::new(std::io::Error::other("synthetic status")),
        })
    }

    async fn stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Receiver<ModelStreamItem>, ModelProviderError> {
        Err(ModelProviderError::Request {
            message: "no stream".to_owned(),
            source: Box::new(std::io::Error::other("no stream")),
        })
    }

    async fn embed(&self, _texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        Err(ModelProviderError::Request {
            message: "no embeddings".to_owned(),
            source: Box::new(std::io::Error::other("no embeddings")),
        })
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Ok(Vec::new())
    }
}
