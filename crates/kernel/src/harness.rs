use crate::BoxError;
use crate::topics::DeliveredVia;
use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::mpsc::Receiver;
use wire::gateway::sessions::SessionSwitches;
use wire::harness::ApprovalDecision;
use wire::harness::HarnessId;
use wire::harness::PlanStatus;
use wire::harness::SteerOutcome;
use wire::json::JsonObject;
use wire::session_id::SessionId;
use wire::stream::ThinkingLevel;

#[derive(Debug, Error)]
pub enum HarnessAdapterError {
    #[error("harness does not support releasing session ownership")]
    ReleaseUnsupported,
    #[error("harness cannot steer a running turn")]
    SteeringUnsupported,
    #[error("harness rejected the session id: {0}")]
    InvalidSession(SessionId),
    #[error("harness session is unknown: {0}")]
    UnknownSession(SessionId),
    #[error("harness turn is already in flight: {0}")]
    TurnInFlight(SessionId),
    #[error("harness event stream was already taken: {0}")]
    EventStreamTaken(SessionId),
    #[error("the harness does not offer a {option} option")]
    OptionUnavailable { option: String },
    #[error("the harness does not offer {value} as its {option}")]
    OptionRejected { option: String, value: String },
    #[error("harness failed: {message}")]
    Failed {
        message: String,
        #[source]
        source: BoxError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    pub session: SessionId,
    pub harness: HarnessId,
    pub model: Option<String>,
    pub effort: Option<ThinkingLevel>,
    pub working_dir: Option<std::path::PathBuf>,
    pub startup_context: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPromptSupport {
    MetaAppend,
    CliFlag(&'static str),
    CliConfig(&'static str),
    None,
}

impl SystemPromptSupport {
    #[must_use]
    pub fn delivered_via(self) -> DeliveredVia {
        match self {
            Self::MetaAppend => DeliveredVia::MetaAppend,
            Self::CliFlag(_) | Self::CliConfig(_) => DeliveredVia::CliFlag,
            Self::None => DeliveredVia::None,
        }
    }

    #[must_use]
    pub fn cli_args(self, text: &str) -> Option<[String; 2]> {
        match self {
            Self::CliFlag(flag) => Some([flag.to_owned(), text.to_owned()]),
            Self::CliConfig(key) => Some(["-c".to_owned(), format!("{key}={}", toml_string(text))]),
            Self::MetaAppend | Self::None => None,
        }
    }
}

fn toml_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            control if control.is_control() => {
                quoted.push_str("\\u");
                let code = u32::from(control);
                for shift in [12, 8, 4, 0] {
                    quoted.extend(char::from_digit((code >> shift) & 0xF, 16));
                }
            }
            plain => quoted.push(plain),
        }
    }
    quoted.push('"');
    quoted
}

#[must_use]
pub fn cli_system_prompt(harness: HarnessId) -> SystemPromptSupport {
    match harness {
        HarnessId::ClaudeCode | HarnessId::Omp => {
            SystemPromptSupport::CliFlag("--append-system-prompt")
        }
        HarnessId::GrokBuild => SystemPromptSupport::CliFlag("--rules"),
        HarnessId::Codex => SystemPromptSupport::CliConfig("developer_instructions"),
        HarnessId::Opencode
        | HarnessId::Hermes
        | HarnessId::KimiCode
        | HarnessId::Dsh
        | HarnessId::Cursor
        | HarnessId::Cowork => SystemPromptSupport::None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_call_id: Option<String>,
    pub name: String,
    pub input: JsonObject,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallInfo {
    pub tool_call_id: String,
    pub name: String,
    pub input: JsonObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    pub status: ToolCallStatus,
    pub output: Option<String>,
    pub title: Option<String>,
    pub input: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub content: String,
    pub priority: PlanPriority,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommands {
    pub harness: HarnessId,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    UserMessageChunk { text: String },
    AgentMessageChunk { text: String },
    AgentThoughtChunk { text: String },
    Usage { usage: wire::stream::LlmUsage },
    ToolCall(ToolCallInfo),
    ToolCallUpdate(ToolCallUpdate),
    Plan(Vec<PlanEntry>),
    AvailableCommands(Vec<Command>),
    CurrentMode { mode: String },
    TurnEnded,
    Exited { code: Option<i32> },
    Permission(PermissionRequest),
    TerminalBytes { data: Vec<u8> },
    TerminalResize { cols: u16, rows: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnInput {
    Text(String),
    Attached {
        text: String,
        attachments: Vec<TurnAttachment>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnAttachment {
    pub path: std::path::PathBuf,
    pub media_type: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOption {
    Mode(String),
    Model(String),
    Effort(ThinkingLevel),
}

impl SessionOption {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Mode(_) => "mode",
            Self::Model(_) => "model",
            Self::Effort(_) => "effort",
        }
    }

    #[must_use]
    pub fn for_turn(model: Option<String>, effort: Option<ThinkingLevel>) -> Vec<Self> {
        model
            .map(Self::Model)
            .into_iter()
            .chain(effort.map(Self::Effort))
            .collect()
    }
}

#[async_trait]
pub trait HarnessAdapter: Send + Sync {
    async fn release(&self, _session: &SessionId) -> Result<(), HarnessAdapterError> {
        Err(HarnessAdapterError::ReleaseUnsupported)
    }
    async fn new_session(&self, spec: SessionSpec) -> Result<SessionId, HarnessAdapterError>;

    fn system_prompt(&self, _harness: HarnessId) -> SystemPromptSupport {
        SystemPromptSupport::None
    }

    async fn resume(&self, session: &SessionId) -> Result<(), HarnessAdapterError>;

    async fn prompt(
        &self,
        session: &SessionId,
        input: TurnInput,
    ) -> Result<(), HarnessAdapterError>;

    async fn steer(
        &self,
        _session: &SessionId,
        _input: TurnInput,
    ) -> Result<SteerOutcome, HarnessAdapterError> {
        Err(HarnessAdapterError::SteeringUnsupported)
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), HarnessAdapterError>;

    async fn answer_permission(
        &self,
        session: &SessionId,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), HarnessAdapterError>;

    async fn set_option(
        &self,
        session: &SessionId,
        option: SessionOption,
    ) -> Result<(), HarnessAdapterError>;

    fn switches(&self, _session: &SessionId) -> SessionSwitches {
        SessionSwitches::default()
    }

    fn mode(&self, _session: &SessionId) -> Option<String> {
        None
    }

    fn admit(
        &self,
        _session: &SessionId,
        option: &SessionOption,
    ) -> Result<(), HarnessAdapterError> {
        Err(HarnessAdapterError::OptionUnavailable {
            option: option.name().to_owned(),
        })
    }

    async fn events(
        &self,
        session: &SessionId,
    ) -> Result<Receiver<SessionEvent>, HarnessAdapterError>;

    async fn offerings(&self) -> Result<Vec<wire::harness::HarnessOffering>, HarnessAdapterError>;

    async fn commands(&self) -> Result<Vec<HarnessCommands>, HarnessAdapterError>;
}
