use std::time::Duration;

use kernel::tasks::Budget;
use kernel::tasks::Criterion;
use kernel::tasks::Finish;
use kernel::tasks::Harness;
use kernel::tasks::Outcome as TaskOutcome;
use kernel::tasks::Parent;
use kernel::tasks::ParentSource;
use kernel::tasks::Target;
use kernel::tasks::Task;
use kernel::tasks::TaskFilter;
use kernel::tasks::TaskStatus;
use kernel::tasks::TaskStore;
use kernel::tasks::Usage;
use kernel::tasks::Verdict;
use uuid::Uuid;

use crate::topics::Outcome;
use crate::topics::at;
use crate::topics::same;

#[must_use]
pub fn queued(agent: &str, node: &str, minute: u32) -> Task {
    Task {
        id: Uuid::now_v7(),
        goal: format!("work for {agent}"),
        criteria: vec![Criterion {
            id: "1".to_owned(),
            description: "it is done".to_owned(),
        }],
        budget: Budget {
            wall_clock: Some(Duration::from_secs(240)),
            max_turns: Some(8),
        },
        status: TaskStatus::Queued,
        target: Target {
            agent: agent.to_owned(),
            preset: Some("preset-1".to_owned()),
            node: node.to_owned(),
            harness: Some(Harness::ClaudeCode),
            model: None,
            effort: Some("high".to_owned()),
            cwd: None,
            permission_mode: Some("bypassPermissions".to_owned()),
        },
        parent: Parent::default(),
        child: None,
        result: None,
        error: None,
        usage: None,
        requested_by: Some("tests".to_owned()),
        owner: "owner".to_owned(),
        created_at: at(minute),
        started_at: None,
        completed_at: None,
        alive_at: None,
    }
}

pub async fn a_task_runs_once_on_its_node_and_only_a_kill_ends_it_early(
    store: &dyn TaskStore,
) -> Outcome {
    let mut task = queued("coder", "node-b", 1);
    task.parent = Parent {
        session: Some("claude-code:parent".to_owned()),
        task: None,
        depth: 0,
        source: Some(ParentSource::Request),
    };
    store.insert(&task).await?;
    let other = queued("grok", "node-c", 2);
    store.insert(&other).await?;
    same("stored", store.load(task.id).await?, task.clone())?;
    let listed = store.list(&TaskFilter::default()).await?;
    same(
        "newest first",
        listed.iter().map(|row| row.id).collect(),
        vec![other.id, task.id],
    )?;
    let filtered = store
        .list(&TaskFilter {
            agent: Some("coder".to_owned()),
            status: Some(TaskStatus::Queued),
            limit: Some(5),
        })
        .await?;
    same("filtered", filtered.len(), 1)?;
    runs_on_its_node(store, &task).await?;
    let finish = finished(store, &task).await?;
    same(
        "kill keeps",
        store.kill(task.id, at(13)).await?,
        TaskStatus::Completed,
    )?;
    same(
        "kill queued",
        store.kill(other.id, at(13)).await?,
        TaskStatus::Queued,
    )?;
    same(
        "killed",
        store.load(other.id).await?.status,
        TaskStatus::Killed,
    )?;
    same(
        "a queued task never completes",
        store.finish(other.id, &finish).await?,
        false,
    )?;
    Ok(())
}

async fn runs_on_its_node(store: &dyn TaskStore, task: &Task) -> Outcome {
    same(
        "another node",
        store.begin(task.id, "node-a", at(3)).await?,
        false,
    )?;
    same(
        "its node",
        store.begin(task.id, "node-b", at(3)).await?,
        true,
    )?;
    same("once", store.begin(task.id, "node-b", at(3)).await?, false)?;
    store
        .attach(task.id, "claude-code:child", Harness::ClaudeCode)
        .await?;
    same(
        "by child",
        store.by_child("claude-code:child").await?.map(|row| row.id),
        Some(task.id),
    )?;
    same(
        "asks",
        store.flip(task.id, TaskStatus::AwaitingInput).await?,
        true,
    )?;
    same(
        "answered",
        store.flip(task.id, TaskStatus::Running).await?,
        true,
    )?;
    store.alive(&[task.id], at(10)).await?;
    same(
        "alive",
        store.reap(at(9), at(11)).await?,
        Vec::<Uuid>::new(),
    )?;
    Ok(())
}

async fn finished(
    store: &dyn TaskStore,
    task: &Task,
) -> Result<Finish, Box<dyn std::error::Error>> {
    let finish = Finish {
        status: TaskStatus::Completed,
        result: Some(TaskOutcome {
            verdict: Verdict::Completed,
            summary: "done".to_owned(),
        }),
        error: None,
        usage: Some(Usage {
            input_tokens: Some(3),
            output_tokens: Some(4),
            turns: 2,
            wall_clock_ms: 900,
        }),
        at: at(12),
    };
    same("finished", store.finish(task.id, &finish).await?, true)?;
    same("final", store.finish(task.id, &finish).await?, false)?;
    let done = store.load(task.id).await?;
    same("completed", done.status, TaskStatus::Completed)?;
    same("result", done.result, finish.result.clone())?;
    same("usage", done.usage, finish.usage)?;
    Ok(finish)
}

pub async fn a_running_task_is_reaped_only_when_its_liveness_goes_stale(
    store: &dyn TaskStore,
) -> Outcome {
    let live = queued("coder", "node-b", 1);
    let dead = queued("coder", "node-c", 1);
    for task in [&live, &dead] {
        store.insert(task).await?;
        store.begin(task.id, &task.target.node, at(2)).await?;
    }
    store.alive(&[live.id], at(30)).await?;
    same("stale", store.reap(at(20), at(31)).await?, vec![dead.id])?;
    same(
        "reaped",
        store.load(dead.id).await?.status,
        TaskStatus::Failed,
    )?;
    same(
        "alive",
        store.load(live.id).await?.status,
        TaskStatus::Running,
    )?;
    same(
        "restart",
        store.orphans("node-b", at(32)).await?,
        vec![live.id],
    )?;
    same(
        "restart error",
        store.load(live.id).await?.error.as_deref(),
        Some("its gateway restarted"),
    )?;
    Ok(())
}
