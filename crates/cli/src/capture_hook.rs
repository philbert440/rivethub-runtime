use std::time::Duration;

use capture::hook::ClaudeHook;
use capture::spool::CaptureSpool;
use capture::writer::CaptureWriter;
use capture::writer::HttpCaptureTransport;
use chrono::DateTime;
use chrono::Utc;
use clap::Subcommand;
use config::Environment;
use tokio::io::AsyncReadExt as _;

use crate::status::Endpoint;
use crate::{Error, Exit, Report, db, status};

const SPOOL_DIR: &str = "capture-spool";
const POST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Subcommand)]
pub enum CaptureCommand {
    #[command(about = "Capture one Claude Code hook payload read from stdin")]
    Hook,
}

pub(crate) async fn run(command: CaptureCommand, env: &Environment) -> Result<Report, Error> {
    match command {
        CaptureCommand::Hook => {
            let mut stdin = String::new();
            tokio::io::stdin().read_to_string(&mut stdin).await?;
            Ok(hook(env, &stdin, Utc::now()).await)
        }
    }
}

pub async fn hook(env: &Environment, stdin: &str, now: DateTime<Utc>) -> Report {
    let stderr = match claude_hook(env).await {
        Ok(hook) => {
            let run = hook.run(stdin, now, &capture::hook::ingest_key(now)).await;
            run.stderr
        }
        Err(error) => format!("capture hook is not configured: {error}"),
    };
    Report {
        stdout: String::new(),
        stderr: if stderr.is_empty() {
            stderr
        } else {
            format!("{stderr}\n")
        },
        exit: Exit::Ok,
        stdout_raw: Vec::new(),
    }
}

async fn claude_hook(env: &Environment) -> Result<ClaudeHook<HttpCaptureTransport>, Error> {
    let loaded = db::load(env, None).await?;
    let paths = config::Paths::new(env)?;
    let Endpoint { url, client } = status::gateway(env, &loaded.config, POST_TIMEOUT).await?;
    let redaction = capture::redaction::resolve(Some(loaded.config.capture_redaction()), env);
    let writer = CaptureWriter::new(
        HttpCaptureTransport::new(url.as_str().trim_end_matches('/').to_owned(), client),
        CaptureSpool::new(paths.state_dir().join(SPOOL_DIR)),
        None,
        redaction,
    );
    Ok(ClaudeHook::new(writer, &paths))
}
