use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use memory_postgres::PostgresMemory;
use memory_postgres::dump::CopySource;
use memory_postgres::queue::QueueHealth;
use memory_sqlite::SqliteStore;
use memory_sqlite::import::ImportTable;
use strum::IntoEnumIterator;
use tokio_util::sync::CancellationToken;

use super::io;
use super::parse::MemoryCommand;
use super::target::Kind;
use super::target::Target;
use super::target::resolve;
use crate::Error;
use crate::Report;
use config::Environment;

pub(super) async fn run(
    command: MemoryCommand,
    env: &Environment,
    shutdown: &CancellationToken,
    stdout_is_terminal: bool,
) -> Result<Report, Error> {
    match command {
        MemoryCommand::QueueStatus { json } => queue_status(json, env).await,
        MemoryCommand::Retry {
            tasks,
            error,
            limit,
            dry_run,
            json,
        } => retry(tasks, error, limit, dry_run, json, env).await,
        MemoryCommand::Requeue {
            task,
            limit,
            clamped,
            dry_run,
            json,
        } => requeue(task, limit, clamped, dry_run, json, env).await,
        MemoryCommand::ToolSynth {
            limit,
            dry_run,
            json,
        } => tool_synth(limit, dry_run, json, env).await,
        MemoryCommand::Export { out, since, sqlite } => {
            export(out, since, sqlite, env, shutdown, stdout_is_terminal).await
        }
        MemoryCommand::Import {
            path,
            dry_run,
            sqlite,
        } => import(path, dry_run, sqlite, env, shutdown).await,
        MemoryCommand::Rules { since, sqlite } => rules(since, sqlite, env).await,
    }
}

async fn queue_status(json: bool, env: &Environment) -> Result<Report, Error> {
    let memory = PostgresMemory::connect(&postgres_url(env)?).await?;
    let rows = memory.queue_status().await?;
    if json {
        Ok(Report::success(format!(
            "{}\n",
            serde_json::to_string(&rows)?
        )))
    } else {
        Ok(Report::success(human_health(&rows)))
    }
}

async fn retry(
    tasks: Vec<String>,
    error: Option<String>,
    limit: Option<u32>,
    dry_run: bool,
    json: bool,
    env: &Environment,
) -> Result<Report, Error> {
    let memory = PostgresMemory::connect(&postgres_url(env)?).await?;
    let reset = memory
        .retry_failed(&tasks, error.as_deref(), limit, dry_run)
        .await?;
    if json {
        Ok(Report::success(format!(
            "{}\n",
            serde_json::to_string(&RetryBody { reset, dry_run })?
        )))
    } else {
        Ok(Report::success(format!("reset {reset} dead job(s)\n")))
    }
}

async fn requeue(
    task: kernel::queue::JobName,
    limit: u32,
    clamped: bool,
    dry_run: bool,
    json: bool,
    env: &Environment,
) -> Result<Report, Error> {
    let memory = PostgresMemory::connect(&postgres_url(env)?).await?;
    let requeued = memory.requeue_failed(&task, limit, dry_run).await?;
    if json {
        Ok(Report::success(format!(
            "{}\n",
            serde_json::to_string(&RequeueBody {
                requeued,
                dry_run,
                limit,
            })?
        )))
    } else {
        Ok(Report::success(requeue_text(requeued, clamped)))
    }
}

async fn tool_synth(
    limit: Option<u32>,
    dry_run: bool,
    json: bool,
    env: &Environment,
) -> Result<Report, Error> {
    let memory = PostgresMemory::connect(&postgres_url(env)?).await?;
    let enqueued = memory.backfill_tool_synth(limit, dry_run).await?;
    if json {
        Ok(Report::success(format!(
            "{}\n",
            serde_json::to_string(&SynthBody { enqueued, dry_run })?
        )))
    } else {
        Ok(Report::success(format!(
            "enqueued {enqueued} synthesize-tool-call job(s)\n"
        )))
    }
}

