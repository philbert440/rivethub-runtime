use std::collections::HashMap;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use kernel::model::Embedding;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use kernel::model::ModelRequest;
use kernel::model::ModelStreamItem;
use kernel::queue::ClaimedJob;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use wire::JsonObject;
use wire::stream::LlmResponse;
use workers::EmbedSettings;
use workers::EmbedTable;
use workers::EmbedTarget;
use workers::EmbedTargets;
use workers::LoadedTarget;
use workers::StoredEmbedding;
use workers::Unembeddable;
use workers::WorkerError;
use workers::accepts;
use workers::handle_embed;

pub const WIDTH: u32 = 8;

pub fn settings() -> EmbedSettings {
    EmbedSettings::standard().with_width(WIDTH)
}

pub fn width() -> usize {
    usize::try_from(WIDTH).expect("the test width fits usize")
}

pub fn target_payload(table: &str, id: &str) -> JsonObject {
    JsonObject::from_serialize(&TargetBody {
        target_table: table.to_owned(),
        target_id: id.to_owned(),
    })
    .expect("an embed target payload serializes")
}

pub async fn embed_target(
    store: &Store,
    provider: &dyn ModelProvider,
    embed_settings: EmbedSettings,
    payload: &JsonObject,
    cancel: &CancellationToken,
) -> Result<(), WorkerError> {
    handle_embed(store, provider, embed_settings, payload, cancel).await
}

pub async fn wait_bool(rx: &mut watch::Receiver<bool>, want: bool) {
    wait_watch(rx, |value| *value == want, "boolean watch").await;
}

pub async fn wait_at_least(rx: &mut watch::Receiver<usize>, count: usize) {
    wait_watch(rx, |value| *value >= count, "count watch").await;
}

async fn wait_watch<T>(rx: &mut watch::Receiver<T>, ready: impl Fn(&T) -> bool, label: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if ready(&rx.borrow()) {
                return;
            }
            rx.changed().await.expect(label);
        }
    })
    .await
    .expect(label);
}

pub fn same_bits(actual: &[f32], expected: &[f32]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(left, right)| left.to_bits() == right.to_bits())
}

#[derive(Serialize)]
struct TargetBody {
    #[serde(rename = "targetTable")]
    target_table: String,
    #[serde(rename = "targetId")]
    target_id: String,
}

#[derive(Debug, Clone)]
struct TargetRow {
    loaded: LoadedTarget,
    failures: u32,
    written: Option<StoredEmbedding>,
    unembeddable: Option<Unembeddable>,
    permanent: bool,
    embed_error: Option<String>,
    embed_status: Option<String>,
}

#[derive(Debug)]
pub struct Store {
    rows: Mutex<HashMap<(EmbedTable, String), TargetRow>>,
    loads: watch::Sender<usize>,
    hold: bool,
    released: watch::Sender<bool>,
}

impl Store {
    pub fn new() -> Self {
        Self::build(false)
    }

    pub fn holding() -> Self {
        Self::build(true)
    }

    fn build(hold: bool) -> Self {
        let (loads, _) = watch::channel(0);
        let (released, _) = watch::channel(false);
        Self {
            rows: Mutex::new(HashMap::new()),
            loads,
            hold,
            released,
        }
    }

    pub async fn insert(
        &self,
        table: EmbedTable,
        id: &str,
        content: &str,
        tool_result: Option<&str>,
    ) -> Result<(), String> {
        let mut rows = self.rows.lock().await;
        let key = (table, id.to_owned());
        if rows.contains_key(&key) {
            return Err(format!("duplicate embed target {id}"));
        }
        rows.insert(
            key,
            TargetRow {
                loaded: LoadedTarget {
                    content: content.to_owned(),
                    tool_result: tool_result.map(str::to_owned),
                },
                failures: 0,
                written: None,
                unembeddable: None,
                permanent: false,
                embed_error: None,
                embed_status: None,
            },
        );
        drop(rows);
        Ok(())
    }

    pub fn loads(&self) -> watch::Receiver<usize> {
        self.loads.subscribe()
    }

    pub fn load_count(&self) -> usize {
        *self.loads.subscribe().borrow()
    }

