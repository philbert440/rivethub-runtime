pub mod mask;
pub mod records;
pub mod script;

use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use rmcp::model::{CallToolRequestParams, ClientConfig, ServerPeerInfo, Tool};
use rmcp::service::{QuitReason, RoleClient, RunningService, ServiceError, ServiceExt};
use rmcp::transport::{IntoTransport, TokioChildProcess};
use serde::Serialize;
use serde_json::to_string_pretty;
use tokio::process::Command;

use crate::error::{Error, Result};
use records::{
    CALLS_FILE, CallOutcome, CallRecord, CallRequest, CallsDocument, META_FILE, MetaDocument,
    TOOLS_FILE, ToolsDocument,
};
use script::{Script, ScriptCall};

pub const USAGE: &str =
    "usage: cargo xtask record-mcp --out DIR [--script FILE] -- COMMAND [ARGS...]";
pub const DEFAULT_SCRIPT: &str = "xtask/data/mcp-script.json";
const SESSION_BIN: &str = "record-mcp";

#[derive(Debug, Clone)]
pub struct Invocation {
    pub out: PathBuf,
    pub script: PathBuf,
    pub command: Vec<String>,
}

impl Invocation {
    fn forward_args(&self) -> Vec<String> {
        let mut args = vec![
            "--out".to_owned(),
            self.out.display().to_string(),
            "--script".to_owned(),
            self.script.display().to_string(),
            "--".to_owned(),
        ];
        args.extend(self.command.iter().cloned());
        args
    }
}

pub fn parse(args: &[String]) -> Result<Invocation> {
    let mut out = None;
    let mut script = None;
    let mut items = args.iter();
    while let Some(arg) = items.next() {
        let name = arg.as_str();
        if name == "--out" {
            out = Some(PathBuf::from(next_value(&mut items, "--out")?));
        } else if name == "--script" {
            script = Some(PathBuf::from(next_value(&mut items, "--script")?));
        } else if name == "--" {
            break;
        } else {
            return Err(Error::Usage(format!("{USAGE}: unexpected argument {arg}")));
        }
    }
    let command: Vec<String> = items.cloned().collect();
    if command.is_empty() {
        return Err(Error::Usage(format!("{USAGE}: a command is required")));
    }
    Ok(Invocation {
        out: out.ok_or_else(|| Error::Usage(format!("{USAGE}: --out DIR is required")))?,
        script: script.unwrap_or_else(|| PathBuf::from(DEFAULT_SCRIPT)),
        command,
    })
}

fn next_value<'a, I>(items: &mut I, flag: &str) -> Result<String>
where
    I: Iterator<Item = &'a String>,
{
    items
        .next()
        .cloned()
        .ok_or_else(|| Error::Usage(format!("{USAGE}: {flag} needs a value")))
}

pub fn dispatch(invocation: &Invocation) -> Result<()> {
    let status = std::process::Command::new("cargo")
        .args(["run", "-q", "-p", "xtask", "--bin", SESSION_BIN, "--"])
        .args(invocation.forward_args())
        .status()
        .map_err(Error::Io)?;
    if status.success() {
        return Ok(());
    }
    Err(Error::Command {
        command: format!("cargo run -q -p xtask --bin {SESSION_BIN}"),
        stderr: String::new(),
    })
}

#[derive(Debug, Clone)]
pub struct Recording {
    pub server: ServerPeerInfo,
    pub tools: Vec<Tool>,
    pub calls: Vec<CallRecord>,
}

