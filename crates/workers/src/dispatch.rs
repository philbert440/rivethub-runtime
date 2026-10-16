use crate::categorize::{CategorizeEnv, TagSettings, handle_suggest};
use crate::embed::{EmbedSettings, handle_embed};
use crate::error::WorkerError;
use crate::role::ProcessRole;
use crate::summarize::{
    SummarizeEnv, SummarySettings, handle_enqueue_idle, handle_enqueue_stale, handle_reap,
    handle_summarize,
};
use crate::wiki_builder::{
    WikiBuilderDeps, WikiBuilderEnv, handle_extract_wiki, handle_wiki_backfill,
};
use chrono::Utc;
use kernel::memory::{EmbeddingStore, MemoryStore, SummaryStore};
use kernel::model::ModelProvider;
use kernel::queue::{ClaimedJob, FixedJob, JobName, JobQueue};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(crate) struct JobEnv {
    pub(crate) settings: crate::MemoryJobSettings,
    pub(crate) wiki_enabled: bool,
    pub(crate) memory_jobs: Option<Arc<dyn kernel::memory::jobs::MemoryJobStore>>,
    pub(crate) wiki: Option<WikiBuilderDeps>,
    pub(crate) targets: Arc<dyn EmbeddingStore>,
    pub(crate) summaries: Arc<dyn SummaryStore>,
    pub(crate) memory: Arc<dyn MemoryStore>,
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) embed: EmbedSettings,
    pub(crate) summary: SummarySettings,
    pub(crate) tags: TagSettings,
}

pub(crate) async fn dispatch<Q: JobQueue>(
    queue: &Q,
    env: &JobEnv,
    job: &ClaimedJob,
    cancel: &CancellationToken,
) -> Result<(), WorkerError> {
    let summarize = SummarizeEnv {
        store: env.summaries.as_ref(),
        provider: env.provider.as_ref(),
        queue,
        settings: &env.summary,
        now: Utc::now(),
        cancel,
    };
    match &job.name {
        JobName::Fixed(FixedJob::ExtractWiki | FixedJob::EnqueueWikiBackfill) => {
            dispatch_wiki(queue, env, job, cancel).await
        }
        JobName::Fixed(FixedJob::EmbedTarget) => {
            handle_embed(
                env.targets.as_ref(),
                env.provider.as_ref(),
                env.embed,
                &job.payload,
                cancel,
            )
            .await
        }
        JobName::Fixed(FixedJob::EnqueueUnembedded) => {
            dispatch_sweep(queue, env, Sweep::Unembedded, cancel).await
        }
        JobName::Fixed(FixedJob::EnqueueUnchunked) => {
            dispatch_sweep(queue, env, Sweep::Unchunked, cancel).await
        }
        JobName::Fixed(FixedJob::EnqueueStaleWiki) => {
            dispatch_sweep(queue, env, Sweep::StaleWiki, cancel).await
        }
        JobName::Fixed(FixedJob::SynthesizeToolCall) => {
            let store = env
                .memory_jobs
                .as_ref()
                .ok_or(WorkerError::MissingMemoryJobs)?;
            crate::tool_synthesis::handle_synthesize_tool_call(
                store.as_ref(),
                env.provider.as_ref(),
                &env.summary.model,
                &job.payload,
                cancel,
            )
            .await
        }
        JobName::Fixed(FixedJob::SummarizeConversation) => {
            handle_summarize(&summarize, &job.payload).await.map(|_| ())
        }
        JobName::Fixed(FixedJob::EnqueueIdle) => handle_enqueue_idle(&summarize).await.map(|_| ()),
        JobName::Fixed(FixedJob::EnqueueStaleSummaries) => {
            handle_enqueue_stale(&summarize).await.map(|_| ())
        }
        JobName::Fixed(FixedJob::ReapDeadJobs) => handle_reap(&summarize).await.map(|_| ()),
        JobName::Fixed(FixedJob::SuggestTags) => {
            let categorize = CategorizeEnv {
                store: env.memory.as_ref(),
                provider: env.provider.as_ref(),
                settings: &env.tags,
                now: Utc::now(),
                cancel,
                attempt: job.attempt,
            };
            handle_suggest(&categorize, &job.payload).await.map(|_| ())
        }
        JobName::Fixed(FixedJob::ConsolidateWiki | FixedJob::RecompileWiki | FixedJob::RunTask)
        | JobName::Heartbeat { .. }
        | JobName::RunTaskOn { .. } => Err(WorkerError::Unhandled {
            name: job.name.to_string(),
        }),
    }
}

async fn dispatch_wiki<Q: JobQueue>(
    queue: &Q,
    env: &JobEnv,
    job: &ClaimedJob,
    cancel: &CancellationToken,
) -> Result<(), WorkerError> {
    if !env.wiki_enabled {
        return Ok(());
    }
    let wiki = env.wiki.as_ref().ok_or(WorkerError::MissingDependencies {
        role: ProcessRole::WikiBuilder,
    })?;
    let wiki_env = WikiBuilderEnv {
        store: wiki.store.as_ref(),
        provider: env.provider.as_ref(),
        queue,
        model: &wiki.model,
        now: Utc::now(),
        cancel,
    };
    if job.name == JobName::Fixed(FixedJob::ExtractWiki) {
        handle_extract_wiki(&wiki_env, &job.payload)
            .await
            .map(|_| ())
    } else {
        handle_wiki_backfill(&wiki_env).await.map(|_| ())
    }
}

enum Sweep {
    Unembedded,
    Unchunked,
    StaleWiki,
}

async fn dispatch_sweep<Q: JobQueue>(
    queue: &Q,
    env: &JobEnv,
    sweep: Sweep,
    cancel: &CancellationToken,
) -> Result<(), WorkerError> {
    if matches!(sweep, Sweep::StaleWiki) && !env.wiki_enabled {
        return Ok(());
    }
    let store = env
        .memory_jobs
        .as_ref()
        .ok_or(WorkerError::MissingMemoryJobs)?;
    let store = store.as_ref();
    let settings = env.settings;
    match sweep {
        Sweep::Unembedded => crate::embedding_sweep::handle_enqueue_unembedded(
            store,
            queue,
            settings.sweep_limit,
            settings.sweep_max_attempts,
            cancel,
        )
        .await
        .map(|_| ()),
        Sweep::Unchunked => crate::embedding_sweep::handle_enqueue_unchunked(
            store,
            queue,
            env.embed,
            settings.chunk_backfill_limit,
            settings.sweep_max_attempts,
            cancel,
        )
        .await
        .map(|_| ()),
        Sweep::StaleWiki => crate::embedding_sweep::handle_enqueue_stale_wiki(
            store,
            queue,
            env.wiki_enabled,
            settings.wiki_sweep_limit,
            cancel,
        )
        .await
        .map(|_| ()),
    }
}
