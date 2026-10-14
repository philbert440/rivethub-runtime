#[expect(
    clippy::redundant_pub_crate,
    reason = "private shared test module: unreachable_pub requires pub(crate) here, the nursery lint disagrees"
)]
mod common;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Duration;
use chrono::Utc;
use common::Ids;
use common::Scenario;
use common::ScenarioFile;
use common::TestResult;
use common::expect_json;
use common::expect_logs;
use common::instant;
use common::recording;
use common::table;
use memory_sqlite::ClaimedSqliteJob;
use memory_sqlite::FailOutcome;
use memory_sqlite::JobFailure;
use memory_sqlite::JobHandler;
use memory_sqlite::JobInsert;
use memory_sqlite::JobPayload;
use memory_sqlite::JobRunner;
use memory_sqlite::JobSweep;
use memory_sqlite::SqliteError;
use memory_sqlite::SqliteStore;
use memory_sqlite::retry_delay_ms;
use parking_lot::Mutex;
use serde::Serialize;
use testkit::Json;

const QUEUE: &str = "queue: enqueue/claim/fail/dead/revive/requeue/recover";
const RUNNER: &str = "runner: tick, handlers, sweeps, failures";
const HTTP: &str =
    "embed-target failures: HTTP 400 → dead, sweep revives once the endpoint recovers";
const OUTCOMES: &str = "embed outcomes: unembeddable, zero vector, model change, width change";

const SKIPPED: &[(&str, &str)] = &[];

#[derive(Serialize)]
struct Pair {
    n: u32,
    f: f64,
}

#[derive(Serialize)]
struct Revived {
    n: u32,
}

#[derive(Serialize)]
struct OkBody {
    i: u32,
}

#[derive(Serialize)]
struct BoomBody {
    x: u32,
}

#[derive(Serialize)]
struct EmptyPayload {}

#[derive(Serialize)]
struct Ran {
    task: String,
    payload: JobPayload,
    attempts: u32,
}

struct Remember(Arc<Mutex<Vec<Ran>>>);

struct Explode;

struct SweepOk;

struct SweepBoom;

#[async_trait]
impl JobHandler for Remember {
    async fn run(&self, job: &ClaimedSqliteJob) -> Result<(), JobFailure> {
        self.0.lock().push(Ran {
            task: job.task.clone(),
            payload: job.payload.clone(),
            attempts: job.attempts,
        });
        Ok(())
    }
}

#[async_trait]
impl JobHandler for Explode {
    async fn run(&self, _job: &ClaimedSqliteJob) -> Result<(), JobFailure> {
        Err(JobFailure::new("handler exploded"))
    }
}

#[async_trait]
impl JobSweep for SweepOk {
    async fn run(&self, _now: chrono::DateTime<Utc>) -> Result<(), JobFailure> {
        Ok(())
    }
}

#[async_trait]
impl JobSweep for SweepBoom {
    async fn run(&self, _now: chrono::DateTime<Utc>) -> Result<(), JobFailure> {
        Err(JobFailure::new("sweep exploded"))
    }
}

#[tokio::test]
async fn queue_replay_matches_the_recording() -> TestResult {
    let file: ScenarioFile = recording("jobs.json")?;
    let scenario = find(&file, QUEUE)?;
    let mut ids = Ids::default();
    replay_queue(scenario, &mut ids).await?;
    expect_logs(&scenario.logs, &Vec::new(), QUEUE, &mut ids)?;
    Ok(())
}

#[tokio::test]
async fn runner_replay_matches_the_recording() -> TestResult {
    let file: ScenarioFile = recording("jobs.json")?;
    replay_runner(find(&file, RUNNER)?).await
}

#[tokio::test]
async fn every_jobs_scenario_is_replayed_or_skipped() -> TestResult {
    let file: ScenarioFile = recording("jobs.json")?;
    let replayed = [QUEUE, RUNNER, HTTP, OUTCOMES];
    for scenario in &file.scenarios {
        let played = replayed.contains(&scenario.name.as_str());
        let skipped = SKIPPED.iter().any(|(name, _)| *name == scenario.name);
        assert!(
            played ^ skipped,
            "{} must be replayed or skipped, not both and not neither",
            scenario.name
        );
    }
    for (name, reason) in SKIPPED {
        assert!(
            file.scenarios.iter().any(|scenario| scenario.name == *name),
            "skip {name} ({reason}) is not a scenario"
        );
    }
    let skipped_names: Vec<&str> = SKIPPED.iter().map(|(name, _)| *name).collect();
    let missing: Vec<&str> = file
        .scenarios
        .iter()
        .map(|scenario| scenario.name.as_str())
        .filter(|name| !replayed.contains(name) && !skipped_names.contains(name))
        .collect();
    assert!(missing.is_empty(), "unlisted scenarios: {missing:?}");
    Ok(())
}

