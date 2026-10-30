mod cleanup;
mod export;
mod io;
mod parse;
mod run;
mod target;
mod titles;

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
  backfill-titles [--dry-run] [--json] [--sqlite <file>] [--model <name> --model-url <url> [--concurrency <n>]]\n\
  reembed [--dry-run] [--sqlite <file>]\n\
  consolidate-wiki [--limit <n>] [--dry-run] [--json]\n\
  cleanup (--dry-run [--database <url>] | --propose --database <url>) --out <dir> --model-url <url> --model <name> [--batch <n>] [--concurrency <n>] [--timeout <seconds>]\n\
Postgres commands read RIVETHUB_PG_URL. export, import and backfill-titles use --sqlite or memory.sqlite.path when no Postgres URL is set. reembed runs on a SQLite memory file only. backfill-titles --model asks that model for a short title for every summarized session whose title is a placeholder or weak, on Postgres only. cleanup reads memory read-only, plans the topic tree, filings and wiki folds with the model, checkpoints every stage in --out and writes report.md there; rerun it to resume. --propose also writes the plan as one tidy batch to the database --database names, never to the live memory database.\n"
        .to_owned()
}
