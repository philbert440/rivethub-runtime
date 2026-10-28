use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use agent_client_protocol::Agent;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Responder;
use agent_client_protocol::Stdio;
use agent_client_protocol::schema::v1 as acp;
use clap::Parser;
use parking_lot::Mutex;
use tokio::sync::Notify;

mod script;

use script::Chunk;
use script::Selection;
use script::Step;

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
    #[arg(long)]
    list_sessions: bool,
    #[arg(long, default_value = "plain")]
    prompt: PromptMode,
    #[arg(long, default_value = "none")]
    usage: UsageMode,
    #[arg(long)]
    record_meta: Option<std::path::PathBuf>,
    #[arg(long)]
    record_new: Option<std::path::PathBuf>,
}

const REPLAYED_CHUNKS: usize = 300;

#[derive(Clone, Copy, clap::ValueEnum)]
enum OnLoad {
    None,
    Replay,
    Reject,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum PromptMode {
    Plain,
    Echo,
    EchoImages,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum UsageMode {
    None,
    Reported,
    Meta,
}

const META_USAGE: &str = r#"{"usage":{"inputTokens":20185,"outputTokens":44,"totalTokens":20229,"cachedReadTokens":2176,"cacheCreationTokens":0,"reasoningTokens":43}}"#;

#[derive(Clone, Copy, clap::ValueEnum)]
enum CrashPoint {
    New,
    Prompt,
}

struct AgentState {
    session_id: String,
    on_load: OnLoad,
    steps: Vec<Step>,
    stop: acp::StopReason,
    crash_on: Option<CrashPoint>,
    hang_until_cancel: bool,
    list_sessions: bool,
    prompt: PromptMode,
    usage: UsageMode,
    record_meta: Option<std::path::PathBuf>,
    record_new: Option<std::path::PathBuf>,
    cancelled: AtomicBool,
    cancel_notify: Notify,
    selection: Mutex<Selection>,
}

fn prepare(args: FakeArgs) -> Option<AgentState> {
    let Ok(stop) = script::parse_stop(&args.stop) else {
        return None;
    };
    let mut steps = Vec::with_capacity(args.steps.len());
    for raw in &args.steps {
        let Ok(parsed) = script::parse_step(raw) else {
            return None;
        };
        steps.push(parsed);
    }
    Some(AgentState {
        session_id: args.session_id,
        on_load: args.on_load,
        steps,
        stop,
        crash_on: args.crash_on,
        hang_until_cancel: args.hang_until_cancel,
        list_sessions: args.list_sessions,
        prompt: args.prompt,
        usage: args.usage,
        record_meta: args.record_meta,
        record_new: args.record_new,
        cancelled: AtomicBool::new(false),
        cancel_notify: Notify::new(),
        selection: Mutex::new(Selection::initial()),
    })
}

fn session_id(state: &AgentState) -> acp::SessionId {
    acp::SessionId::new(state.session_id.clone())
}

fn current_options(state: &AgentState) -> Vec<acp::SessionConfigOption> {
    script::fixture_options(&state.selection.lock())
}

fn on_initialize(
    state: &AgentState,
    request: &acp::InitializeRequest,
    responder: Responder<acp::InitializeResponse>,
) -> Result<(), acp::Error> {
    responder.respond(
        acp::InitializeResponse::new(request.protocol_version).agent_capabilities(
            acp::AgentCapabilities::new()
                .load_session(!matches!(state.on_load, OnLoad::None))
                .prompt_capabilities(
                    acp::PromptCapabilities::new().image(state.prompt == PromptMode::EchoImages),
                )
                .session_capabilities(
                    acp::SessionCapabilities::new()
                        .list(state.list_sessions.then(acp::SessionListCapabilities::new)),
                ),
        ),
    )
}

fn on_new_session(
    state: &AgentState,
    request: &acp::NewSessionRequest,
    responder: Responder<acp::NewSessionResponse>,
) -> Result<(), acp::Error> {
    if matches!(state.crash_on, Some(CrashPoint::New)) {
        std::process::abort();
    }
    if let Some(path) = &state.record_new {
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| writeln!(file, "{}", request.cwd.display()))
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
    }
    if let Some(path) = &state.record_meta {
        let recorded = serde_json::to_string(&request.meta)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        std::fs::write(path, recorded)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
    }
    responder.respond(
        acp::NewSessionResponse::new(session_id(state)).config_options(current_options(state)),
    )
}

fn on_load(
    state: &AgentState,
    connection: &ConnectionTo<Client>,
    responder: Responder<acp::LoadSessionResponse>,
) -> Result<(), acp::Error> {
    match state.on_load {
        OnLoad::Reject => {
            responder.respond_with_error(acp::Error::invalid_params().data("session not found"))
        }
        OnLoad::Replay => {
            for _ in 0..REPLAYED_CHUNKS {
                connection.send_notification(acp::SessionNotification::new(
                    session_id(state),
                    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                        acp::ContentBlock::from("restored"),
                    )),
                ))?;
            }
            responder
                .respond(acp::LoadSessionResponse::new().config_options(current_options(state)))
        }
        OnLoad::None => responder.respond(acp::LoadSessionResponse::new()),
    }
}

