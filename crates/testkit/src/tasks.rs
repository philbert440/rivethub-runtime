use std::time::Duration;

use kernel::filing::FilingStore;
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
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::TopicStore;
use uuid::Uuid;

use crate::topics::Outcome;
use crate::topics::at;
use crate::topics::new_topic;
use crate::topics::same;
use crate::topics::session;

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

pub async fn a_queued_task_times_out_when_no_node_starts_it_by_its_deadline(
    store: &dyn TaskStore,
) -> Outcome {
    let unclaimed = queued("coder", "node-gone", 1);
    let started = queued("coder", "node-b", 1);
    let mut open = queued("coder", "node-gone", 1);
    open.budget.wall_clock = None;
    for task in [&unclaimed, &started, &open] {
        store.insert(task).await?;
    }
    store.begin(started.id, "node-b", at(2)).await?;
    same("early", store.expire(at(4)).await?, Vec::<Uuid>::new())?;
    same("due", store.expire(at(5)).await?, vec![unclaimed.id])?;
    let expired = store.load(unclaimed.id).await?;
    same("status", expired.status, TaskStatus::Timeout)?;
    same(
        "reason",
        expired.error.as_deref(),
        Some(kernel::tasks::UNCLAIMED_PAST_DEADLINE),
    )?;
    same("completed", expired.completed_at, Some(at(5)))?;
    same(
        "running",
        store.load(started.id).await?.status,
        TaskStatus::Running,
    )?;
    same(
        "no deadline",
        store.load(open.id).await?.status,
        TaskStatus::Queued,
    )?;
    same("once", store.expire(at(9)).await?, Vec::<Uuid>::new())?;
    Ok(())
}

pub async fn a_delegated_child_is_filed_under_its_parents_topic(
    tasks: &dyn TaskStore,
    topics: &dyn TopicStore,
    filing: &dyn FilingStore,
) -> Outcome {
    let home = topics.create(new_topic("Ledger", &[], None)?).await?;
    let elsewhere = topics.create(new_topic("Elsewhere", &[], None)?).await?;
    let parent = session("parent")?;
    topics
        .place(Placement {
            session: parent.clone(),
            topic: home.id,
            by: PlacementSource::You,
            at: at(1),
            owner: Some(crate::owner()?),
        })
        .await?;
    let chain = [
        ("child", "parent"),
        ("grandchild", "child"),
        ("moved", "parent"),
    ];
    let mut ids = Vec::new();
    for (minute, (child, above)) in (2..).zip(chain) {
        let mut task = queued("coder", "node-b", minute);
        task.parent.session = Some(session(above)?.as_str().to_owned());
        tasks.insert(&task).await?;
        tasks
            .attach(task.id, session(child)?.as_str(), Harness::ClaudeCode)
            .await?;
        ids.push(task.id);
    }
    topics
        .place(Placement {
            session: session("moved")?,
            topic: elsewhere.id,
            by: PlacementSource::You,
            at: at(9),
            owner: Some(crate::owner()?),
        })
        .await?;
    listed_in(filing, home.id).await?;
    let all = filing.all_filed().await?;
    let child = all
        .sessions
        .iter()
        .find(|row| row.placement.session.as_str() == "claude-code:child")
        .ok_or("the child in all filings")?;
    same("topic", child.placement.topic, home.id)?;
    same(
        "task",
        child.lineage.as_ref().map(|lineage| lineage.task),
        ids.first().copied(),
    )?;
    same("node", child.node.as_deref(), Some("node-b"))?;
    same(
        "moved stays",
        all.sessions
            .iter()
            .filter(|row| row.placement.session.as_str() == "claude-code:moved")
            .map(|row| (row.placement.topic, row.lineage.is_some()))
            .collect::<Vec<_>>(),
        vec![(elsewhere.id, false)],
    )?;
    same(
        "delegated",
        filing
            .delegated(&kernel::filing::Target::Session(session("child")?))
            .await?,
        true,
    )?;
    same(
        "not delegated",
        filing
            .delegated(&kernel::filing::Target::Session(parent))
            .await?,
        false,
    )?;
    Ok(())
}

async fn listed_in(filing: &dyn FilingStore, topic: kernel::topics::TopicId) -> Outcome {
    let filed = filing.filed(topic).await?;
    let mut inherited: Vec<(String, Option<String>)> = filed
        .sessions
        .iter()
        .map(|row| {
            (
                row.placement.session.as_str().to_owned(),
                row.lineage
                    .as_ref()
                    .map(|lineage| lineage.parent.as_str().to_owned()),
            )
        })
        .collect();
    inherited.sort();
    same(
        "inherited",
        inherited,
        vec![
            (
                "claude-code:child".to_owned(),
                Some("claude-code:parent".to_owned()),
            ),
            (
                "claude-code:grandchild".to_owned(),
                Some("claude-code:child".to_owned()),
            ),
            ("claude-code:parent".to_owned(), None),
        ],
    )?;
    Ok(())
}