async fn export(
    out: Option<PathBuf>,
    since: Option<DateTime<Utc>>,
    sqlite: Option<PathBuf>,
    env: &Environment,
    shutdown: &CancellationToken,
    stdout_is_terminal: bool,
) -> Result<Report, Error> {
    let plain = match resolve(env, sqlite, Kind::Either)? {
        Target::Postgres(url) => export_postgres(&url, since, shutdown).await?,
        Target::Sqlite(path) => export_sqlite(&path, since, shutdown).await?,
    };
    let bytes = tokio::task::spawn_blocking(move || io::gzip(&plain)).await??;
    publish(out, bytes, stdout_is_terminal).await
}

async fn import(
    path: PathBuf,
    dry_run: bool,
    sqlite: Option<PathBuf>,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let target = resolve(env, sqlite, Kind::Either)?;
    if let Target::Postgres(url) = &target
        && super::target::cloud_url(url)?
    {
        return Ok(io::cloud_refusal());
    }
    let keep_file = matches!(target, Target::Sqlite(_));
    let loaded = io::read_dump(&path, keep_file).await?;
    let header = io::preamble(&loaded.text)?;
    match target {
        Target::Postgres(url) => {
            import_postgres(&url, &loaded.text, dry_run, header, shutdown).await
        }
        Target::Sqlite(path) => import_sqlite(path, loaded, dry_run, header, shutdown).await,
    }
}

async fn rules(
    since: Option<DateTime<Utc>>,
    sqlite: Option<PathBuf>,
    env: &Environment,
) -> Result<Report, Error> {
    let applied = match resolve(env, sqlite, Kind::Either)? {
        Target::Postgres(url) => {
            PostgresMemory::connect(&url)
                .await?
                .backfill_project_rules(since)
                .await?
        }
        Target::Sqlite(path) => {
            let store = open_sqlite(&path).await?;
            apply_sqlite_rules(&store, since).await?
        }
    };
    Ok(Report::success(format!(
        "applied {applied} project rule tag(s)\n"
    )))
}

async fn export_postgres(
    url: &str,
    since: Option<DateTime<Utc>>,
    shutdown: &CancellationToken,
) -> Result<Vec<u8>, Error> {
    let origin = super::target::origin_for_url(url)?;
    let tables = copy_sources();
    let memory = PostgresMemory::connect(url).await?;
    Ok(memory
        .export_memory_dump(&tables, since, &origin, shutdown)
        .await?)
}

async fn export_sqlite(
    path: &Path,
    since: Option<DateTime<Utc>>,
    shutdown: &CancellationToken,
) -> Result<Vec<u8>, Error> {
    let origin = super::target::sqlite_origin()?;
    let store = open_sqlite(path).await?;
    Ok(store
        .export_pg_dump(since, origin, shutdown.clone())
        .await?)
}

async fn import_postgres(
    url: &str,
    text: &str,
    dry_run: bool,
    header: io::Header,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let origin = io::finish_origin(header, super::target::origin_for_url(url)?);
    let imported = PostgresMemory::connect(url)
        .await?
        .import_memory_dump(text, dry_run, shutdown)
        .await?;
    io::summary_report(origin, imported.rows, dry_run)
}

async fn import_sqlite(
    database: PathBuf,
    loaded: io::Loaded,
    dry_run: bool,
    header: io::Header,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let origin = io::finish_origin(header, super::target::sqlite_origin()?);
    let store = open_sqlite(&database).await?;
    let dump = loaded.dump_path();
    let report = if dry_run {
        store.preview_pg_dump(dump, shutdown.clone()).await?
    } else {
        store.import_pg_dump(dump, shutdown.clone()).await?
    };
    io::summary_report(origin, io::sqlite_rows(report.rows), dry_run)
}

async fn apply_sqlite_rules(
    store: &SqliteStore,
    since: Option<DateTime<Utc>>,
) -> Result<u64, Error> {
    let found = store.conversation_directories(since).await?;
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in found {
        grouped.entry(row.cwd).or_default().push(row.id);
    }
    let mut applied = 0u64;
    for (cwd, ids) in grouped {
        let Some(tag) = memory_postgres::project::project_rule_tag(Some(cwd), true).await? else {
            continue;
        };
        for id in ids {
            applied += store
                .apply_project_rule(id, tag.value.clone(), tag.display.clone())
                .await?;
        }
    }
    Ok(applied)
}