fn find<'a>(file: &'a ScenarioFile, name: &str) -> TestResult<&'a Scenario> {
    file.scenarios
        .iter()
        .find(|scenario| scenario.name == name)
        .ok_or_else(|| format!("missing scenario {name}").into())
}

async fn replay_queue(scenario: &Scenario, ids: &mut Ids) -> TestResult {
    let fresh = common::fresh_store(None)?;
    let store = &fresh.store;
    let mut last: Option<ClaimedSqliteJob> = None;
    let mut finished: Option<String> = None;
    for step in &scenario.steps {
        let now = instant(&step.at)?;
        let actual = queue_step(store, step.label.as_str(), now, &mut last, &mut finished).await?;
        if step.label == "claim" {
            finished = Some(required_id(last.as_ref())?);
        }
        expect_json(recorded(step), &actual, &step.label, ids)?;
    }
    Ok(())
}

async fn queue_step(
    store: &SqliteStore,
    label: &str,
    now: chrono::DateTime<Utc>,
    last: &mut Option<ClaimedSqliteJob>,
    finished: &mut Option<String>,
) -> TestResult<Json> {
    if let Some(enqueued) = enqueue_step(store, label, now).await? {
        return Ok(enqueued);
    }
    let actual = match label {
        "claim empty task list" => claim_json(store, Vec::new(), now, last).await?,
        "claim other task" => claim_json(store, vec!["other".to_owned()], now, last).await?,
        "claim t"
        | "claim t (no-key job is older: run_at order)"
        | "claim t again (k1 due)"
        | "claim" => claim_json(store, vec!["t".to_owned()], now, last).await?,
        "fail j1 (retry)" => fail_json(store, last.as_ref(), &"boom ".repeat(500), now).await?,
        "fail j1 (dead)" => fail_json(store, last.as_ref(), "second failure", now).await?,
        "revive k1" => bool_json(
            store
                .revive(
                    "k1".to_owned(),
                    serde_json::to_string(&Revived { n: 4 })?,
                    now,
                )
                .await?,
        ),
        "revive missing" => bool_json(
            store
                .revive("missing".to_owned(), "null".to_owned(), now)
                .await?,
        ),
        "recoverRunning" => Json::from(store.recover_running(now).await?),
        "fail to dead with maxAttempts reached?" => {
            let id = required_id(last.as_ref())?;
            *finished = Some(id.clone());
            lower_max(store, &id).await?;
            fail_json(store, last.as_ref(), "second failure", now).await?
        }
        "requeueDead(u) (none)" => Json::from(store.requeue_dead(Some("u".to_owned()), now).await?),
        "requeueDead(t)" => Json::from(store.requeue_dead(Some("t".to_owned()), now).await?),
        "requeueDead() all" => Json::from(store.requeue_dead(None, now).await?),
        "counts" => serde_json::to_value(store.job_counts().await?)?,
        "complete" => {
            store
                .complete_job(finished.clone().ok_or("no job to complete")?)
                .await?;
            Json::Null
        }
        "retryDelayMs 0..10" => {
            let pairs: Vec<(u32, u64)> = (0..=10)
                .map(|attempt| (attempt, retry_delay_ms(attempt)))
                .collect();
            serde_json::to_value(pairs)?
        }
        label if label.starts_with("rows") => Json::Array(table(store, "ros_jobs").await?),
        label => return Err(format!("unhandled queue step {label}").into()),
    };
    Ok(actual)
}

async fn enqueue_step(
    store: &SqliteStore,
    label: &str,
    now: chrono::DateTime<Utc>,
) -> TestResult<Option<Json>> {
    let json = match label {
        "enqueue k1" | "enqueue k1 again (dedup)" | "enqueue k1 while dead" => bool_json(
            store
                .enqueue_job(
                    JobInsert::new("t", serde_json::to_string(&Pair { n: 1, f: 0.5 })?, now)
                        .key("k1")
                        .max_attempts(2),
                )
                .await?,
        ),
        "enqueue no key, null payload" => bool_json(
            store
                .enqueue_job(JobInsert::new(
                    "t",
                    serde_json::to_string(&None::<u8>)?,
                    now,
                ))
                .await?,
        ),
        "enqueue future runAt" => bool_json(
            store
                .enqueue_job(
                    JobInsert::new("t", serde_json::to_string("later")?, now)
                        .key("k3")
                        .run_at(now + Duration::hours(1)),
                )
                .await?,
        ),
        _ => return Ok(None),
    };
    Ok(Some(json))
}

