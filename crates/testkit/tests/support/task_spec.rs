#![cfg(test)]

use std::collections::BTreeSet;

use chrono::DateTime;
use chrono::Utc;
use kernel::tasks::TaskError;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use serde::Deserialize;
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

const SKIPS: [&str; 4] = [
    "full-wire-spec-column",
    "http-create-bag-spec",
    "null-versus-absent",
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
pub(super) struct StoredRow {
    pub id: Uuid,
    pub goal: String,
    pub context_refs: Vec<ContextRef>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub spec: JsonObject,
    pub executor: TaskExecutorKind,
    pub executor_target: Option<String>,
    pub agent_id: String,
    pub requested_by: Option<String>,
    pub origin: TaskOrigin,
    pub parent_task_id: Option<Uuid>,
    pub chain_depth: i32,
    pub node_affinity: Option<String>,
    pub claimed_by: Option<String>,
    pub budget: TaskBudget,
    pub usage: Option<TaskUsage>,
    pub status: TaskStatus,
    pub attempt: i32,
    pub max_attempts: i32,
    pub pending_message: Option<String>,
    pub error: Option<String>,
    pub result: Option<TaskResult>,
    pub conversation_id: Option<Uuid>,
    pub session_key: Option<String>,
    pub harness_session_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i32>,
    pub eval: Option<VerifierResult>,
    pub eval_attempt: i32,
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
    overrides
        .permission_decisions
        .get_or_insert_with(Vec::new)
        .extend_from_slice(extra);
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

async fn round_trip(store: &dyn TaskStore, raw: &dyn RawStore, row: &StoredRow) {
    let loaded = store.load(row.id).await.expect("load");
    assert_eq!(loaded.goal, row.goal);
    assert_eq!(loaded.agent_id, row.agent_id);
    assert_eq!(loaded.status, row.status);
    assert_eq!(loaded.origin, row.origin);
    assert_eq!(loaded.executor, row.executor);
    assert_eq!(loaded.error, row.error);
    assert_eq!(loaded.duration_ms, row.duration_ms);
    assert_eq!(loaded.session_key, row.session_key);
    assert_assembled(&loaded, row);
    let mut copy = loaded.clone();
    copy.id = Uuid::now_v7();
    store.insert(&copy).await.expect("rewrite");
    let stored = store.load(copy.id).await.expect("reloaded");
    assert_eq!(stored.session_key, row.session_key);
    assert_eq!(stored.spec.session, loaded.spec.session);
    assert_eq!(stored.spec.task_id, copy.id.to_string());
    assert_eq!(bag(&stored.spec), row.spec);
    assert_eq!(raw.raw_spec(copy.id).await, row.spec);
}

async fn expect_load_error(store: &dyn TaskStore, id: Uuid) {
    let error = store.load(id).await.expect_err("load");
    assert!(matches!(
        error,
        TaskError::Stored(_)
            | TaskError::Operation {
                operation: "load task",
                ..
            }
    ));
}

async fn expect_missing_session(store: &dyn TaskStore, id: Uuid) {
    let error = store.load(id).await.expect_err("load");
    assert!(
        matches!(error, TaskError::Stored(ref message) if message == "session key is absent"),
        "{error}"
    );
}

async fn append_two(store: &dyn TaskStore, row: &StoredRow) {
    let loaded = store.load(row.id).await.expect("load");
    assert_assembled(&loaded, row);
    let first = decision("req-a", TaskPermissionOutcome::Allow);
    let second = decision("req-b", TaskPermissionOutcome::Deny);
    store
        .append_permission(row.id, &first)
        .await
        .expect("first");
    store
        .append_permission(row.id, &second)
        .await
        .expect("second");
    let stored = store.load(row.id).await.expect("reloaded");
    let decisions = stored.spec.permission_decisions.clone().expect("decisions");
    assert_eq!(decisions, vec![first, second]);
    assert_eq!(bag(&stored.spec), with_decisions(&loaded.spec, &decisions));
}

async fn append_existing(store: &dyn TaskStore, row: &StoredRow) {
    let loaded = store.load(row.id).await.expect("load");
    assert_assembled(&loaded, row);
    let added = decision("req-added", TaskPermissionOutcome::Deny);
    store
        .append_permission(row.id, &added)
        .await
        .expect("append");
    let stored = store.load(row.id).await.expect("reloaded");
    let decisions = stored.spec.permission_decisions.clone().expect("decisions");
    let existing = decisions.first().expect("existing");
    let appended = decisions.get(1).expect("appended");
    assert_eq!(existing.request_id, "req-existing");
    assert_eq!(appended.request_id, "req-added");
    assert_eq!(bag(&stored.spec), with_decisions(&loaded.spec, &[added]));
}

async fn reject_append(store: &dyn TaskStore, raw: &dyn RawStore, row: &StoredRow) {
    expect_load_error(store, row.id).await;
    let error = store
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
    assert_eq!(raw.raw_spec(row.id).await, row.spec);
}

async fn task_rows_replay_the_override_bag(store: &dyn TaskStore, raw: &dyn RawStore) {
    let fixture = load_fixture();
    assert_skips(&fixture);
    assert!(fixture.rows.iter().any(|scenario| {
        scenario.name == "production-exclude-tools" && scenario.kind == Kind::RoundTrip
    }));
    let mut kinds = BTreeSet::new();
    for scenario in &fixture.rows {
        raw.insert_row(&scenario.row).await;
        kinds.insert(scenario.kind);
        match scenario.kind {
            Kind::RoundTrip => round_trip(store, raw, &scenario.row).await,
            Kind::RejectLoad => expect_load_error(store, scenario.row.id).await,
            Kind::RejectSession => expect_missing_session(store, scenario.row.id).await,
            Kind::AppendTwo => append_two(store, &scenario.row).await,
            Kind::AppendExisting => append_existing(store, &scenario.row).await,
            Kind::RejectAppend => reject_append(store, raw, &scenario.row).await,
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

async fn a_runtime_insert_stores_only_override_keys(store: &dyn TaskStore, raw: &dyn RawStore) {
    let id = Uuid::now_v7();
    store.insert(&runtime_record(id)).await.expect("insert");
    let stored = raw.raw_spec(id).await;
    let expected = JsonObject::from_serialize(&TaskOverrides {
        prompt_mode: Some(TaskPromptMode::Task),
        ..TaskOverrides::default()
    })
    .expect("expected bag");
    assert_eq!(stored, expected);
    let loaded = store.load(id).await.expect("load");
    assert_eq!(loaded.spec.task_id, id.to_string());
    assert_eq!(loaded.spec.session, format!("task:{id}"));
    assert_eq!(loaded.spec.prompt_mode, Some(TaskPromptMode::Task));
    assert_eq!(bag(&loaded.spec), stored);
}

async fn tools_on_the_runtime_spec_are_refused(store: &dyn TaskStore) {
    let mut record = runtime_record(Uuid::now_v7());
    record.spec.tools = Some(vec!["shell".to_owned()]);
    let error = store.insert(&record).await.expect_err("tools");
    assert!(
        matches!(
            error,
            TaskError::Stored(ref message) if message == "tools is not a column override"
        ),
        "{error}"
    );
}

#[async_trait::async_trait]
pub(super) trait RawStore: Sync {
    async fn insert_row(&self, row: &StoredRow);
    async fn raw_spec(&self, id: Uuid) -> JsonObject;
}

pub(super) async fn run(store: &dyn TaskStore, peer: &dyn TaskStore, raw: &dyn RawStore) {
    task_rows_replay_the_override_bag(store, raw).await;
    a_runtime_insert_stores_only_override_keys(store, raw).await;
    tools_on_the_runtime_spec_are_refused(store).await;
    concurrent_permissions(store, peer, raw).await;
    null_decisions_refuse_append(store, raw).await;
    let id = Uuid::now_v7();
    assert!(matches!(
        store.append_permission(id, &decision("missing", TaskPermissionOutcome::Deny)).await,
        Err(TaskError::Missing { id: missing }) if missing == id
    ));
}

async fn concurrent_permissions(store: &dyn TaskStore, peer: &dyn TaskStore, raw: &dyn RawStore) {
    let id = Uuid::now_v7();
    let mut task = runtime_record(id);
    task.spec.extra =
        serde_json::from_str(r#"{"future":{"keep":[1,true,"value"]}}"#).expect("extra");
    store.insert(&task).await.expect("insert concurrent task");
    let first = decision("parallel-a", TaskPermissionOutcome::Allow);
    let second = decision("parallel-b", TaskPermissionOutcome::Deny);
    let (left, right) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            store.append_permission(id, &first),
            peer.append_permission(id, &second)
        )
    })
    .await
    .expect("appends bounded");
    left.expect("left append");
    right.expect("right append");
    let loaded = store.load(id).await.expect("load concurrent task");
    let decisions = loaded
        .spec
        .permission_decisions
        .as_ref()
        .expect("decisions");
    assert_eq!(decisions.len(), 2);
    assert!(decisions.contains(&first));
    assert!(decisions.contains(&second));
    assert_eq!(
        raw.raw_spec(id).await,
        with_decisions(&task.spec, decisions)
    );
}

async fn null_decisions_refuse_append(store: &dyn TaskStore, raw: &dyn RawStore) {
    let mut row = load_fixture().rows.remove(0).row;
    row.id = Uuid::now_v7();
    row.spec =
        serde_json::from_str(r#"{"permissionDecisions":null,"future":"keep"}"#).expect("bag");
    raw.insert_row(&row).await;
    let error = store
        .append_permission(row.id, &decision("null", TaskPermissionOutcome::Deny))
        .await
        .expect_err("null array");
    assert!(
        matches!(error, TaskError::Stored(ref message) if message.contains("permissionDecisions"))
    );
    assert_eq!(raw.raw_spec(row.id).await, row.spec);
}
