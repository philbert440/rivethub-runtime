use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use agent_client_protocol as acp;
use agent_client_protocol::Client as _;
use clap::Parser;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::compat::TokioAsyncReadCompatExt as _;
use tokio_util::compat::TokioAsyncWriteCompatExt as _;

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct FakeArgs {
    #[arg(long)]
    session_id: String,
    #[arg(long, default_value = "none")]
    on_load: OnLoad,
    #[arg(long)]
    steps: Vec<String>,
    #[arg(long, default_value = "end-turn")]
    stop: String,
    #[arg(long)]
    crash_on: Option<CrashPoint>,
    #[arg(long)]
    hang_until_cancel: bool,
}

#[derive(Clone, clap::ValueEnum)]
enum OnLoad {
    None,
    Replay,
    Reject,
}

#[derive(Clone, clap::ValueEnum)]
enum CrashPoint {
    New,
    Prompt,
}

enum ClientCall {
    Notify {
        notification: acp::SessionNotification,
        ack: oneshot::Sender<()>,
    },
    Permission {
        request: acp::RequestPermissionRequest,
        reply: oneshot::Sender<Result<acp::RequestPermissionResponse, acp::Error>>,
    },
}

enum Step {
    Text(String),
    Thought(String),
    User(String),
    Tool(String),
    RawArrayTool,
    ToolUpdate(String, acp::ToolCallStatus),
    Plan,
    Commands,
    Mode(String),
    Permission,
}

fn parse_step(spec: &str) -> Result<Step, String> {
    let (kind, rest) = spec.split_once(':').unwrap_or((spec, ""));
    match kind {
        "text" => Ok(Step::Text(rest.to_owned())),
        "thought" => Ok(Step::Thought(rest.to_owned())),
        "user" => Ok(Step::User(rest.to_owned())),
        "tool" => Ok(Step::Tool(rest.to_owned())),
        "tool-raw-array" => Ok(Step::RawArrayTool),
        "tool-update" => {
            let (id, status) = rest
                .split_once(':')
                .ok_or_else(|| format!("tool-update needs id:status, got {spec:?}"))?;
            let status = match status {
                "pending" => acp::ToolCallStatus::Pending,
                "in-progress" => acp::ToolCallStatus::InProgress,
                "failed" => acp::ToolCallStatus::Failed,
                "completed" => acp::ToolCallStatus::Completed,
                other => return Err(format!("unknown tool-update status {other:?}")),
            };
            Ok(Step::ToolUpdate(id.to_owned(), status))
        }
        "plan" => Ok(Step::Plan),
        "commands" => Ok(Step::Commands),
        "mode" => Ok(Step::Mode(rest.to_owned())),
        "permission" => Ok(Step::Permission),
        other => Err(format!("unknown fake agent step {other:?}")),
    }
}

fn ready_response<T>(value: T) -> impl Future<Output = Result<T, acp::Error>> {
    std::future::ready(Ok(value))
}

fn unsupported<T>() -> impl Future<Output = Result<T, acp::Error>> {
    std::future::ready(Err(acp::Error::method_not_found()))
}

fn tool_update(id: &str, fields: acp::ToolCallUpdateFields) -> acp::SessionUpdate {
    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate {
        id: acp::ToolCallId(id.to_owned().into()),
        fields,
        meta: None,
    })
}

fn tool_call_update(title: String) -> acp::SessionUpdate {
    acp::SessionUpdate::ToolCall(acp::ToolCall {
        id: acp::ToolCallId("fake-tool-1".into()),
        title,
        kind: acp::ToolKind::Execute,
        status: acp::ToolCallStatus::InProgress,
        content: Vec::new(),
        locations: Vec::new(),
        raw_input: Some(serde_json::json!({"command": "ls -1"})),
        raw_output: None,
        meta: None,
    })
}

fn raw_array_tool_call() -> acp::SessionUpdate {
    acp::SessionUpdate::ToolCall(acp::ToolCall {
        id: acp::ToolCallId("fake-tool-1".into()),
        title: "raw".to_owned(),
        kind: acp::ToolKind::Other,
        status: acp::ToolCallStatus::InProgress,
        content: Vec::new(),
        locations: Vec::new(),
        raw_input: Some(serde_json::json!(["not", "an", "object"])),
        raw_output: None,
        meta: None,
    })
}

fn plan_update() -> acp::SessionUpdate {
    acp::SessionUpdate::Plan(acp::Plan {
        entries: vec![acp::PlanEntry {
            content: "measure the thing".to_owned(),
            priority: acp::PlanEntryPriority::High,
            status: acp::PlanEntryStatus::InProgress,
            meta: None,
        }],
        meta: None,
    })
}

fn commands_update() -> acp::SessionUpdate {
    acp::SessionUpdate::AvailableCommandsUpdate {
        available_commands: vec![acp::AvailableCommand {
            name: "review".to_owned(),
            description: "review the change".to_owned(),
            input: None,
            meta: None,
        }],
    }
}

