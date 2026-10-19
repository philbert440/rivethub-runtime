mod export;
mod io;
mod parse;
mod run;
mod target;

use config::Environment;
use tokio_util::sync::CancellationToken;

use crate::Report;

pub use export::MemoryExporter;

pub async fn execute(
    args: &[String],
    env: &Environment,
    shutdown: &CancellationToken,
    stdout_is_terminal: bool,
) -> Report {
    let notice = env.legacy_notice(config::POSTGRES_URL_VARIABLE, "RIVETOS_PG_URL");
    let result = match parse::parse(args) {
        Ok(None) => Ok(Report::success(help())),
        Ok(Some(command)) => run::run(command, env, shutdown, stdout_is_terminal).await,
        Err(error) => Err(error),
    };
    attach(notice, result)
}

fn attach(notice: Option<String>, result: Result<Report, crate::Error>) -> Report {
    let mut report = match result {
        Ok(report) => report,
        Err(error) => Report::failure(&error),
    };
    if let Some(notice) = notice {
        report.stderr = format!("warning: {notice}\n{}", report.stderr);
    }
    report
}

fn help() -> String {
    "Usage: rh memory <command>\n\
Commands:\n\
  queue-status [--json]\n\
  retry-failed --task <identifier> [--task <identifier>] [--error <substring>] [--limit <n>] [--dry-run] [--json]\n\
  requeue --task <identifier> [--limit <n>] [--dry-run] [--json]\n\
  backfill-tool-synth [--limit <n>] [--dry-run] [--json]\n\
  export [--out <file>] [--since <iso>] [--sqlite <file>]\n\
  import <file> [--dry-run] [--sqlite <file>]\n\
  tags backfill-rule [--since <iso>] [--sqlite <file>]\n\
  reembed [--dry-run] [--sqlite <file>]\n\
Postgres commands read RIVETHUB_PG_URL. export, import and tags backfill-rule use --sqlite or memory.sqlite.path when no Postgres URL is set. reembed runs on a SQLite memory file only.\n"
        .to_owned()
}