    pub fn release(&self) {
        let _previous = self.released.send_replace(true);
    }

    pub async fn written(&self, table: EmbedTable, id: &str) -> Option<StoredEmbedding> {
        self.rows
            .lock()
            .await
            .get(&(table, id.to_owned()))
            .and_then(|row| row.written.clone())
    }

    pub async fn failures(&self, table: EmbedTable, id: &str) -> u32 {
        self.row(table, id).await.failures
    }

    pub async fn permanent(&self, table: EmbedTable, id: &str) -> bool {
        self.row(table, id).await.permanent
    }

    pub async fn embed_error(&self, table: EmbedTable, id: &str) -> Option<String> {
        self.row(table, id).await.embed_error
    }

    pub async fn embed_status(&self, table: EmbedTable, id: &str) -> Option<String> {
        self.row(table, id).await.embed_status
    }

    pub async fn unembeddable(&self, table: EmbedTable, id: &str) -> Option<Unembeddable> {
        self.rows
            .lock()
            .await
            .get(&(table, id.to_owned()))
            .and_then(|row| row.unembeddable)
    }

    pub async fn writes(&self) -> usize {
        self.rows
            .lock()
            .await
            .values()
            .filter(|row| row.written.is_some())
            .count()
    }

    async fn row(&self, table: EmbedTable, id: &str) -> TargetRow {
        self.rows
            .lock()
            .await
            .get(&(table, id.to_owned()))
            .cloned()
            .expect("the embed target exists")
    }

    async fn enter(&self) -> Result<(), WorkerError> {
        if !self.hold {
            return Ok(());
        }
        self.loads.send_modify(|count| {
            *count = count.saturating_add(1);
        });
        let mut released = self.released.subscribe();
        while !*released.borrow() {
            released.changed().await.map_err(|_| WorkerError::Closed)?;
        }
        Ok(())
    }
}

fn target_key(target: &EmbedTarget) -> (EmbedTable, String) {
    (target.table, target.id.clone())
}

#[async_trait]
impl EmbedTargets for Store {
    async fn load(&self, target: &EmbedTarget) -> Result<Option<LoadedTarget>, WorkerError> {
        self.enter().await?;
        let loaded = self
            .rows
            .lock()
            .await
            .get(&target_key(target))
            .map(|row| row.loaded.clone());
        Ok(loaded)
    }

    async fn write(&self, stored: StoredEmbedding) -> Result<(), WorkerError> {
        let mut rows = self.rows.lock().await;
        let Some(row) = rows.get_mut(&target_key(&stored.target)) else {
            return Err(WorkerError::Target(format!("missing {}", stored.target.id)));
        };
        row.written = Some(stored);
        row.failures = 0;
        row.permanent = false;
        row.embed_error = None;
        row.embed_status = None;
        drop(rows);
        Ok(())
    }

    async fn mark_unembeddable(
        &self,
        target: &EmbedTarget,
        reason: Unembeddable,
    ) -> Result<(), WorkerError> {
        let mut rows = self.rows.lock().await;
        let Some(row) = rows.get_mut(&target_key(target)) else {
            return Err(WorkerError::Target(format!("missing {}", target.id)));
        };
        row.unembeddable = Some(reason);
        row.embed_status = Some("unembeddable".to_owned());
        row.embed_error = Some(reason.error_text());
        drop(rows);
        Ok(())
    }

    async fn record_null(&self, target: &EmbedTarget, error: &str) -> Result<u32, WorkerError> {
        let mut rows = self.rows.lock().await;
        let Some(row) = rows.get_mut(&target_key(target)) else {
            return Err(WorkerError::Target(format!("missing {}", target.id)));
        };
        row.failures = row.failures.saturating_add(1);
        row.embed_error = Some(error.to_owned());
        let failures = row.failures;
        drop(rows);
        Ok(failures)
    }