fn text_content(text: impl Into<String>) -> acp::ContentBlock {
    acp::ContentBlock::from(text.into())
}

struct FakeAgent {
    args: FakeArgs,
    calls: mpsc::UnboundedSender<ClientCall>,
    cancelled: Arc<AtomicBool>,
    cancel_signalled: Arc<Notify>,
}

impl FakeAgent {
    fn session_id(&self) -> acp::SessionId {
        acp::SessionId(self.args.session_id.clone().into())
    }

    async fn emit(
        &self,
        session: &acp::SessionId,
        update: acp::SessionUpdate,
    ) -> Result<(), acp::Error> {
        let (ack, acked) = oneshot::channel();
        self.calls
            .send(ClientCall::Notify {
                notification: acp::SessionNotification {
                    session_id: session.clone(),
                    update,
                    meta: None,
                },
                ack,
            })
            .map_err(|_| acp::Error::internal_error())?;
        acked.await.map_err(|_| acp::Error::internal_error())
    }

    async fn run_step(
        &self,
        session: &acp::SessionId,
        step: Step,
    ) -> Result<Option<acp::StopReason>, acp::Error> {
        match step {
            Step::Text(text) => {
                self.emit(
                    session,
                    acp::SessionUpdate::AgentMessageChunk {
                        content: text_content(text),
                    },
                )
                .await?;
            }
            Step::Thought(text) => {
                self.emit(
                    session,
                    acp::SessionUpdate::AgentThoughtChunk {
                        content: text_content(text),
                    },
                )
                .await?;
            }
            Step::User(text) => {
                self.emit(
                    session,
                    acp::SessionUpdate::UserMessageChunk {
                        content: text_content(text),
                    },
                )
                .await?;
            }
            Step::Tool(title) => {
                self.emit(session, tool_call_update(title)).await?;
            }
            Step::RawArrayTool => {
                self.emit(session, raw_array_tool_call()).await?;
            }
            Step::ToolUpdate(id, status) => {
                let content = match status {
                    acp::ToolCallStatus::Completed => Some(vec![acp::ToolCallContent::Content {
                        content: text_content("build ok"),
                    }]),
                    acp::ToolCallStatus::Pending
                    | acp::ToolCallStatus::InProgress
                    | acp::ToolCallStatus::Failed => None,
                };
                self.emit(
                    session,
                    tool_update(
                        &id,
                        acp::ToolCallUpdateFields {
                            status: Some(status),
                            content,
                            ..Default::default()
                        },
                    ),
                )
                .await?;
            }
            Step::Plan => {
                self.emit(session, plan_update()).await?;
            }
            Step::Commands => {
                self.emit(session, commands_update()).await?;
            }
            Step::Mode(mode) => {
                self.emit(
                    session,
                    acp::SessionUpdate::CurrentModeUpdate {
                        current_mode_id: acp::SessionModeId(mode.into()),
                    },
                )
                .await?;
            }
            Step::Permission => {
                return self.permission_step(session).await;
            }
        }
        Ok(None)
    }

    async fn permission_step(
        &self,
        session: &acp::SessionId,
    ) -> Result<Option<acp::StopReason>, acp::Error> {
        let (reply, answered) = oneshot::channel();
        self.calls
            .send(ClientCall::Permission {
                request: acp::RequestPermissionRequest {
                    session_id: session.clone(),
                    tool_call: acp::ToolCallUpdate {
                        id: acp::ToolCallId("fake-tool-1".into()),
                        fields: acp::ToolCallUpdateFields {
                            status: Some(acp::ToolCallStatus::Pending),
                            title: Some("run the build".to_owned()),
                            raw_input: Some(serde_json::json!({"command": "cargo build"})),
                            ..Default::default()
                        },
                        meta: None,
                    },
                    options: vec![
                        acp::PermissionOption {
                            id: acp::PermissionOptionId("allow".into()),
                            name: "Allow".to_owned(),
                            kind: acp::PermissionOptionKind::AllowOnce,
                            meta: None,
                        },
                        acp::PermissionOption {
                            id: acp::PermissionOptionId("reject".into()),
                            name: "Reject".to_owned(),
                            kind: acp::PermissionOptionKind::RejectOnce,
                            meta: None,
                        },
                    ],
                    meta: None,
                },
                reply,
            })
            .map_err(|_| acp::Error::internal_error())?;
        let response = answered.await.map_err(|_| acp::Error::internal_error())??;
        let cancelled = matches!(response.outcome, acp::RequestPermissionOutcome::Cancelled);
        if cancelled {
            return Ok(Some(acp::StopReason::Refusal));
        }
        Ok(None)
    }
}

impl acp::Agent for FakeAgent {
    fn initialize(
        &self,
        _arguments: acp::InitializeRequest,
    ) -> impl Future<Output = Result<acp::InitializeResponse, acp::Error>> {
        ready_response(acp::InitializeResponse {
            protocol_version: acp::V1,
            agent_capabilities: acp::AgentCapabilities {
                load_session: !matches!(self.args.on_load, OnLoad::None),
                ..Default::default()
            },
            auth_methods: Vec::new(),
            meta: None,
        })
    }

