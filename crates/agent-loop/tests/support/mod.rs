use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_loop::audit::AuditLog;
use agent_loop::fence::WorkspaceFence;
use agent_loop::hooks::{HookError, HookPipeline};
use agent_loop::shell::ShellDanger;
use agent_loop::{Turn, TurnLimits};
use kernel::ToolContext;
use kernel::model::ModelRequest;
use tempfile::TempDir;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use wire::JsonObject;
use wire::hooks::{HookContextBase, HookPipelineResult};
use wire::message::{Message, MessageContent, Role};

#[derive(Debug)]
pub struct Harness {
    pub dir: TempDir,
    pub hooks: Arc<HookPipeline>,
    pub cancel: CancellationToken,
    pub tasks: JoinSet<Result<(), HookError>>,
}

impl Harness {
    pub async fn new() -> Self {
        let dir = scratch();
        let mut tasks = JoinSet::new();
        let cancel = CancellationToken::new();
        let audit = AuditLog::open(&dir.path().join("audit.jsonl"), &mut tasks, cancel.clone())
            .await
            .expect("audit opens");
        let fence = WorkspaceFence::new(
            vec![dir.path().to_path_buf()],
            names(&["file_read", "file_write"]),
            None,
        )
        .await
        .expect("workspace fence");
        let shell = ShellDanger::new(names(&["shell"])).expect("danger patterns");
        Self {
            dir,
            hooks: Arc::new(HookPipeline::new(shell, fence, audit)),
            cancel,
            tasks,
        }
    }

    pub async fn close(&mut self) {
        self.cancel.cancel();
        while let Some(result) =
            tokio::time::timeout(Duration::from_secs(5), self.tasks.join_next())
                .await
                .expect("audit shutdown bounded")
        {
            result.expect("audit task joins").expect("audit persisted");
        }
    }

    pub async fn records(&self) -> Vec<HookPipelineResult> {
        tokio::fs::read_to_string(self.dir.path().join("audit.jsonl"))
            .await
            .expect("audit readable")
            .lines()
            .map(|line| serde_json::from_str(line).expect("typed audit record"))
            .collect()
    }
}

pub fn names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

pub fn scratch() -> TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repository root");
    tempfile::tempdir_in(root.join("target")).expect("temporary workspace")
}

pub fn base() -> HookContextBase {
    HookContextBase {
        agent_id: Some("test-agent".to_owned()),
        session_id: Some("test-session".to_owned()),
        timestamp: chrono::DateTime::from_timestamp_millis(1_700_000_000_000).expect("timestamp"),
        metadata: JsonObject::default(),
    }
}

pub fn turn() -> Turn {
    Turn {
        request: ModelRequest {
            model: "test-model".to_owned(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("hello".to_owned()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: Vec::new(),
            thinking: None,
            max_tokens: Some(100),
        },
        context: base(),
        tool_context: ToolContext {
            agent_id: Some("test-agent".to_owned()),
            working_dir: None::<PathBuf>,
            session: None,
        },
        authorized_tools: BTreeSet::new(),
        limits: TurnLimits {
            steps: 3,
            timeout: Duration::from_secs(20),
            cleanup_timeout: Duration::from_secs(5),
            response_bytes: 4096,
        },
    }
}

pub mod provider;

pub fn text(value: &str) -> kernel::model::ModelChunk {
    kernel::model::ModelChunk::Text {
        delta: value.to_owned(),
    }
}

pub fn call(index: u32, name: &str, arguments: &str) -> Vec<kernel::model::ModelChunk> {
    vec![
        kernel::model::ModelChunk::ToolCallStart {
            index,
            id: format!("call-{index}"),
            name: name.to_owned(),
        },
        kernel::model::ModelChunk::ToolCallDelta {
            index,
            arguments_delta: arguments.to_owned(),
        },
    ]
}

pub fn agent(
    provider: Arc<dyn kernel::model::ModelProvider>,
    tools: Vec<Arc<dyn kernel::tool::Tool>>,
    h: &Harness,
) -> agent_loop::AgentLoop {
    agent_loop::AgentLoop::new("provider".to_owned(), provider, tools, Arc::clone(&h.hooks))
        .expect("agent")
}