const NULL_RESULT: Json = Json::Null;

fn recorded(step: &common::Step) -> &Json {
    match step.result {
        Some(ref value) => value,
        None => &NULL_RESULT,
    }
}

async fn claim_json(
    store: &SqliteStore,
    tasks: Vec<String>,
    now: chrono::DateTime<Utc>,
    last: &mut Option<ClaimedSqliteJob>,
) -> TestResult<Json> {
    let claimed = store.claim_tasks(tasks, now).await?;
    let json = match &claimed {
        Some(job) => serde_json::to_value(job)?,
        None => Json::Null,
    };
    if claimed.is_some() {
        *last = claimed;
    }
    Ok(json)
}

async fn fail_json(
    store: &SqliteStore,
    job: Option<&ClaimedSqliteJob>,
    error: &str,
    now: chrono::DateTime<Utc>,
) -> TestResult<Json> {
    let outcome = store
        .fail_job(required_id(job)?, error.to_owned(), now)
        .await?;
    Ok(Json::String(match outcome {
        FailOutcome::Retry => "retry".to_owned(),
        FailOutcome::Dead => "dead".to_owned(),
    }))
}

fn required_id(job: Option<&ClaimedSqliteJob>) -> TestResult<String> {
    job.map(|job| job.id.clone())
        .ok_or_else(|| "no claimed job".into())
}

fn bool_json(value: bool) -> Json {
    Json::Bool(value)
}

async fn lower_max(store: &SqliteStore, id: &str) -> TestResult {
    let id = id.to_owned();
    store
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE ros_jobs SET max_attempts = attempts WHERE id = ?1",
                [id],
            )
            .map_err(|source| SqliteError::Query {
                operation: "test max attempts",
                source,
            })?;
            Ok(())
        })
        .await?;
    Ok(())
}

async fn replay_runner(scenario: &Scenario) -> TestResult {
    let fresh = common::fresh_store(None)?;
    let store = fresh.store.clone();
    let start = instant(&scenario.steps.first().ok_or("runner has no steps")?.at)?;
    seed_runner(&store, start).await?;
    let ran = Arc::new(Mutex::new(Vec::new()));
    let mut runner = JobRunner::new(store.clone());
    runner.on_task("ok", Arc::new(Remember(Arc::clone(&ran))));
    runner.on_task("boom", Arc::new(Explode));
    runner.on_sweep("broken", Duration::minutes(10), Arc::new(SweepBoom));
    runner.on_sweep("healthy", Duration::minutes(10), Arc::new(SweepOk));
    let mut ids = Ids::default();
    for step in &scenario.steps {
        let now = instant(&step.at)?;
        let actual = match step.label.as_str() {
            "tick 1 (recovers running once, batch 3)"
            | "tick 2"
            | "tick 3 (boom retried → dead)"
            | "tick 4 (nothing due; sweeps not due)"
            | "tick 5 (sweeps due)" => Json::from(runner.tick(now, 3).await?),
            "isRunning" => Json::Bool(runner.is_running()),
            "tick after halt" => {
                runner.halt();
                Json::from(runner.tick(now, 3).await?)
            }
            "rows" => Json::Array(table(&store, "ros_jobs").await?),
            "ran" => serde_json::to_value(&*ran.lock())?,
            "sweeps" => Json::from(runner.sweeps_completed()),
            label => return Err(format!("unhandled runner step {label}").into()),
        };
        expect_json(recorded(step), &actual, &step.label, &mut ids)?;
    }
    expect_logs(&scenario.logs, &store.logs(), RUNNER, &mut ids)?;
    Ok(())
}

async fn seed_runner(store: &SqliteStore, now: chrono::DateTime<Utc>) -> TestResult {
    for index in 0..4 {
        store
            .enqueue_job(JobInsert::new(
                "ok",
                serde_json::to_string(&OkBody { i: index })?,
                now,
            ))
            .await?;
    }
    store
        .enqueue_job(
            JobInsert::new("boom", serde_json::to_string(&BoomBody { x: 1 })?, now)
                .key("boom-1")
                .max_attempts(2),
        )
        .await?;
    store
        .enqueue_job(
            JobInsert::new("nohandler", serde_json::to_string(&EmptyPayload {})?, now).key("nh"),
        )
        .await?;
    common::execute(
        store,
        "UPDATE ros_jobs SET state = 'running', locked_at = created_at WHERE job_key = 'nh'",
    )
    .await?;
    Ok(())
}
