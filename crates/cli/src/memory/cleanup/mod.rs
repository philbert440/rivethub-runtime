mod book;
mod filing;
mod fold;
mod pool;
mod render;
mod report;
mod tree;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use config::Environment;
use kernel::cleanup::CleanupSource as _;
use kernel::cleanup::NameGuard;
use kernel::cleanup::Snapshot;
use memory_postgres::PostgresMemory;
use serde::Deserialize;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use workers::cleanup::ModelCall;

use crate::Error;
use crate::Report;
use crate::argv::Scan;
use crate::argv::Taken;
use book::Book;

const SNAPSHOT: &str = "snapshot.json";
const TIMINGS: &str = "timings.json";
const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_BATCH: usize = 20;
const DEFAULT_TIMEOUT_SECONDS: u64 = 900;
const ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::memory) struct CleanupArgs {
    out: PathBuf,
    model_url: String,
    model: String,
    settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    concurrency: usize,
    batch: usize,
    attempts: u32,
    timeout: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Timings {
    seconds: BTreeMap<String, f64>,
}

impl Timings {
    fn add(&mut self, stage: &str, started: Instant) {
        let spent: &mut f64 = self.seconds.entry(stage.to_owned()).or_default();
        *spent += started.elapsed().as_secs_f64();
    }
}

pub(in crate::memory) fn parse(args: &[String]) -> Result<CleanupArgs, Error> {
    let mut scan = Scan::new(args);
    let mut dry_run = false;
    let (mut out, mut model_url, mut model) = (None, None, None);
    let mut settings = Settings {
        concurrency: DEFAULT_CONCURRENCY,
        batch: DEFAULT_BATCH,
        attempts: ATTEMPTS,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
    };
    while let Some(token) = scan.current().map(str::to_owned) {
        if scan.flag("--dry-run") {
            dry_run = true;
            continue;
        }
        if let Some(value) = text(&mut scan, "--out")? {
            out = Some(PathBuf::from(value));
        } else if let Some(value) = text(&mut scan, "--model-url")? {
            model_url = Some(value);
        } else if let Some(value) = text(&mut scan, "--model")? {
            model = Some(value);
        } else if let Some(value) = text(&mut scan, "--concurrency")? {
            settings.concurrency = positive(&value, "--concurrency")?;
        } else if let Some(value) = text(&mut scan, "--batch")? {
            settings.batch = positive(&value, "--batch")?;
        } else if let Some(value) = text(&mut scan, "--timeout")? {
            settings.timeout = Duration::from_secs(
                u64::try_from(positive(&value, "--timeout")?)
                    .map_err(|_| Error::CleanupNumber("--timeout"))?,
            );
        } else if token.starts_with('-') {
            return Err(Error::UnknownOption(token));
        } else {
            return Err(Error::Unexpected(token));
        }
    }
    if !dry_run {
        return Err(Error::CleanupDryRun);
    }
    Ok(CleanupArgs {
        out: out.ok_or(Error::CleanupOut)?,
        model_url: model_url.ok_or(Error::CleanupModel)?,
        model: model.ok_or(Error::CleanupModel)?,
        settings,
    })
}

pub(in crate::memory) async fn run(
    args: CleanupArgs,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let book = Book::open(args.out.clone()).await?;
    let mut timings: Timings = book.read(TIMINGS).await?.unwrap_or_default();
    let started = Instant::now();
    let snapshot = if let Some(snapshot) = book.read::<Snapshot>(SNAPSHOT).await? {
        snapshot
    } else {
        let snapshot = gather(super::run::postgres_url(env)?, env).await?;
        book.write(SNAPSHOT, &snapshot).await?;
        timings.add("gather", started);
        book.write(TIMINGS, &timings).await?;
        snapshot
    };
    book.note(&format!(
        "snapshot: {} conversations, {} pages",
        snapshot.conversations.len(),
        snapshot.pages.len()
    ))
    .await?;
    let provider = providers::OpenAiCompatible::build(
        config::ProviderType::Vllm,
        &config::provider::OpenaiProvider {
            base_url: Some(args.model_url.clone()),
            model: Some(args.model.clone()),
            probe_models: Some(false),
            ..config::provider::OpenaiProvider::default()
        },
    )?
    .with_shutdown(shutdown.child_token());
    let call = ModelCall {
        provider: &provider,
        model: &args.model,
        timeout: args.settings.timeout,
        cancel: shutdown,
    };
    let guard = NameGuard::new(
        snapshot
            .conversations
            .iter()
            .filter_map(|conversation| conversation.title()),
        snapshot.refused.iter().map(String::as_str),
    );
    let stage = Stage {
        book: &book,
        snapshot: &snapshot,
        call: &call,
        settings: &args.settings,
        guard: &guard,
    };
    let started = Instant::now();
    let proposed = tree::stage(&stage).await;
    timings.add("tree", started);
    book.write(TIMINGS, &timings).await?;
    let started = Instant::now();
    let filed = filing::stage(&stage, proposed?).await;
    timings.add("filing", started);
    book.write(TIMINGS, &timings).await?;
    let filed = filed?;
    let started = Instant::now();
    let folds = fold::stage(&stage, &filed).await;
    timings.add("fold", started);
    book.write(TIMINGS, &timings).await?;
    let folds = folds?;
    let summary = report::write(&stage, &filed, &folds, &timings).await?;
    Ok(Report::success(format!(
        "{summary}\nwrote {}\n",
        book.path(report::REPORT).display()
    )))
}

struct Stage<'a> {
    book: &'a Book,
    snapshot: &'a Snapshot,
    call: &'a ModelCall<'a>,
    settings: &'a Settings,
    guard: &'a NameGuard,
}

impl Stage<'_> {
    async fn incomplete(&self, stage: &'static str, failed: usize) -> Result<(), Error> {
        if failed == 0 {
            return Ok(());
        }
        self.book
            .note(&format!("{stage}: {failed} job(s) failed; rerun to resume"))
            .await?;
        Err(Error::CleanupIncomplete { stage, failed })
    }
}

async fn gather(url: String, env: &Environment) -> Result<Snapshot, Error> {
    let (_, loaded) = crate::configuration::load(env).await?;
    let memory = PostgresMemory::connect_read_only(&url)
        .await?
        .with_wiki_directory(loaded.config.wiki_directory(env));
    let snapshot = memory.snapshot().await;
    memory.pool().close().await;
    Ok(snapshot?)
}

fn text(scan: &mut Scan<'_>, flag: &'static str) -> Result<Option<String>, Error> {
    match scan.option(flag, None) {
        Taken::Value(value) if !value.is_empty() => Ok(Some(value)),
        Taken::Value(_) | Taken::Missing => Err(Error::CleanupValue(flag)),
        Taken::Absent => Ok(None),
    }
}

fn positive(text: &str, flag: &'static str) -> Result<usize, Error> {
    match text.parse::<usize>() {
        Ok(value) if value > 0 => Ok(value),
        Ok(_) | Err(_) => Err(Error::CleanupNumber(flag)),
    }
}
