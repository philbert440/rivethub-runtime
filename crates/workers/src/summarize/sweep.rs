use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use kernel::memory::ConversationWindow;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;
use wire::JsonObject;

use super::SummarizeEnv;
use super::leaf::count_of;
use super::llm::cancelled;
use super::settings::DEAD_REST;
use super::settings::SummaryTrigger;
use crate::error::WorkerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryHealth {
    pub eligible: u64,
    pub active_tail: u64,
    pub below_floor: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SummarizePayload {
    pub(super) conversation_id: Uuid,
    pub(super) trigger_type: SummaryTrigger,
}

pub(super) fn read_summarize(payload: &JsonObject) -> Result<SummarizePayload, WorkerError> {
    let value = serde_json::to_value(payload).map_err(WorkerError::summary_payload)?;
    serde_json::from_value(value).map_err(WorkerError::summary_payload)
}

pub fn summary_health(
    windows: &[ConversationWindow],
    settings: &super::SummarySettings,
) -> Result<SummaryHealth, WorkerError> {
    let batch = u64::from(settings.leaf_batch);
    let floor = u64::from(settings.idle_floor());
    if batch == 0 {
        return Err(WorkerError::SummarySettings {
            message: "leaf batch must be at least 1".to_owned(),
        });
    }
    let mut health = SummaryHealth {
        eligible: 0,
        active_tail: 0,
        below_floor: 0,
    };
    for window in windows {
        let count = u64::try_from(window.messages.len()).map_err(|_| WorkerError::Count)?;
        let full = count / batch * batch;
        let rest = count % batch;
        health.eligible = health
            .eligible
            .checked_add(full)
            .ok_or(WorkerError::Count)?;
        if rest >= floor {
            health.active_tail = health
                .active_tail
                .checked_add(rest)
                .ok_or(WorkerError::Count)?;
        } else {
            health.below_floor = health
                .below_floor
                .checked_add(rest)
                .ok_or(WorkerError::Count)?;
        }
    }
    Ok(health)
}

pub async fn handle_enqueue_idle(env: &SummarizeEnv<'_>) -> Result<u64, WorkerError> {
    cancelled(env)?;
    let windows = env.store.pending().await?;
    let mut enqueued = 0u64;
    for window in windows {
        if let Some(trigger) = idle_trigger(&window, env)? {
            enqueue(env, window.conversation.id, trigger).await?;
            enqueued = enqueued.checked_add(1).ok_or(WorkerError::Count)?;
        }
    }
    Ok(enqueued)
}

pub async fn handle_enqueue_stale(env: &SummarizeEnv<'_>) -> Result<u64, WorkerError> {
    cancelled(env)?;
    let windows = env.store.pending().await?;
    let mut enqueued = 0u64;
    for window in windows {
        if stale_due(&window, env)? {
            enqueue(env, window.conversation.id, SummaryTrigger::Stale).await?;
            enqueued = enqueued.checked_add(1).ok_or(WorkerError::Count)?;
        }
    }
    Ok(enqueued)
}

pub async fn handle_reap(env: &SummarizeEnv<'_>) -> Result<u64, WorkerError> {
    cancelled(env)?;
    let cutoff = env
        .now
        .checked_sub_signed(DEAD_REST)
        .ok_or(WorkerError::Clock)?;
    env.queue
        .revive_dead(&JobName::Fixed(FixedJob::SummarizeConversation), cutoff)
        .await
        .map_err(WorkerError::queue)
}

pub(super) async fn enqueue_again(
    env: &SummarizeEnv<'_>,
    id: Uuid,
    trigger: SummaryTrigger,
) -> Result<(), WorkerError> {
    enqueue(env, id, trigger).await
}

pub(super) fn unfinished(
    window: &ConversationWindow,
    trigger: SummaryTrigger,
    env: &SummarizeEnv<'_>,
) -> Result<bool, WorkerError> {
    let available = count_of(window.messages.len())?;
    let leaves = count_of(window.leaves.len())?;
    let branches = count_of(window.branches.len())?;
    let full = available >= env.settings.leaf_batch;
    let partial = partial_allowed(trigger, window, available, env.settings.leaf_batch, env);
    let more_leaves = leaves >= env.settings.min_leaves_for_branch;
    let more_branches = branches >= env.settings.min_branches_for_root;
    Ok(full || partial || more_leaves || more_branches)
}

pub(super) fn partial_allowed(
    trigger: SummaryTrigger,
    window: &ConversationWindow,
    count: u32,
    batch: u32,
    env: &SummarizeEnv<'_>,
) -> bool {
    if count == 0 || count >= batch {
        return false;
    }
    let Some(age) = message_age(window.latest_message_at, env.now) else {
        return false;
    };
    match trigger {
        SummaryTrigger::Window => false,
        SummaryTrigger::SessionIdle => {
            age >= env.settings.idle && count >= env.settings.idle_floor()
        }
        SummaryTrigger::Stale => age >= env.settings.stale && count >= env.settings.stale_min_batch,
    }
}

fn idle_trigger(
    window: &ConversationWindow,
    env: &SummarizeEnv<'_>,
) -> Result<Option<SummaryTrigger>, WorkerError> {
    let count = count_of(window.messages.len())?;
    if count >= env.settings.leaf_batch {
        return Ok(Some(SummaryTrigger::Window));
    }
    if partial_allowed(
        SummaryTrigger::SessionIdle,
        window,
        count,
        env.settings.leaf_batch,
        env,
    ) {
        Ok(Some(SummaryTrigger::SessionIdle))
    } else {
        Ok(None)
    }
}

fn stale_due(window: &ConversationWindow, env: &SummarizeEnv<'_>) -> Result<bool, WorkerError> {
    let count = count_of(window.messages.len())?;
    if count < env.settings.stale_min_batch {
        return Ok(false);
    }
    let Some(age) = message_age(window.latest_message_at, env.now) else {
        return Ok(false);
    };
    Ok(age >= env.settings.stale)
}

async fn enqueue(
    env: &SummarizeEnv<'_>,
    id: Uuid,
    trigger: SummaryTrigger,
) -> Result<(), WorkerError> {
    env.queue
        .enqueue(summarize_job(id, trigger, env.now)?)
        .await
        .map_err(WorkerError::queue)
}

fn summarize_job(
    id: Uuid,
    trigger: SummaryTrigger,
    now: DateTime<Utc>,
) -> Result<Job, WorkerError> {
    let payload = SummarizePayload {
        conversation_id: id,
        trigger_type: trigger,
    };
    Ok(Job {
        name: JobName::Fixed(FixedJob::SummarizeConversation),
        payload: JsonObject::from_serialize(&payload).map_err(WorkerError::summary_payload)?,
        key: Some(format!("compact-{id}")),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        run_at: Some(now),
    })
}

fn message_age(latest: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<TimeDelta> {
    latest.map(|at| now.signed_duration_since(at))
}