    async fn mark_permanent(&self, target: &EmbedTarget, error: &str) -> Result<(), WorkerError> {
        let mut rows = self.rows.lock().await;
        let Some(row) = rows.get_mut(&target_key(target)) else {
            return Err(WorkerError::Target(format!("missing {}", target.id)));
        };
        row.permanent = true;
        row.embed_status = Some("failed".to_owned());
        row.embed_error = Some(error.to_owned());
        drop(rows);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Ready,
    Running,
    Backoff,
    Dead,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobView {
    pub id: JobId,
    pub name: String,
    pub phase: Phase,
    pub attempt: u32,
    pub error: Option<String>,
}

#[derive(Debug)]
struct Queued {
    id: JobId,
    name: JobName,
    payload: JsonObject,
    attempt: u32,
    max_attempts: u32,
    phase: Phase,
    bypass: bool,
    priority: i16,
    last_error: Option<String>,
}

#[derive(Debug)]
pub struct Queue {
    rows: Mutex<Vec<Queued>>,
    next_id: AtomicI64,
    claim_error: Mutex<Option<JobQueueError>>,
    claims: AtomicUsize,
    events: watch::Sender<u64>,
}

impl Queue {
    pub fn new() -> Self {
        let (events, _) = watch::channel(0);
        Self {
            rows: Mutex::new(Vec::new()),
            next_id: AtomicI64::new(1),
            claim_error: Mutex::new(None),
            claims: AtomicUsize::new(0),
            events,
        }
    }

    pub async fn push(&self, name: JobName, payload: JsonObject, max_attempts: u32) -> JobId {
        self.add(name, payload, 0, max_attempts, false).await
    }

    pub async fn push_any(&self, name: JobName, payload: JsonObject, max_attempts: u32) -> JobId {
        self.add(name, payload, 0, max_attempts, true).await
    }

    pub async fn fail_next_claim(&self) {
        let error = JobQueueError::Unavailable(Box::new(std::io::Error::other("queue down")));
        *self.claim_error.lock().await = Some(error);
    }

    pub fn claims(&self) -> usize {
        self.claims.load(Ordering::Relaxed)
    }

    pub async fn wait_claims(&self, count: usize) {
        let mut events = self.events.subscribe();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.claims() >= count {
                    return;
                }
                events.changed().await.expect("queue watch");
            }
        })
        .await
        .expect("the claim count");
    }

    pub async fn view(&self, id: JobId) -> JobView {
        self.rows
            .lock()
            .await
            .iter()
            .find(|row| row.id == id)
            .map(snapshot)
            .expect("the queued job exists")
    }

    pub async fn wait_phase(&self, id: JobId, phase: Phase) {
        let mut events = self.events.subscribe();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.phase_of(id).await == Some(phase) {
                    return;
                }
                events.changed().await.expect("queue watch");
            }
        })
        .await
        .expect("the job phase");
    }

    async fn phase_of(&self, id: JobId) -> Option<Phase> {
        self.rows
            .lock()
            .await
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.phase)
    }

    async fn add(
        &self,
        name: JobName,
        payload: JsonObject,
        priority: i16,
        max_attempts: u32,
        bypass: bool,
    ) -> JobId {
        let id = JobId::new(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.rows.lock().await.push(Queued {
            id,
            name,
            payload,
            attempt: 0,
            max_attempts,
            phase: Phase::Ready,
            bypass,
            priority,
            last_error: None,
        });
        self.bump();
        id
    }

    fn bump(&self) {
        self.events.send_modify(|tick| {
            *tick = tick.wrapping_add(1);
        });
    }
}

fn snapshot(row: &Queued) -> JobView {
    JobView {
        id: row.id,
        name: row.name.to_string(),
        phase: row.phase,
        attempt: row.attempt,
        error: row.last_error.clone(),
    }
}

fn select_ready(rows: &[Queued], role: WorkerRole) -> Option<usize> {
    let mut chosen: Option<usize> = None;
    for (index, row) in rows.iter().enumerate() {
        if row.phase != Phase::Ready || !(row.bypass || accepts(role, &row.name)) {
            continue;
        }
        chosen = Some(match chosen {
            None => index,
            Some(current) => prefer(rows, current, index),
        });
    }
    chosen
}

fn prefer(rows: &[Queued], current: usize, index: usize) -> usize {
    let Some(left) = rows.get(current) else {
        return index;
    };
    let Some(right) = rows.get(index) else {
        return current;
    };
    if right.priority < left.priority || (right.priority == left.priority && right.id < left.id) {
        index
    } else {
        current
    }
}

