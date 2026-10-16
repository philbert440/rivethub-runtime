#![cfg(test)]

use std::collections::BTreeSet;

use chrono::DateTime;
use chrono::Utc;
use kernel::tasks::TaskError;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use memory_postgres::PostgresTasks;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;
use wire::JsonObject;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPermissionDecision;
use wire::task::TaskPermissionOutcome;
use wire::task::TaskPromptMode;
use wire::task::TaskResult;
use wire::task::TaskSpec;
use wire::task::TaskStatus;
use wire::task::TaskUsage;
use wire::task::VerifierResult;
use wire::task_overrides::TaskOverrides;

const SKIPS: [&str; 6] = [
    "foreign-slice-recordings",
    "full-wire-spec-column",
    "http-create-bag-spec",
    "null-versus-absent",
    "session-api-tool-filter",
    "tools-allow-list",
];

#[derive(Debug, Deserialize)]
struct Fixture {
    skipped: Vec<Skip>,
    rows: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
struct Skip {
    name: String,
    reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    RoundTrip,
    RejectLoad,
    RejectSession,
    AppendTwo,
    AppendExisting,
    RejectAppend,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    kind: Kind,
    row: StoredRow,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRow {
    id: Uuid,
    goal: String,
    context_refs: Vec<ContextRef>,
    acceptance_criteria: Vec<AcceptanceCriterion>,
    spec: JsonObject,
    executor: String,
    executor_target: Option<String>,
    agent_id: String,
    requested_by: Option<String>,
    origin: String,
    parent_task_id: Option<Uuid>,
    chain_depth: i32,
    node_affinity: Option<String>,
    claimed_by: Option<String>,
    budget: TaskBudget,
    usage: Option<TaskUsage>,
    status: String,
    attempt: i32,
    max_attempts: i32,
    pending_message: Option<String>,
    error: Option<String>,
    result: Option<TaskResult>,
    conversation_id: Option<Uuid>,
    session_key: Option<String>,
    harness_session_ids: Vec<String>,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    last_heartbeat_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    duration_ms: Option<i32>,
    eval: Option<VerifierResult>,
    eval_attempt: i32,
}

fn load_fixture() -> Fixture {
    let text = std::fs::read_to_string(testkit::recording("tasks/ts-rows.json")).expect("fixture");
    serde_json::from_str(&text).expect("fixture json")
}

fn assert_skips(fixture: &Fixture) {
    let mut names = BTreeSet::new();
    for skip in &fixture.skipped {
        assert!(!skip.reason.trim().is_empty(), "{}", skip.name);
        assert!(names.insert(skip.name.clone()), "duplicate {}", skip.name);
    }
    let expected: BTreeSet<String> = SKIPS.into_iter().map(str::to_owned).collect();
    assert_eq!(names, expected);
}

fn bag(spec: &TaskSpec) -> JsonObject {
    JsonObject::from_serialize(&TaskOverrides::from_spec(spec)).expect("bag")
}

fn with_decisions(spec: &TaskSpec, extra: &[TaskPermissionDecision]) -> JsonObject {
    let mut overrides = TaskOverrides::from_spec(spec);
    let mut decisions = overrides.permission_decisions.take().unwrap_or_default();
    decisions.extend_from_slice(extra);
    overrides.permission_decisions = Some(decisions);
    JsonObject::from_serialize(&overrides).expect("decisions")
}

fn decision(id: &str, outcome: TaskPermissionOutcome) -> TaskPermissionDecision {
    TaskPermissionDecision {
        request_id: id.to_owned(),
        tool: "bash".to_owned(),
        decision: outcome,
        at: DateTime::<Utc>::from_timestamp_millis(1_760_576_600_463).expect("stamp"),
        message: None,
    }
}

fn json_text<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("json")
}

const INSERT_ROW: &str = concat!(
    "INSERT INTO rh_tasks (",
    "id, goal, context_refs, acceptance_criteria, spec, executor, executor_target, ",
    "requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity, ",
    "usage, status, attempt, max_attempts, result, error, conversation_id, session_key, ",
    "pending_message, claimed_by, harness_session_ids, created_at, started_at, ",
    "last_heartbeat_at, completed_at, duration_ms, eval, eval_attempt",
    ") VALUES (",
    "$1, $2, $3::jsonb, $4::jsonb, $5::jsonb, $6, $7, ",
    "$8, $9, $10, $11, $12, $13::jsonb, $14, ",
    "$15::jsonb, $16, $17, $18, $19::jsonb, $20, $21, $22, ",
    "$23, $24, $25::jsonb, $26, $27, ",
    "$28, $29, $30, $31::jsonb, $32)"
);

async fn insert_row(pool: &PgPool, row: &StoredRow) {
    sqlx::query(INSERT_ROW)
        .bind(row.id)
        .bind(&row.goal)
        .bind(json_text(&row.context_refs))
        .bind(json_text(&row.acceptance_criteria))
        .bind(json_text(&row.spec))
        .bind(&row.executor)
        .bind(&row.executor_target)
        .bind(&row.requested_by)
        .bind(&row.agent_id)
        .bind(&row.origin)
        .bind(row.parent_task_id)
        .bind(row.chain_depth)
        .bind(json_text(&row.budget))
        .bind(&row.node_affinity)
        .bind(row.usage.as_ref().map(json_text))
        .bind(&row.status)
        .bind(row.attempt)
        .bind(row.max_attempts)
        .bind(row.result.as_ref().map(json_text))
        .bind(&row.error)
        .bind(row.conversation_id)
        .bind(&row.session_key)
        .bind(&row.pending_message)
        .bind(&row.claimed_by)
        .bind(json_text(&row.harness_session_ids))
        .bind(row.created_at)
        .bind(row.started_at)
        .bind(row.last_heartbeat_at)
        .bind(row.completed_at)
        .bind(row.duration_ms)
        .bind(row.eval.as_ref().map(json_text))
        .bind(row.eval_attempt)
        .execute(pool)
        .await
        .expect("insert row");
}

fn assert_assembled(loaded: &TaskRecord, row: &StoredRow) {
    let session = row.session_key.clone().expect("session");
    assert_eq!(loaded.spec.task_id, row.id.to_string());
    assert_eq!(loaded.spec.agent_id, row.agent_id);
    assert_eq!(loaded.spec.goal, row.goal);
    assert_eq!(loaded.spec.resolved_context, row.context_refs);
    assert_eq!(loaded.spec.acceptance_criteria, row.acceptance_criteria);
    assert_eq!(loaded.spec.budget, row.budget);
    assert_eq!(loaded.spec.session, session);
    assert_eq!(loaded.spec.tools, None);
    assert_eq!(bag(&loaded.spec), row.spec);
}

async fn round_trip(postgres: &PostgresTasks, row: &StoredRow) {
    let loaded = postgres.load(row.id).await.expect("load");
    assert_eq!(loaded.goal, row.goal);
    assert_eq!(loaded.agent_id, row.agent_id);
    assert_eq!(loaded.status.to_string(), row.status);
    assert_eq!(loaded.origin.to_string(), row.origin);
    assert_eq!(loaded.executor.to_string(), row.executor);
    assert_eq!(loaded.error, row.error);
    assert_eq!(loaded.duration_ms, row.duration_ms);
    assert_eq!(loaded.session_key, row.session_key);
    assert_assembled(&loaded, row);
    let mut copy = loaded.clone();
    copy.id = Uuid::now_v7();
    postgres.insert(&copy).await.expect("rewrite");
    let stored = postgres.load(copy.id).await.expect("reloaded");
    assert_eq!(stored.session_key, row.session_key);
    assert_eq!(stored.spec.session, loaded.spec.session);
    assert_eq!(stored.spec.task_id, copy.id.to_string());
    assert_eq!(bag(&stored.spec), row.spec);
}

async fn expect_load_error(postgres: &PostgresTasks, id: Uuid) {
    let error = postgres.load(id).await.expect_err("load");
    assert!(matches!(
        error,
        TaskError::Operation {
            operation: "load task",
            ..
        }
    ));
}

async fn expect_missing_session(postgres: &PostgresTasks, id: Uuid) {
    let error = postgres.load(id).await.expect_err("load");
    assert!(
        matches!(error, TaskError::Stored(ref message) if message == "session key is absent"),
        "{error}"
    );
}

async fn raw_spec(pool: &PgPool, id: Uuid) -> JsonObject {
    let spec: sqlx::types::Json<JsonObject> =
        sqlx::query_scalar("SELECT spec FROM rh_tasks WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("spec");
    spec.0
}

async fn append_two(postgres: &PostgresTasks, row: &StoredRow) {
    let loaded = postgres.load(row.id).await.expect("load");
    assert_assembled(&loaded, row);
    let first = decision("req-a", TaskPermissionOutcome::Allow);
    let second = decision("req-b", TaskPermissionOutcome::Deny);
    postgres
        .append_permission(row.id, &first)
        .await
        .expect("first");
    postgres
        .append_permission(row.id, &second)
        .await
        .expect("second");
    let stored = postgres.load(row.id).await.expect("reloaded");
    let decisions = stored.spec.permission_decisions.clone().expect("decisions");
    assert_eq!(decisions, vec![first, second]);
    assert_eq!(bag(&stored.spec), with_decisions(&loaded.spec, &decisions));
}

async fn append_existing(postgres: &PostgresTasks, row: &StoredRow) {
    let loaded = postgres.load(row.id).await.expect("load");
    assert_assembled(&loaded, row);
    let added = decision("req-added", TaskPermissionOutcome::Deny);
    postgres
        .append_permission(row.id, &added)
        .await
        .expect("append");
    let stored = postgres.load(row.id).await.expect("reloaded");
    let decisions = stored.spec.permission_decisions.clone().expect("decisions");
    let existing = decisions.first().expect("existing");
    let appended = decisions.get(1).expect("appended");
    assert_eq!(existing.request_id, "req-existing");
    assert_eq!(appended.request_id, "req-added");
    assert_eq!(bag(&stored.spec), with_decisions(&loaded.spec, &[added]));
}

async fn reject_append(postgres: &PostgresTasks, pool: &PgPool, row: &StoredRow) {
    expect_load_error(postgres, row.id).await;
    let error = postgres
        .append_permission(row.id, &decision("req-added", TaskPermissionOutcome::Deny))
        .await
        .expect_err("append");
    assert!(
        matches!(
            error,
            TaskError::Stored(ref message) if message.contains("permissionDecisions")
        ),
        "{error}"
    );
    assert_eq!(raw_spec(pool, row.id).await, row.spec);
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn task_rows_replay_the_override_bag(pool: PgPool) {
    let fixture = load_fixture();
    assert_skips(&fixture);
    assert!(fixture.rows.iter().any(|scenario| {
        scenario.name == "production-exclude-tools" && scenario.kind == Kind::RoundTrip
    }));
    let postgres = PostgresTasks::from_pool(pool.clone());
    let mut kinds = BTreeSet::new();
    for scenario in &fixture.rows {
        insert_row(&pool, &scenario.row).await;
        kinds.insert(scenario.kind);
        match scenario.kind {
            Kind::RoundTrip => round_trip(&postgres, &scenario.row).await,
            Kind::RejectLoad => expect_load_error(&postgres, scenario.row.id).await,
            Kind::RejectSession => expect_missing_session(&postgres, scenario.row.id).await,
            Kind::AppendTwo => append_two(&postgres, &scenario.row).await,
            Kind::AppendExisting => append_existing(&postgres, &scenario.row).await,
            Kind::RejectAppend => reject_append(&postgres, &pool, &scenario.row).await,
        }
    }
    assert_eq!(
        kinds,
        BTreeSet::from([
            Kind::RoundTrip,
            Kind::RejectLoad,
            Kind::RejectSession,
            Kind::AppendTwo,
            Kind::AppendExisting,
            Kind::RejectAppend,
        ])
    );
}

fn runtime_record(id: Uuid) -> TaskRecord {
    let session = format!("task:{id}");
    TaskRecord {
        id,
        goal: "ship it".to_owned(),
        context_refs: Vec::new(),
        acceptance_criteria: Vec::new(),
        spec: TaskSpec {
            task_id: id.to_string(),
            agent_id: "worker".to_owned(),
            goal: "ship it".to_owned(),
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
        agent_id: "worker".to_owned(),
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
        created_at: Utc::now(),
        started_at: None,
        last_heartbeat_at: None,
        completed_at: None,
        duration_ms: None,
        eval: None,
        eval_attempt: 0,
    }
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_runtime_insert_stores_only_override_keys(pool: PgPool) {
    let postgres = PostgresTasks::from_pool(pool.clone());
    let id = Uuid::now_v7();
    postgres.insert(&runtime_record(id)).await.expect("insert");
    let stored = raw_spec(&pool, id).await;
    let expected = JsonObject::from_serialize(&TaskOverrides {
        prompt_mode: Some(TaskPromptMode::Task),
        ..TaskOverrides::default()
    })
    .expect("expected bag");
    assert_eq!(stored, expected);
    let loaded = postgres.load(id).await.expect("load");
    assert_eq!(loaded.spec.task_id, id.to_string());
    assert_eq!(loaded.spec.session, format!("task:{id}"));
    assert_eq!(loaded.spec.prompt_mode, Some(TaskPromptMode::Task));
    assert_eq!(bag(&loaded.spec), stored);
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn tools_on_the_runtime_spec_are_refused(pool: PgPool) {
    let postgres = PostgresTasks::from_pool(pool);
    let mut record = runtime_record(Uuid::now_v7());
    record.spec.tools = Some(vec!["shell".to_owned()]);
    let error = postgres.insert(&record).await.expect_err("tools");
    assert!(
        matches!(
            error,
            TaskError::Stored(ref message) if message == "tools is not a column override"
        ),
        "{error}"
    );
}
