#![cfg(test)]

mod catalog;
mod create;
mod lifecycle;
mod outcomes;
mod skips;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::TimeZone;
use chrono::Utc;
use kernel::tasks::ApprovalBroker;
use kernel::tasks::ApprovalDecision;
use kernel::tasks::ApprovalError;
use kernel::tasks::CatalogSource;
use kernel::tasks::ClaimSelector;
use kernel::tasks::ClaimedJob;
use kernel::tasks::Finish;
use kernel::tasks::TaskAgents;
use kernel::tasks::TaskError;
use kernel::tasks::TaskJobs;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskRecord;
use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use testkit::TaskMemory;
use uuid::Uuid;
use wire::gateway::GatewayError;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogCommand;
use wire::gateway::catalog::CatalogExecutorEntry;
use wire::gateway::catalog::CatalogNode;
use wire::gateway::catalog::CatalogSkillEntry;
use wire::harness::HarnessId;
use wire::json::JsonObject;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::ExecutorInteractionCapabilities;
use wire::task::ExecutorResultCapabilities;
use wire::task::ExecutorTurnCapabilities;
use wire::task::HarnessExecutorCapabilities;
use wire::task::McpInjection;
use wire::task::TaskApprovalRequest;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPromptMode;
use wire::task::TaskSpec;
use wire::task::TaskStatus;

use gateway::routes::tasks::COMMAND_LIST_TIMEOUT;
use gateway::routes::tasks::TasksApi;
use gateway::routes::tasks::router;

struct Setup {
    memory: TaskMemory,
    catalog: Arc<Catalog>,
    agents: Vec<String>,
    broker: Option<Arc<Broker>>,
    mesh: bool,
    allowed: Option<Vec<HarnessId>>,
    timeout: Duration,
}

impl Setup {
    fn new() -> Self {
        Self {
            memory: TaskMemory::new(),
            catalog: Arc::new(Catalog::steady()),
            agents: vec!["alpha".to_owned(), "beta".to_owned()],
            broker: None,
            mesh: false,
            allowed: None,
            timeout: COMMAND_LIST_TIMEOUT,
        }
    }

    fn router(&self) -> axum::Router {
        let memory = self.memory.clone();
        router(Arc::new(TasksApi {
            store: Arc::new(memory.clone()),
            jobs: Arc::new(memory.clone()),
            query: Arc::new(memory.clone()),
            watch: Arc::new(memory),
            catalog: self.catalog.clone(),
            agents: Arc::new(Agents {
                ids: self.agents.clone(),
            }),
            approvals: self
                .broker
                .clone()
                .map(|broker| broker as Arc<dyn ApprovalBroker>),
            node: "local".to_owned(),
            mesh: self.mesh,
            allowed_harnesses: self.allowed.clone(),
            command_timeout: self.timeout,
        }))
    }
}

#[derive(Debug)]
struct Agents {
    ids: Vec<String>,
}

#[async_trait]
impl TaskAgents for Agents {
    async fn contains(&self, agent_id: &str) -> Result<bool, TaskError> {
        Ok(self.ids.iter().any(|id| id == agent_id))
    }
}

#[derive(Debug)]
struct Catalog {
    agents: Vec<CatalogAgent>,
    executors: Vec<CatalogExecutorEntry>,
    tools: Vec<String>,
    skills: Vec<CatalogSkillEntry>,
    commands: Vec<CatalogCommand>,
    hang: AtomicBool,
    fail: AtomicBool,
    seen: Mutex<Vec<String>>,
}