    fn authenticate(
        &self,
        _arguments: acp::AuthenticateRequest,
    ) -> impl Future<Output = Result<acp::AuthenticateResponse, acp::Error>> {
        ready_response(acp::AuthenticateResponse::default())
    }

    fn new_session(
        &self,
        _arguments: acp::NewSessionRequest,
    ) -> impl Future<Output = Result<acp::NewSessionResponse, acp::Error>> {
        if self
            .args
            .crash_on
            .as_ref()
            .is_some_and(|point| matches!(point, CrashPoint::New))
        {
            std::process::abort();
        }
        ready_response(acp::NewSessionResponse {
            session_id: self.session_id(),
            modes: None,
            meta: None,
        })
    }

    async fn load_session(
        &self,
        _arguments: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        match self.args.on_load {
            OnLoad::Reject => {
                return Err(acp::Error::invalid_params().with_data("session not found"));
            }
            OnLoad::Replay => {
                self.emit(
                    &self.session_id(),
                    acp::SessionUpdate::AgentMessageChunk {
                        content: text_content("restored"),
                    },
                )
                .await?;
            }
            OnLoad::None => {}
        }
        ready_response(acp::LoadSessionResponse::default()).await
    }

    async fn set_session_mode(
        &self,
        arguments: acp::SetSessionModeRequest,
    ) -> Result<acp::SetSessionModeResponse, acp::Error> {
        self.emit(
            &arguments.session_id,
            acp::SessionUpdate::CurrentModeUpdate {
                current_mode_id: arguments.mode_id,
            },
        )
        .await?;
        Ok(acp::SetSessionModeResponse::default())
    }

    async fn prompt(
        &self,
        arguments: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        if self
            .args
            .crash_on
            .as_ref()
            .is_some_and(|point| matches!(point, CrashPoint::Prompt))
        {
            std::process::abort();
        }
        let mut scripted_stop = None;
        for spec in &self.args.steps {
            let step =
                parse_step(spec).map_err(|error| acp::Error::internal_error().with_data(error))?;
            if let Some(stop) = self.run_step(&arguments.session_id, step).await? {
                scripted_stop = Some(stop);
            }
        }
        if self.args.hang_until_cancel {
            while !self.cancelled.load(Ordering::SeqCst) {
                self.cancel_signalled.notified().await;
            }
        }
        let stop = scripted_stop.unwrap_or(match self.args.stop.as_str() {
            "max-tokens" => acp::StopReason::MaxTokens,
            "refusal" => acp::StopReason::Refusal,
            _ => acp::StopReason::EndTurn,
        });
        Ok(acp::PromptResponse {
            stop_reason: stop,
            meta: None,
        })
    }

    fn cancel(
        &self,
        _args: acp::CancelNotification,
    ) -> impl Future<Output = Result<(), acp::Error>> {
        self.cancelled.store(true, Ordering::SeqCst);
        self.cancel_signalled.notify_one();
        ready_response(())
    }

    fn ext_method(
        &self,
        _method: Arc<str>,
        _params: Arc<serde_json::value::RawValue>,
    ) -> impl Future<Output = Result<Arc<serde_json::value::RawValue>, acp::Error>> {
        unsupported()
    }

    fn ext_notification(
        &self,
        _method: Arc<str>,
        _params: Arc<serde_json::value::RawValue>,
    ) -> impl Future<Output = Result<(), acp::Error>> {
        unsupported()
    }
}

async fn forward(conn: acp::AgentSideConnection, mut calls: mpsc::UnboundedReceiver<ClientCall>) {
    while let Some(call) = calls.recv().await {
        match call {
            ClientCall::Notify { notification, ack } => {
                if conn.session_notification(notification).await.is_err() {
                    break;
                }
                if ack.send(()).is_err() {
                    break;
                }
            }
            ClientCall::Permission { request, reply } => {
                let outcome = conn.request_permission(request).await;
                if reply.send(outcome).is_err() {
                    break;
                }
            }
        }
    }
}

async fn serve(args: FakeArgs) -> Result<(), acp::Error> {
    let outgoing = tokio::io::stdout().compat_write();
    let incoming = tokio::io::stdin().compat();
    let (calls, call_rx) = mpsc::unbounded_channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_signalled = Arc::new(Notify::new());
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (conn, io) = acp::AgentSideConnection::new(
                FakeAgent {
                    args,
                    calls,
                    cancelled,
                    cancel_signalled,
                },
                outgoing,
                incoming,
                |future| {
                    tokio::task::spawn_local(future);
                },
            );
            tokio::task::spawn_local(forward(conn, call_rx));
            match io.await {
                Ok(()) => Ok(()),
                Err(error) => Err(acp::Error::internal_error().with_data(error.to_string())),
            }
        })
        .await
}

fn main() -> std::process::ExitCode {
    let args = FakeArgs::parse();
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return std::process::ExitCode::FAILURE;
    };
    if runtime.block_on(serve(args)).is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