pub async fn collect<T, E, A>(transport: T, script: &Script) -> Result<Recording>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let service = ClientConfig::default()
        .serve(transport)
        .await
        .map_err(|error| Error::Initialize(Box::new(error)))?;
    let recorded = session(&service, script).await;
    let closed = close(service).await;
    match (recorded, closed) {
        (Ok(recording), Ok(())) => Ok(recording),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

async fn session(
    service: &RunningService<RoleClient, ClientConfig>,
    script: &Script,
) -> Result<Recording> {
    let server = service
        .peer_info()
        .ok_or(Error::ServerInfo)?
        .as_ref()
        .clone();
    let mut tools = service
        .list_all_tools()
        .await
        .map_err(|source| Error::ListTools { source })?;
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    let mut calls = Vec::with_capacity(script.calls.len());
    for call in &script.calls {
        calls.push(perform(service, call).await?);
    }
    Ok(Recording {
        server,
        tools,
        calls,
    })
}

async fn perform(
    service: &RunningService<RoleClient, ClientConfig>,
    call: &ScriptCall,
) -> Result<CallRecord> {
    let mut params = CallToolRequestParams::new(call.name.clone());
    params.arguments = call.arguments.clone();
    let outcome = match service.call_tool(params).await {
        Ok(result) => CallOutcome::Result { result },
        Err(ServiceError::McpError(error)) => CallOutcome::ProtocolError {
            code: error.code.0,
            message: error.message.to_string(),
        },
        Err(source) => {
            return Err(Error::CallTool {
                name: call.name.clone(),
                source,
            });
        }
    };
    Ok(CallRecord {
        request: CallRequest {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        },
        note: call.note.clone(),
        outcome,
    })
}

async fn close(service: RunningService<RoleClient, ClientConfig>) -> Result<()> {
    match service.cancel().await {
        Ok(QuitReason::JoinError(source)) | Err(source) => Err(Error::Close(source)),
        Ok(_) => Ok(()),
    }
}

pub async fn run(invocation: &Invocation) -> Result<()> {
    let script = script::load(&invocation.script)?;
    let (process, program) = child_command(&invocation.command)?;
    let transport = TokioChildProcess::new(process).map_err(|source| Error::Spawn {
        program: program.clone(),
        source,
    })?;
    let recording = collect(transport, &script).await?;
    let recorded_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let masked = write_documents(&invocation.out, &recording, &program, &recorded_at)?;
    report(&invocation.out, &recording, &masked)?;
    Ok(())
}

fn child_command(command: &[String]) -> Result<(Command, String)> {
    let mut parts = command.iter();
    let executable = parts.next().ok_or_else(|| Error::Usage(USAGE.to_owned()))?;
    let program = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Usage(format!("{USAGE}: the command program has no file name")))?;
    let mut process = Command::new(executable);
    process.args(parts);
    Ok((process, program))
}

pub fn write_documents(
    out: &Path,
    recording: &Recording,
    program: &str,
    recorded_at: &str,
) -> Result<Vec<mask::Masked>> {
    std::fs::create_dir_all(out)?;
    let mut masked = Vec::new();
    write_one(
        out,
        TOOLS_FILE,
        &ToolsDocument {
            tools: recording.tools.clone(),
        },
        &mut masked,
    )?;
    write_one(
        out,
        CALLS_FILE,
        &CallsDocument {
            calls: recording.calls.clone(),
        },
        &mut masked,
    )?;
    write_one(
        out,
        META_FILE,
        &MetaDocument {
            recorder_version: env!("CARGO_PKG_VERSION").to_owned(),
            program: program.to_owned(),
            recorded_at: recorded_at.to_owned(),
            server: recording.server.clone(),
        },
        &mut masked,
    )?;
    Ok(masked)
}

fn write_one<T: Serialize>(
    out: &Path,
    file: &'static str,
    document: &T,
    masked: &mut Vec<mask::Masked>,
) -> Result<()> {
    let text = to_string_pretty(document)?;
    let (masked_text, hits) = mask::mask_document(file, &text);
    let mut written = masked_text;
    written.push('\n');
    std::fs::write(out.join(file), written)?;
    masked.extend(hits);
    Ok(())
}

fn report(out: &Path, recording: &Recording, masked: &[mask::Masked]) -> Result<()> {
    let stdout = std::io::stdout();
    let mut sink = stdout.lock();
    writeln!(
        sink,
        "record-mcp: {} tools, {} calls -> {}",
        recording.tools.len(),
        recording.calls.len(),
        out.display()
    )?;
    for file in [TOOLS_FILE, CALLS_FILE, META_FILE] {
        writeln!(sink, "record-mcp: wrote {}", out.join(file).display())?;
    }
    for hit in masked {
        writeln!(sink, "MASKED {}: {}: {}", hit.file, hit.rule, hit.matched)?;
    }
    sink.flush()?;
    Ok(())
}