impl Catalog {
    fn steady() -> Self {
        Self {
            agents: vec![CatalogAgent::Node(CatalogNode {
                id: "alpha".to_owned(),
                node: "local".to_owned(),
                local: true,
                provider: None,
                model: None,
            })],
            executors: vec![CatalogExecutorEntry {
                key: "chat-loop".to_owned(),
                capabilities: caps(),
                commands: vec![command("stale")],
                harness_id: None,
                implemented: Some(true),
            }],
            tools: vec!["bash".to_owned()],
            skills: vec![CatalogSkillEntry {
                name: "review".to_owned(),
                description: "look".to_owned(),
            }],
            commands: vec![command("real")],
            hang: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn empty() -> Self {
        Self {
            agents: Vec::new(),
            executors: Vec::new(),
            tools: Vec::new(),
            skills: Vec::new(),
            commands: Vec::new(),
            hang: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            seen: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl CatalogSource for Catalog {
    async fn agents(&self) -> Result<Vec<CatalogAgent>, TaskError> {
        Ok(self.agents.clone())
    }

    async fn executors(&self) -> Result<Vec<CatalogExecutorEntry>, TaskError> {
        Ok(self.executors.clone())
    }

    async fn tools(&self) -> Result<Vec<String>, TaskError> {
        Ok(self.tools.clone())
    }

    async fn skills(&self) -> Result<Vec<CatalogSkillEntry>, TaskError> {
        Ok(self.skills.clone())
    }

    async fn commands(&self, executor: &str) -> Result<Vec<CatalogCommand>, TaskError> {
        self.seen.lock().push(executor.to_owned());
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(TaskError::QueueAbsent);
        }
        if self.hang.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(self.commands.clone())
    }
}

#[derive(Debug, Default)]
struct Broker {
    pending: Mutex<Vec<TaskApprovalRequest>>,
    denied: Mutex<Vec<(Uuid, String)>>,
    resolved: Mutex<Vec<(String, ApprovalDecision)>>,
    deny_fails: AtomicBool,
}

#[async_trait]
impl ApprovalBroker for Broker {
    async fn pending(&self, task: Uuid) -> Result<Vec<TaskApprovalRequest>, ApprovalError> {
        let task_id = task.to_string();
        let pending = self.pending.lock().clone();
        Ok(pending
            .into_iter()
            .filter(|item| item.task_id == task_id)
            .collect())
    }

    async fn resolve(
        &self,
        task: Uuid,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), ApprovalError> {
        let task_id = task.to_string();
        let mut pending = self.pending.lock();
        let Some(index) = pending
            .iter()
            .position(|item| item.task_id == task_id && item.request_id == request_id)
        else {
            return Err(ApprovalError::Unknown);
        };
        pending.remove(index);
        drop(pending);
        self.resolved.lock().push((request_id.to_owned(), decision));
        Ok(())
    }

    async fn deny_pending(&self, task: Uuid, reason: &str) -> Result<(), ApprovalError> {
        if self.deny_fails.load(Ordering::SeqCst) {
            return Err(ApprovalError::Unknown);
        }
        self.denied.lock().push((task, reason.to_owned()));
        Ok(())
    }
}

fn caps() -> HarnessExecutorCapabilities {
    HarnessExecutorCapabilities {
        turn: ExecutorTurnCapabilities {
            steerable: true,
            multi_turn: true,
            structured_stream: false,
        },
        result: ExecutorResultCapabilities {
            usage_in_result: true,
            session_id_capture: false,
        },
        interaction: ExecutorInteractionCapabilities {
            slash_commands: false,
            effort_selection: false,
        },
        mcp_injection: McpInjection::None,
    }
}

fn command(name: &str) -> CatalogCommand {
    CatalogCommand {
        name: name.to_owned(),
        description: "d".to_owned(),
        arg_hint: None,
        source: "disk".to_owned(),
    }
}

fn approval(task: Uuid, request_id: &str) -> TaskApprovalRequest {
    TaskApprovalRequest {
        task_id: task.to_string(),
        request_id: request_id.to_owned(),
        name: "bash".to_owned(),
        input: JsonObject::default(),
        tool_call_id: Some("call-1".to_owned()),
    }
}

fn record(id: Uuid, agent: &str, at: DateTime<Utc>) -> TaskRecord {
    TaskRecord {
        id,
        goal: "goal".to_owned(),
        context_refs: Vec::new(),
        acceptance_criteria: Vec::new(),
        spec: TaskSpec {
            task_id: id.to_string(),
            agent_id: agent.to_owned(),
            goal: "goal".to_owned(),
            resolved_context: Vec::new(),
            acceptance_criteria: Vec::new(),
            budget: wire::task::TaskBudget::default(),
            tools: None,
            working_dir: None,
            model: None,
            effort: None,
            system_prompt_append: None,
            isolation: None,
            prompt_mode: Some(TaskPromptMode::Task),
            exclude_tools: None,
            resume_message: None,
            session: format!("task:{id}"),
            permission_decisions: None,
        },
        executor: TaskExecutorKind::ChatLoop,
        executor_target: None,
        requested_by: Some("gateway".to_owned()),
        agent_id: agent.to_owned(),
        origin: TaskOrigin::Api,
        parent_task_id: None,
        chain_depth: 0,
        budget: wire::task::TaskBudget::default(),
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
        created_at: at,
        started_at: None,
        last_heartbeat_at: None,
        completed_at: None,
        duration_ms: None,
        eval: None,
        eval_attempt: 0,
    }
}

fn finished(status: TaskStatus) -> Finish {
    Finish {
        status,
        result: None,
        error: None,
        usage: None,
        completed_at: Utc::now(),
        duration_ms: None,
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    target: &str,
    body: Option<&str>,
) -> (u16, Vec<u8>) {
    let mut builder = axum::http::Request::builder().method(method).uri(target);
    let payload = match body {
        Some(text) => {
            builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
            text.to_owned()
        }
        None => String::new(),
    };
    let request = builder
        .body(axum::body::Body::from(payload))
        .expect("request");
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        tower::ServiceExt::oneshot(app.clone(), request),
    )
    .await
    .expect("deadline")
    .expect("response");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .expect("body")
        .to_vec();
    (status, bytes)
}

fn json<T: DeserializeOwned>(bytes: &[u8]) -> T {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(bytes)))
}

fn message(bytes: &[u8]) -> String {
    json::<GatewayError>(bytes).error
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskPost<'a> {
    goal: &'a str,
    agent_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    executor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    executor_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requested_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chain_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_affinity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_refs: Option<Vec<ContextRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    acceptance_criteria: Option<Vec<AcceptanceCriterion>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget: Option<TaskBudget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spec: Option<TaskSpec>,
}

fn task_post(goal: &str) -> TaskPost<'_> {
    TaskPost {
        goal,
        agent_id: "alpha",
        executor: None,
        executor_target: None,
        requested_by: None,
        parent_task_id: None,
        chain_depth: None,
        node_affinity: None,
        context_refs: None,
        acceptance_criteria: None,
        budget: None,
        spec: None,
    }
}

fn render(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("json")
}

fn goal(text: &str) -> String {
    render(&task_post(text))
}

fn mode_of(mode: &str) -> TaskPromptMode {
    match mode {
        "heartbeat" => TaskPromptMode::Heartbeat,
        "task" => TaskPromptMode::Task,
        other => panic!("prompt mode {other}"),
    }
}

fn spec_for(
    agent_id: &str,
    session: &str,
    mode: TaskPromptMode,
    working_dir: Option<&str>,
    resolved_context: Vec<ContextRef>,
    acceptance_criteria: Vec<AcceptanceCriterion>,
) -> TaskSpec {
    TaskSpec {
        task_id: "old".to_owned(),
        agent_id: agent_id.to_owned(),
        goal: "spec-goal".to_owned(),
        resolved_context,
        acceptance_criteria,
        budget: TaskBudget {
            max_turns: Some(9),
            ..TaskBudget::default()
        },
        tools: None,
        working_dir: working_dir.map(str::to_owned),
        model: None,
        effort: None,
        system_prompt_append: None,
        isolation: None,
        prompt_mode: Some(mode),
        exclude_tools: None,
        resume_message: None,
        session: session.to_owned(),
        permission_decisions: None,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IgnoredDelegation<'a> {
    from: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefaultsPost<'a> {
    goal: &'a str,
    agent_id: &'a str,
    max_attempts: u32,
    preset_id: &'a str,
    preset_name: &'a str,
    shared_link: &'a str,
    delegation: IgnoredDelegation<'a>,
    mesh_from: &'a str,
}

fn ignored_fields_body() -> String {
    render(&DefaultsPost {
        goal: "ship",
        agent_id: "alpha",
        max_attempts: 9,
        preset_id: "preset",
        preset_name: "named",
        shared_link: "https://example.invalid/s",
        delegation: IgnoredDelegation { from: "other" },
        mesh_from: "east",
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NullRequester<'a> {
    goal: &'a str,
    agent_id: &'a str,
    requested_by: (),
}

fn null_requested_by() -> String {
    render(&NullRequester {
        goal: "ship",
        agent_id: "alpha",
        requested_by: (),
    })
}

async fn only_id(memory: &TaskMemory) -> Uuid {
    let tasks = memory.list(&TaskListFilter::default()).await.expect("list");
    tasks.first().expect("one task").id
}

async fn claim(memory: &TaskMemory, name: &str) -> Option<ClaimedJob> {
    memory
        .claim(&ClaimSelector {
            names: vec![name.to_owned()],
            only_id: None,
        })
        .await
        .expect("claim")
}

fn stamp(day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, 0, 0, 0)
        .single()
        .expect("stamp")
}