fn on_set_config(
    state: &AgentState,
    request: acp::SetSessionConfigOptionRequest,
    responder: Responder<acp::SetSessionConfigOptionResponse>,
    connection: &ConnectionTo<Client>,
) -> Result<(), acp::Error> {
    let echo = {
        let mut selection = state.selection.lock();
        match script::apply_selection(&mut selection, &request) {
            Ok(echo) => echo,
            Err(error) => return responder.respond_with_error(error),
        }
    };
    connection.send_notification(acp::SessionNotification::new(
        request.session_id,
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::from(
            echo,
        ))),
    ))?;
    responder.respond(acp::SetSessionConfigOptionResponse::new(current_options(
        state,
    )))
}

fn spawn_prompt(
    state: Arc<AgentState>,
    request: acp::PromptRequest,
    responder: Responder<acp::PromptResponse>,
    connection: &ConnectionTo<Client>,
) -> Result<(), acp::Error> {
    let task_connection = connection.clone();
    connection.spawn(async move {
        let session_id = request.session_id;
        if state.prompt != PromptMode::Plain {
            for block in &request.prompt {
                script::emit_text(
                    &task_connection,
                    &session_id,
                    &described(block),
                    Chunk::Agent,
                )?;
            }
        }
        match execute_prompt(&state, &task_connection, &session_id).await {
            Ok(stop) => responder.respond(ended(stop, state.usage)?),
            Err(error) => responder.respond_with_error(error),
        }
    })
}

fn ended(stop: acp::StopReason, usage: UsageMode) -> Result<acp::PromptResponse, acp::Error> {
    let response = acp::PromptResponse::new(stop);
    match usage {
        UsageMode::None => Ok(response),
        UsageMode::Reported => Ok(response.usage(
            acp::Usage::new(23_252, 2, 4)
                .cached_read_tokens(10_470)
                .cached_write_tokens(12_776),
        )),
        UsageMode::Meta => {
            let meta: acp::Meta = serde_json::from_str(META_USAGE)
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
            Ok(response.meta(meta))
        }
    }
}

fn described(block: &acp::ContentBlock) -> String {
    match block {
        acp::ContentBlock::Text(text) => format!("text:{}", text.text),
        acp::ContentBlock::Image(image) => format!(
            "image:{}:{}:{}",
            image.mime_type,
            image.data.len(),
            image.uri.as_deref().unwrap_or_default()
        ),
        acp::ContentBlock::ResourceLink(link) => format!(
            "link:{}:{}:{}",
            link.name,
            link.mime_type.as_deref().unwrap_or_default(),
            link.uri
        ),
        acp::ContentBlock::Audio(_) | acp::ContentBlock::Resource(_) | _ => "other".to_owned(),
    }
}

async fn execute_prompt(
    state: &AgentState,
    connection: &ConnectionTo<Client>,
    session_id: &acp::SessionId,
) -> Result<acp::StopReason, acp::Error> {
    if matches!(state.crash_on, Some(CrashPoint::Prompt)) {
        std::process::abort();
    }
    for step in &state.steps {
        if let Some(stop) = script::run_step(connection, session_id, step).await? {
            return Ok(stop);
        }
    }
    if state.hang_until_cancel {
        wait_until_cancelled(state).await;
        return Ok(acp::StopReason::Cancelled);
    }
    Ok(state.stop)
}

async fn wait_until_cancelled(state: &AgentState) {
    loop {
        let notified = state.cancel_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if state.cancelled.load(Ordering::SeqCst) {
            return;
        }
        notified.await;
    }
}

fn on_cancel(state: &AgentState) {
    state.cancelled.store(true, Ordering::SeqCst);
    state.cancel_notify.notify_one();
}

async fn serve(state: Arc<AgentState>) -> Result<(), acp::Error> {
    let initialize_state = Arc::clone(&state);
    let new_state = Arc::clone(&state);
    let load_state = Arc::clone(&state);
    let config_state = Arc::clone(&state);
    let prompt_state = Arc::clone(&state);
    let cancel_state = Arc::clone(&state);
    Agent
        .builder()
        .name("fake-acp-agent")
        .on_receive_request(
            async move |request: acp::InitializeRequest, responder, _connection| {
                on_initialize(&initialize_state, &request, responder)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::NewSessionRequest, responder, _connection| {
                on_new_session(&new_state, &request, responder)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: acp::LoadSessionRequest, responder, connection| {
                on_load(&load_state, &connection, responder)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::SetSessionConfigOptionRequest, responder, connection| {
                on_set_config(&config_state, request, responder, &connection)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::PromptRequest, responder, connection| {
                spawn_prompt(Arc::clone(&prompt_state), request, responder, &connection)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |_notification: acp::CancelNotification, _connection| {
                on_cancel(&cancel_state);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
}

fn main() -> std::process::ExitCode {
    let args = FakeArgs::parse();
    let Some(state) = prepare(args) else {
        return std::process::ExitCode::FAILURE;
    };
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return std::process::ExitCode::FAILURE;
    };
    if runtime.block_on(serve(Arc::new(state))).is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
