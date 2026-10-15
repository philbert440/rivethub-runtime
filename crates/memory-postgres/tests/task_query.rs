use std::time::Duration;

use chrono::TimeDelta;
use chrono::Utc;
use kernel::tasks::Finish;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use memory_postgres::PostgresTasks;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPromptMode;
use wire::task::TaskSpec;
use wire::task::TaskStatus;

fn record(id: Uuid, agent: &str, created_offset_secs: i64) -> TaskRecord {
    let session = format!("task:{id}");
    let created_at = Utc::now() + TimeDelta::seconds(created_offset_secs);
    TaskRecord {
        id,
        goal: agent.to_owned(),
        context_refs: Vec::new(),
        acceptance_criteria: Vec::new(),
        spec: TaskSpec {
            task_id: id.to_string(),
            agent_id: agent.to_owned(),
            goal: agent.to_owned(),
            resolved_context: Vec::new(),
            acceptance_criteria: Vec::new(),
            budget: TaskBudget::default(),
            tools: None,
            working_dir: None,
            model: None,
            effort: None,
            system_prompt_append: None,
            isolation: None,
            prompt_mode: Some(TaskPromptMode::Task),
            exclude_tools: None,
            resume_message: None,
            session: session.clone(),
            permission_decisions: None,
        },
        executor: TaskExecutorKind::ChatLoop,
        executor_target: None,
        requested_by: None,
        agent_id: agent.to_owned(),
        origin: TaskOrigin::Api,
        parent_task_id: None,
        chain_depth: 0,
        budget: TaskBudget::default(),
        node_affinity: None,
        usage: None,
        status: TaskStatus::Queued,
        attempt: 0,
        max_attempts: 1,
        result: None,
        error: None,
        conversation_id: None,
        session_key: Some(session),
        pending_message: None,
        claimed_by: None,
        harness_session_ids: Vec::new(),
        created_at,
        started_at: None,
        last_heartbeat_at: None,
        completed_at: None,
        duration_ms: None,
        eval: None,
        eval_attempt: 0,
    }
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn list_filters_rows_and_terminal_updates_wake_the_watch(pool: PgPool) {
    let postgres = PostgresTasks::from_pool(pool);
    let older = Uuid::now_v7();
    let newer = Uuid::now_v7();
    postgres
        .insert(&record(older, "alpha", -10))
        .await
        .expect("older");
    postgres
        .insert(&record(newer, "beta", 10))
        .await
        .expect("newer");

    let all = postgres
        .list(&TaskListFilter::default())
        .await
        .expect("list");
    assert_eq!(
        all.iter().map(|task| task.id).collect::<Vec<_>>(),
        vec![newer, older]
    );

    let by_agent = postgres
        .list(&TaskListFilter {
            agent_id: Some("alpha".to_owned()),
            ..TaskListFilter::default()
        })
        .await
        .expect("agent");
    assert_eq!(by_agent.len(), 1);
    assert_eq!(by_agent.first().map(|task| task.id), Some(older));

    let none = postgres
        .list(&TaskListFilter {
            limit: Some(0),
            ..TaskListFilter::default()
        })
        .await
        .expect("limit");
    assert_eq!(none, Vec::new());

    let mut watch = TaskWatch::subscribe(&postgres);
    let cancel = CancellationToken::new();
    let handle = postgres.spawn_watch(cancel.clone()).await.expect("listen");
    postgres
        .finish(
            newer,
            &Finish {
                status: TaskStatus::Completed,
                result: None,
                error: None,
                usage: None,
                completed_at: Utc::now(),
                duration_ms: None,
            },
        )
        .await
        .expect("finish");
    tokio::time::timeout(Duration::from_secs(5), watch.changed())
        .await
        .expect("watch timeout")
        .expect("watch closed");
    assert!(*watch.borrow() >= 1);

    let queued = postgres
        .list(&TaskListFilter {
            status: Some(TaskStatus::Queued),
            ..TaskListFilter::default()
        })
        .await
        .expect("queued");
    assert_eq!(queued.len(), 1);
    assert_eq!(queued.first().map(|task| task.id), Some(older));

    cancel.cancel();
    handle.await.expect("join").expect("drive");
}
