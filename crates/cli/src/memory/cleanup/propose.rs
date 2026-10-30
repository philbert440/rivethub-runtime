use chrono::Utc;
use config::Environment;
use kernel::cleanup::BackfillBatch;
use kernel::tidy::TidyStore as _;
use memory_postgres::PostgresMemory;
use serde::Deserialize;
use serde::Serialize;

use super::Stage;
use super::filing::Filed;
use super::fold::Folds;
use crate::Error;

const PROPOSED: &str = "proposed.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Proposed {
    batch: String,
    proposals: usize,
    database: String,
}

pub(super) fn refuse_live(target: &str, env: &Environment) -> Result<String, Error> {
    let live = super::super::run::postgres_url(env).map_err(|_| Error::CleanupLiveUnknown)?;
    let live = database_name(&live)?;
    if database_name(target)? == live {
        return Err(Error::CleanupLive);
    }
    Ok(live)
}

pub(super) async fn write(
    stage: &Stage<'_>,
    filed: &Filed,
    folds: &Folds,
    target: &str,
    live: &str,
) -> Result<String, Error> {
    if let Some(done) = stage.book.read::<Proposed>(PROPOSED).await? {
        return Ok(format!(
            "batch {} with {} proposals was already written to {}\n",
            done.batch, done.proposals, done.database
        ));
    }
    let memory = PostgresMemory::connect(target).await?;
    let database = memory.database().await?;
    if database == live {
        memory.pool().close().await;
        return Err(Error::CleanupLive);
    }
    let batch = BackfillBatch {
        snapshot: stage.snapshot,
        tree: &filed.tree,
        filings: &filed.filings,
        pages: &folds.pages,
        generic: &filed.generic,
    }
    .batch(Utc::now())?;
    let stored = memory.propose(batch).await;
    memory.pool().close().await;
    let stored = stored?;
    let proposed = Proposed {
        batch: stored.id.to_string(),
        proposals: stored.proposals.len(),
        database,
    };
    stage.book.write(PROPOSED, &proposed).await?;
    Ok(format!(
        "proposed batch {} with {} proposals in {}\n",
        proposed.batch, proposed.proposals, proposed.database
    ))
}

fn database_name(url: &str) -> Result<String, Error> {
    let parsed = url::Url::parse(url).map_err(Error::MemoryUrl)?;
    let name = parsed.path().trim_start_matches('/');
    if name.is_empty() {
        return Err(Error::CleanupDatabase);
    }
    Ok(name.to_owned())
}
