use std::path::Path;
use std::path::PathBuf;

use chrono::Utc;
use kernel::memory::MemoryStore;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use kernel::wiki::builder::ExtractWiki;
use kernel::wiki::builder::WikiAction;
use kernel::wiki::builder::WikiBuilderStore;
use kernel::wiki::builder::WikiPatch;
use kernel::wiki::builder::WikiRouting;
use memory_sqlite::SqliteStore;
use uuid::Uuid;
use wire::gateway::wiki::WikiSlug;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPromptMode;
use wire::task::TaskSpec;
use wire::task::TaskStatus;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: SqliteStore,
}

fn fixture() -> TestResult<Fixture> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("memory.sqlite");
    migrate::sqlite(&path)?;
    let store = SqliteStore::open(&path)?;
    Ok(Fixture {
        _dir: dir,
        path,
        store,
    })
}

fn summary_id() -> TestResult<Uuid> {
    Ok(Uuid::parse_str("0199e2a0-7b3c-7a00-8000-000000000001")?)
}

fn seed_summary(path: &Path, id: Uuid) -> TestResult {
    let conn = rusqlite::Connection::open(path)?;
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    conn.execute(
        "INSERT INTO ros_summaries (id, content, created_at) VALUES (?1, ?2, ?3)",
        (id.to_string(), "AcmeKit deploys on Fridays.", now),
    )?;
    Ok(())
}

fn patch() -> TestResult<WikiPatch> {
    Ok(WikiPatch {
        action: WikiAction::Create,
        slug: WikiSlug::try_from("acmekit-deploys".to_owned())?,
        title: "AcmeKit deploys".to_owned(),
        aliases: Vec::new(),
        tags: Vec::new(),
        entities: Vec::new(),
        summary: "AcmeKit deploys on Fridays.".to_owned(),
        article: None,
        related: None,
        history_entry: None,
        entity_type: None,
    })
}

fn job(id: Uuid) -> ExtractWiki {
    ExtractWiki {
        summary_id: id,
        conversation_id: None,
    }
}

fn cell(path: &Path, sql: &str, id: &str) -> TestResult<String> {
    let conn = rusqlite::Connection::open(path)?;
    let value = conn.query_row(sql, [id], |row| row.get(0))?;
    Ok(value)
}

#[tokio::test]
async fn apply_wiki_stores_the_topic_and_a_repeat_returns_it() -> TestResult {
    let fixture = fixture()?;
    let id = summary_id()?;
    seed_summary(&fixture.path, id)?;
    let created = fixture
        .store
        .apply_wiki(
            &job(id),
            vec![patch()?],
            &WikiRouting::default(),
            Utc::now(),
        )
        .await?;
    assert_eq!(created, vec!["acmekit-deploys".to_owned()]);
    let title = cell(
        &fixture.path,
        "SELECT title FROM ros_wiki_topics WHERE slug = ?1",
        "acmekit-deploys",
    )?;
    assert_eq!(title, "AcmeKit deploys");
    let again = fixture
        .store
        .apply_wiki(
            &job(id),
            vec![patch()?],
            &WikiRouting::default(),
            Utc::now(),
        )
        .await?;
    assert_eq!(again, created);
    Ok(())
}

#[tokio::test]
async fn fail_wiki_does_not_clobber_a_done_extraction() -> TestResult {
    let fixture = fixture()?;
    let id = summary_id()?;
    seed_summary(&fixture.path, id)?;
    fixture
        .store
        .apply_wiki(
            &job(id),
            vec![patch()?],
            &WikiRouting::default(),
            Utc::now(),
        )
        .await?;
    fixture
        .store
        .fail_wiki(id, "later failure", Utc::now())
        .await?;
    let status = cell(
        &fixture.path,
        "SELECT status FROM ros_wiki_extractions WHERE summary_id = ?1",
        &id.to_string(),
    )?;
    assert_eq!(status, "done");
    Ok(())
}

fn queued(id: Uuid, pending: Option<&str>) -> TaskRecord {
    let session = format!("task:{id}");
    TaskRecord {
        id,
        goal: "finish the ledger".to_owned(),
        context_refs: Vec::new(),
        acceptance_criteria: Vec::new(),
        spec: TaskSpec {
            delegation: None,
            mesh_from: None,
            task_id: id.to_string(),
            agent_id: "main".to_owned(),
            goal: "finish the ledger".to_owned(),
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
            preset_id: None,
            preset_name: None,
            audit_only: None,
            extra: wire::Extra::default(),
        },
        executor: TaskExecutorKind::ChatLoop,
        executor_target: None,
        requested_by: None,
        agent_id: "main".to_owned(),
        origin: TaskOrigin::Tool,
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
        pending_message: pending.map(str::to_owned),
        claimed_by: None,
        harness_session_ids: Vec::new(),
        created_at: Utc::now(),
        started_at: None,
        last_heartbeat_at: None,
        completed_at: None,
        duration_ms: None,
        eval: None,
        eval_attempt: 0,
    }
}

#[tokio::test]
async fn mark_running_returns_the_previous_pending_message() -> TestResult {
    let fixture = fixture()?;
    let id = Uuid::now_v7();
    fixture.store.insert(&queued(id, Some("steer"))).await?;
    let running = fixture
        .store
        .mark_running(id, "node:agent", "node", Utc::now())
        .await?;
    assert_eq!(running.pending_message.as_deref(), Some("steer"));
    assert_eq!(running.status, TaskStatus::Running);
    let stored = cell(
        &fixture.path,
        "SELECT ifnull(pending_message, '') FROM rh_tasks WHERE id = ?1",
        &id.to_string(),
    )?;
    assert_eq!(stored, "");
    Ok(())
}

#[tokio::test]
async fn sweep_on_an_empty_task_table_succeeds() -> TestResult {
    let fixture = fixture()?;
    fixture.store.sweep("node:agent", Utc::now()).await?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct CompletedColumns {
    usage: wire::task::TaskUsage,
    result: wire::task::TaskResult,
}

#[tokio::test]
async fn task_json_writes_omit_absent_fields() -> TestResult {
    let fixture = fixture()?;
    let id = Uuid::now_v7();
    let mut record = queued(id, None);
    let completed_text =
        include_str!("../../../testkit/tests/fixtures/tasks/completed-columns.json");
    let completed: CompletedColumns = serde_json::from_str(completed_text)?;
    let eval_text = include_str!("../../../testkit/tests/fixtures/tasks/eval-column.json");
    record.eval = Some(serde_json::from_str(eval_text)?);
    record.usage = Some(completed.usage);
    record.result = Some(completed.result);
    fixture.store.insert(&record).await?;
    let stored = cell(
        &fixture.path,
        "SELECT json_object('usage', json(usage), 'result', json(result)) FROM rh_tasks WHERE id = ?1",
        &id.to_string(),
    )?;
    assert_eq!(
        serde_json::from_str::<wire::JsonObject>(&stored)?,
        serde_json::from_str::<wire::JsonObject>(completed_text)?
    );
    let stored = cell(
        &fixture.path,
        "SELECT eval FROM rh_tasks WHERE id = ?1",
        &id.to_string(),
    )?;
    assert_eq!(
        serde_json::from_str::<wire::JsonObject>(&stored)?,
        serde_json::from_str::<wire::JsonObject>(eval_text)?
    );
    let stored = cell(
        &fixture.path,
        "SELECT budget FROM rh_tasks WHERE id = ?1",
        &id.to_string(),
    )?;
    assert_eq!(
        serde_json::from_str::<wire::JsonObject>(&stored)?,
        wire::JsonObject::default()
    );
    fixture.store.close().await?;
    Ok(())
}