fn default_max_attempts(name: &JobName) -> u32 {
    match name {
        JobName::Fixed(fixed) => match fixed {
            FixedJob::EmbedTarget => 5,
            FixedJob::CompactConversation | FixedJob::SynthesizeToolCall => 3,
            FixedJob::SuggestTags | FixedJob::ExtractWiki => 2,
            FixedJob::RunTask => 1,
            FixedJob::EnqueueUnembedded
            | FixedJob::EnqueueUnchunked
            | FixedJob::EnqueueIdle
            | FixedJob::EnqueueWikiBackfill
            | FixedJob::ConsolidateWiki
            | FixedJob::RecompileWiki
            | FixedJob::EnqueueStaleWiki
            | FixedJob::EnqueueStaleCompaction
            | FixedJob::ReapDeadJobs => 25,
        },
        JobName::Heartbeat { .. } | JobName::RunTaskOn { .. } => 1,
    }
}

fn missing(id: JobId) -> JobQueueError {
    JobQueueError::Claim {
        message: format!("job {id} is not running"),
        source: Box::new(std::io::Error::other("job is not running")),
    }
}

#[async_trait]
impl JobQueue for Queue {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError> {
        let max_attempts = default_max_attempts(&job.name);
        self.add(job.name, job.payload, job.priority, max_attempts, false)
            .await;
        Ok(())
    }

    async fn claim(&self, role: WorkerRole) -> Result<Option<ClaimedJob>, JobQueueError> {
        let pending = self.claim_error.lock().await.take();
        if let Some(error) = pending {
            return Err(error);
        }
        let claimed = {
            let mut rows = self.rows.lock().await;
            let Some(index) = select_ready(&rows, role) else {
                return Ok(None);
            };
            let Some(row) = rows.get_mut(index) else {
                return Err(JobQueueError::Claim {
                    message: "the ready job index is outside the queue".to_owned(),
                    source: Box::new(std::io::Error::other("ready job index")),
                });
            };
            row.attempt = row.attempt.saturating_add(1);
            row.phase = Phase::Running;
            self.claims.fetch_add(1, Ordering::Relaxed);
            let claimed = ClaimedJob {
                id: row.id,
                name: row.name.clone(),
                payload: row.payload.clone(),
                attempt: row.attempt,
            };
            drop(rows);
            claimed
        };
        self.bump();
        Ok(Some(claimed))
    }

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError> {
        {
            let mut rows = self.rows.lock().await;
            let Some(row) = rows.iter_mut().find(|row| row.id == id) else {
                return Err(missing(id));
            };
            if row.phase != Phase::Running {
                return Err(missing(id));
            }
            row.phase = Phase::Done;
            drop(rows);
        }
        self.bump();
        Ok(())
    }

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobQueueError> {
        {
            let mut rows = self.rows.lock().await;
            let Some(row) = rows.iter_mut().find(|row| row.id == id) else {
                return Err(missing(id));
            };
            if row.phase != Phase::Running {
                return Err(missing(id));
            }
            row.last_error = Some(error.to_owned());
            row.phase = if row.attempt >= row.max_attempts {
                Phase::Dead
            } else {
                Phase::Backoff
            };
            drop(rows);
        }
        self.bump();
        Ok(())
    }
}

#[derive(Debug)]
pub struct HangEmbed {
    started: watch::Sender<bool>,
}

impl HangEmbed {
    pub fn new() -> Self {
        let (started, _) = watch::channel(false);
        Self { started }
    }

    pub fn started(&self) -> watch::Receiver<bool> {
        self.started.subscribe()
    }
}

fn rejected(message: &str) -> ModelProviderError {
    ModelProviderError::Request {
        message: message.to_owned(),
        source: Box::new(std::io::Error::other(message.to_owned())),
    }
}

#[async_trait]
impl ModelProvider for HangEmbed {
    async fn chat(&self, _request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        Err(rejected("the hang provider has no chat"))
    }

    async fn stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Receiver<ModelStreamItem>, ModelProviderError> {
        Err(rejected("the hang provider has no stream"))
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        drop(texts);
        let _previous = self.started.send_replace(true);
        std::future::pending().await
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Ok(Vec::new())
    }
}
