use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use kernel::tasks::Finish;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use memory_sqlite::SqliteStore;
use uuid::Uuid;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskSpec;
use wire::task::TaskStatus;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn open() -> TestResult<(tempfile::TempDir, SqliteStore)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("memory.sqlite");
    migrate::sqlite(&path)?;
    let store = SqliteStore::open(&path)?;
    Ok((dir, store))
}

fn spec(id: Uuid, agent: &str) -> TaskSpec {
    TaskSpec {
        delegation: None,
        mesh_from: None,
        task_id: id.to_string(),
        agent_id: agent.to_owned(),
        goal: "reconcile the ledger".to_owned(),
        resolved_context: Vec::new(),
        acceptance_criteria: Vec::new(),
        budget: TaskBudget::default(),
        tools: None,
        working_dir: None,
        model: None,
        effort: None,
        system_prompt_append: None,
        isolation: None,
        prompt_mode: None,
        exclude_tools: None,
        resume_message: None,
        session: format!("task:{id}"),
        permission_decisions: None,
        preset_id: None,
        preset_name: None,
        audit_only: None,
        extra: wire::Extra::default(),
    }
}

fn queued(agent: &str, created_at: DateTime<Utc>) -> TaskRecord {
    let id = Uuid::now_v7();
    TaskRecord {
        id,
        goal: "reconcile the ledger".to_owned(),
        context_refs: Vec::new(),
        acceptance_criteria: Vec::new(),
        spec: spec(id, agent),
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
        session_key: Some(format!("task:{id}")),
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

fn ids(records: &[TaskRecord]) -> Vec<Uuid> {
    records.iter().map(|record| record.id).collect()
}

#[tokio::test]
async fn listing_filters_by_status_and_agent_newest_first_within_the_limit() -> TestResult {
    let (_dir, store) = open()?;
    let start = Utc::now() - Duration::minutes(10);
    let oldest = queued("main", start);
    let middle = queued("reviewer", start + Duration::minutes(1));
    let newest = queued("main", start + Duration::minutes(2));
    for record in [&oldest, &middle, &newest] {
        store.insert(record).await?;
    }
    store.kill(middle.id, Utc::now()).await?;
    let all = store.list(&TaskListFilter::default()).await?;
    assert_eq!(ids(&all), vec![newest.id, middle.id, oldest.id]);
    let killed = TaskListFilter {
        status: Some(TaskStatus::Killed),
        ..TaskListFilter::default()
    };
    assert_eq!(ids(&store.list(&killed).await?), vec![middle.id]);
    let main = TaskListFilter {
        agent_id: Some("main".to_owned()),
        ..TaskListFilter::default()
    };
    assert_eq!(ids(&store.list(&main).await?), vec![newest.id, oldest.id]);
    let one = TaskListFilter {
        limit: Some(1),
        ..TaskListFilter::default()
    };
    assert_eq!(ids(&store.list(&one).await?), vec![newest.id]);
    Ok(())
}

#[tokio::test]
async fn listing_an_untouched_store_returns_no_tasks() -> TestResult {
    let (_dir, store) = open()?;
    let listed = store.list(&TaskListFilter::default()).await?;
    assert_eq!(ids(&listed), Vec::<Uuid>::new());
    Ok(())
}

#[tokio::test]
async fn the_watch_moves_only_when_a_task_reaches_a_terminal_status() -> TestResult {
    let (_dir, store) = open()?;
    let mut watch = TaskWatch::subscribe(&store);
    let record = queued("main", Utc::now());
    store.insert(&record).await?;
    store.set_pending(record.id, "also check March").await?;
    store
        .mark_running(record.id, "node:agent", "node", Utc::now())
        .await?;
    assert!(!watch.has_changed()?);
    let finish = Finish {
        status: TaskStatus::Completed,
        result: None,
        error: None,
        usage: None,
        completed_at: Utc::now(),
        duration_ms: Some(5),
    };
    assert!(store.finish(record.id, &finish).await?);
    assert!(watch.has_changed()?);
    assert!(*watch.borrow_and_update() > 0);
    let replaced = queued("main", Utc::now());
    let session = TaskRecord {
        session_key: Some("heartbeat:main".to_owned()),
        ..replaced
    };
    store.insert(&session).await?;
    assert!(!watch.has_changed()?);
    store
        .kill_queued_by_session("heartbeat:main", Utc::now())
        .await?;
    assert!(watch.has_changed()?);
    Ok(())
}