async fn publish(out: Option<PathBuf>, bytes: Vec<u8>, tty: bool) -> Result<Report, Error> {
    if let Some(path) = out {
        let display = path.display().to_string();
        tokio::task::spawn_blocking(move || io::write_bytes(&path, &bytes)).await??;
        return Ok(Report::success(format!("wrote {display}\n")));
    }
    if tty {
        return Err(Error::GzipTty);
    }
    Ok(Report {
        stdout: String::new(),
        stderr: String::new(),
        exit: crate::Exit::Ok,
        stdout_raw: bytes,
    })
}

fn postgres_url(env: &Environment) -> Result<String, Error> {
    match resolve(env, None, Kind::Postgres)? {
        Target::Postgres(url) => Ok(url),
        Target::Sqlite(_) => Err(Error::PostgresUrlRequired),
    }
}

async fn open_sqlite(path: &Path) -> Result<SqliteStore, Error> {
    let path = path.to_path_buf();
    Ok(tokio::task::spawn_blocking(move || SqliteStore::open(path)).await??)
}

fn human_health(rows: &[QueueHealth]) -> String {
    let mut lines = Vec::new();
    for row in rows {
        let mut line = format!(
            "{} pending={} running={} scheduled={} dead={} recent_dead={}",
            row.task, row.pending, row.running, row.scheduled, row.dead, row.recent_dead
        );
        if let Some(minutes) = row.oldest_pending_minutes {
            line.push_str(" oldest_pending_minutes=");
            line.push_str(&minutes.to_string());
        }
        if let Some(error) = &row.last_error {
            line.push_str(" last_error=");
            line.push_str(error);
        }
        lines.push(line);
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn requeue_text(count: u64, clamped: bool) -> String {
    if clamped {
        format!("requeued {count} dead job(s) (limit 1000)\n")
    } else {
        format!("requeued {count} dead job(s)\n")
    }
}

fn copy_sources() -> Vec<CopySource<'static>> {
    ImportTable::iter().filter_map(copy_source).collect()
}

fn copy_source(table: ImportTable) -> Option<CopySource<'static>> {
    let columns = table.dump_columns()?;
    let (relation, compatibility) = match table {
        ImportTable::Conversations => ("rh_conversations", "ros_conversations"),
        ImportTable::Messages => ("rh_messages", "ros_messages"),
        ImportTable::Summaries => ("rh_summaries", "ros_summaries"),
        ImportTable::SummarySources => ("rh_summary_sources", "ros_summary_sources"),
        ImportTable::Tags => ("rh_tags", "ros_tags"),
        ImportTable::TagTaxonomy => ("rh_tag_taxonomy", "ros_tag_taxonomy"),
        ImportTable::WikiTopics => ("rh_wiki_topics", "ros_wiki_topics"),
        ImportTable::WikiProvenance => ("rh_wiki_provenance", "ros_wiki_provenance"),
        ImportTable::WikiExtractions => ("rh_wiki_extractions", "ros_wiki_extractions"),
        ImportTable::WikiCitations => ("rh_wiki_citations", "ros_wiki_citations"),
        ImportTable::WikiRedirects => ("rh_wiki_redirects", "ros_wiki_redirects"),
        ImportTable::Jobs | ImportTable::Meta | ImportTable::EmbedQueue => return None,
    };
    Some(CopySource {
        relation,
        compatibility,
        columns,
    })
}

#[derive(serde::Serialize)]
struct RetryBody {
    reset: u64,
    dry_run: bool,
}

#[derive(serde::Serialize)]
struct RequeueBody {
    requeued: u64,
    dry_run: bool,
    limit: u32,
}

#[derive(serde::Serialize)]
struct SynthBody {
    enqueued: u64,
    dry_run: bool,
}
