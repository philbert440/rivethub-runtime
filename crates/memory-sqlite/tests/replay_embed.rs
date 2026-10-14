#[expect(
    clippy::redundant_pub_crate,
    reason = "private shared test module: unreachable_pub requires pub(crate) here, the nursery lint disagrees"
)]
mod common;

use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use common::Ids;
use common::Scenario;
use common::ScenarioFile;
use common::TestResult;
use common::expect_json;
use common::expect_logs;
use common::instant;
use common::recording;
use common::step_result;
use common::table;
use memory_sqlite::EmbedRunner;
use memory_sqlite::SqliteError;
use memory_sqlite::SqliteStore;
use rusqlite::OptionalExtension as _;
use serde::Serialize;
use testkit::FakeProvider;
use testkit::Json;
use wire::message::Role;

const HTTP: &str =
    "embed-target failures: HTTP 400 → dead, sweep revives once the endpoint recovers";
const OUTCOMES: &str = "embed outcomes: unembeddable, zero vector, model change, width change";
const SENTENCE: &str = "how do we deploy the AcmeKit service to staging";
const WIDTH_REQUEUE: &str = "a width change re-queues every cleared row; the recording leaves jobs empty and does not re-embed them on this tick";
const WIDTH_DUE: &str = "2026-10-10T13:11:00.001Z";

#[derive(Serialize)]
struct MessageRow {
    embed_status: Option<String>,
    embed_error: Option<String>,
    embed_failures: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    embedding_hex: Option<String>,
}

#[derive(Serialize)]
struct HttpRows {
    jobs: Vec<Json>,
    message: Vec<MessageRow>,
}

#[derive(Serialize)]
struct OutcomeRow {
    event_id: String,
    embed_status: Option<String>,
    embed_error: Option<String>,
    embed_failures: i64,
    bytes: Option<i64>,
}

#[derive(Serialize)]
struct MetaRow {
    key: String,
    value: String,
}

#[derive(Serialize)]
struct DoneRow {
    embed_status: Option<String>,
    bytes: Option<i64>,
}

#[derive(Serialize)]
struct AfterModel {
    meta: Vec<MetaRow>,
    messages: Vec<DoneRow>,
    jobs: Vec<Json>,
}

#[derive(Serialize)]
struct WidthRow {
    event_id: String,
    embed_status: Option<String>,
    embed_error: Option<String>,
    embed_failures: i64,
    bytes: Option<i64>,
}

#[derive(Serialize)]
struct WidthState {
    messages: Vec<WidthRow>,
    jobs: Vec<Json>,
}

#[derive(Serialize)]
struct ClearedRow {
    event_id: String,
    embed_status: Option<String>,
    embed_error: Option<String>,
    bytes: Option<i64>,
}

#[derive(Serialize)]
struct ClearedState {
    meta: Vec<MetaRow>,
    messages: Vec<ClearedRow>,
    jobs: Vec<Json>,
}

struct Phase {
    fresh: common::Fresh,
    embedder: EmbedRunner,
    fake: Arc<FakeProvider>,
}

impl Phase {
    fn store(&self) -> &SqliteStore {
        &self.fresh.store
    }
}

#[tokio::test]
async fn http_failures_revive_when_the_endpoint_recovers() -> TestResult {
    let file: ScenarioFile = recording("jobs.json")?;
    let scenario = find(&file, HTTP)?;
    let fresh = common::fresh_store(None)?;
    let store = fresh.store.clone();
    let fake = Arc::new(FakeProvider::http_errors(400));
    let embedder = EmbedRunner::attach(&store, fake.clone(), "fake-embed-8", None)?;
    capture(&store, "m1", SENTENCE, instant("2026-10-04T12:00:00.000Z")?).await?;
    let mut ids = Ids::default();
    for step in &scenario.steps {
        let now = instant(&step.at)?;
        let actual = http_step(&store, &embedder, &fake, step.label.as_str(), now).await?;
        expect_json(step_result(step)?, &actual, &step.label, &mut ids)?;
    }
    expect_logs(&scenario.logs, &store.logs(), HTTP, &mut ids)?;
    Ok(())
}

async fn http_step(
    store: &SqliteStore,
    embedder: &EmbedRunner,
    fake: &FakeProvider,
    label: &str,
    now: DateTime<Utc>,
) -> TestResult<Json> {
    let actual = match label {
        "runJobs 1" | "runJobs 2" | "runJobs 3" | "runJobs 4" | "runJobs 5" | "runJobs 6" => {
            Json::from(embedder.run_jobs(now).await?)
        }
        "runJobs after recovery (sweep requeues the dead job)" => {
            fake.use_fnv(8).await;
            Json::from(embedder.run_jobs(now).await?)
        }
        "rows 1" | "rows 2" | "rows 3" | "rows 4" | "rows 5" | "rows 6" | "rows after recovery" => {
            serde_json::to_value(HttpRows {
                jobs: table(store, "ros_jobs").await?,
                message: message_rows(store).await?,
            })?
        }
        "health while failing" | "health after recovery" => {
            serde_json::to_value(store.health(now).await?)?
        }
        other => return Err(format!("unhandled http step {other}").into()),
    };
    Ok(actual)
}

#[tokio::test]
async fn embed_outcomes_cover_model_and_width_changes() -> TestResult {
    let file: ScenarioFile = recording("jobs.json")?;
    let scenario = find(&file, OUTCOMES)?;
    let cases = unembeddable_case().await?;
    let mut deployed: Option<Phase> = None;
    let mut ids = Ids::default();
    for step in &scenario.steps {
        let now = instant(&step.at)?;
        let actual = if step.label.starts_with("open") {
            let phase = deployed_case().await?;
            let actual = outcome_step(&phase, step.label.as_str(), now).await?;
            deployed = Some(phase);
            actual
        } else if let Some(phase) = deployed.as_mut() {
            outcome_step(phase, step.label.as_str(), now).await?
        } else {
            outcome_step(&cases, step.label.as_str(), now).await?
        };
        if step.label == "state" && step.at == "2026-10-10T13:11:00.000Z" {
            expect_width_requeue(step_result(step)?, &actual, &mut ids)?;
            let phase = deployed.as_ref().ok_or("width phase")?;
            assert_requeued_jobs(phase.store(), &actual).await?;
        } else {
            expect_json(step_result(step)?, &actual, &step.label, &mut ids)?;
        }
    }
    let deployed = deployed.ok_or("the model-change phase did not open")?;
    expect_logs(&scenario.logs, &deployed.store().logs(), OUTCOMES, &mut ids)?;
    Ok(())
}

fn expect_width_requeue(recorded: &Json, actual: &Json, ids: &mut Ids) -> TestResult {
    let recorded_jobs = recorded.get("jobs").ok_or("recorded jobs")?;
    if recorded_jobs != &Json::Array(Vec::new()) {
        return Err(format!("{WIDTH_REQUEUE}: the recording no longer leaves jobs empty").into());
    }
    expect_json(&without_jobs(recorded), &without_jobs(actual), "state", ids)
}

fn without_jobs(value: &Json) -> Json {
    let mut copy = value.clone();
    if let Json::Object(map) = &mut copy {
        map.remove("jobs");
    }
    copy
}

async fn assert_requeued_jobs(store: &SqliteStore, actual: &Json) -> TestResult {
    let jobs = actual
        .get("jobs")
        .and_then(Json::as_array)
        .ok_or("actual jobs")?;
    if jobs.len() != 3 {
        return Err(format!("{WIDTH_REQUEUE}: {} jobs", jobs.len()).into());
    }
    let mut expected = Vec::new();
    for event in ["e1", "e2", "e3"] {
        expected.push(format!(
            "embed-ros_messages-{}",
            message_id(store, event).await?
        ));
    }
    expected.sort();
    let mut found = Vec::new();
    for job in jobs {
        let task = job.get("task").and_then(Json::as_str);
        let state = job.get("state").and_then(Json::as_str);
        let attempts = job.get("attempts").and_then(Json::as_i64);
        let run_at = job.get("run_at").and_then(Json::as_str);
        if task != Some("embed-target")
            || state != Some("queued")
            || attempts != Some(0)
            || run_at != Some(WIDTH_DUE)
        {
            return Err(format!("{WIDTH_REQUEUE}: {job}").into());
        }
        found.push(
            job.get("job_key")
                .and_then(Json::as_str)
                .ok_or("job_key")?
                .to_owned(),
        );
    }
    found.sort();
    if found != expected {
        return Err(format!("{WIDTH_REQUEUE}: {found:?} vs {expected:?}").into());
    }
    let cleared = format!("embed-ros_messages-{}", message_id(store, "w1").await?);
    if found.iter().any(|key| key == &cleared) {
        return Err(format!("{WIDTH_REQUEUE}: re-queued {cleared}").into());
    }
    Ok(())
}

async fn message_id(store: &SqliteStore, event: &str) -> TestResult<String> {
    let event = event.to_owned();
    let id = store
        .with_conn(move |conn| {
            conn.query_row(
                "SELECT id FROM ros_messages WHERE json_extract(metadata, '$.event_id') = ?1",
                [event],
                |row| row.get::<_, String>(0),
            )
            .map_err(|source| SqliteError::Query {
                operation: "test message id",
                source,
            })
        })
        .await?;
    Ok(id)
}

async fn unembeddable_case() -> TestResult<Phase> {
    let phase = phase_with(FakeProvider::fnv_embeddings(8))?;
    capture_many(
        phase.store(),
        &[
            ("u1", ""),
            ("u2", "???"),
            ("u3", "data:text/plain;base64,aaaa"),
            ("u4", "hello world"),
        ],
        instant("2026-10-10T12:00:00.000Z")?,
    )
    .await?;
    Ok(phase)
}

async fn deployed_case() -> TestResult<Phase> {
    let phase = phase_with(FakeProvider::fnv_embeddings(8))?;
    let start = instant("2026-10-10T12:00:00.000Z")?;
    phase
        .store()
        .capture(common::deploy_batch("cli:embed", "rivet")?, start)
        .await?;
    drain(&phase.embedder, start).await?;
    Ok(phase)
}

fn phase_with(fake: FakeProvider) -> TestResult<Phase> {
    let fresh = common::fresh_store(None)?;
    let fake = Arc::new(fake);
    let embedder = EmbedRunner::attach(&fresh.store, fake.clone(), "fake-embed-8", None)?;
    Ok(Phase {
        fresh,
        embedder,
        fake,
    })
}

async fn outcome_step(phase: &Phase, label: &str, now: DateTime<Utc>) -> TestResult<Json> {
    let store = phase.store().clone();
    let actual = match label {
        "runJobs" | "runJobs (reconcile: model changed → clear vectors)" => {
            Json::from(phase.embedder.run_jobs(now).await?)
        }
        "message embed state" => serde_json::to_value(outcome_rows(&store).await?)?,
        "jobs" => Json::Array(table(&store, "ros_jobs").await?),
        "meta" | "open with another model, workers false (warn only)" => {
            if label.starts_with("open") {
                phase.embedder.configure_model("fake-embed-8-v2").await?;
            }
            serde_json::to_value(meta_rows(&store).await?)?
        }
        "after model change" => serde_json::to_value(AfterModel {
            meta: meta_rows(&store).await?,
            messages: done_rows(&store).await?,
            jobs: table(&store, "ros_jobs").await?,
        })?,
        "drain" => Json::from(drain(&phase.embedder, now).await?),
        "runJobs: 10-wide vector into an 8-wide store fails" => {
            capture(&store, "w1", "width probe sentence", now).await?;
            phase.fake.use_fnv(10).await;
            Json::from(phase.embedder.run_jobs(now).await?)
        }
        "state" if now == instant("2026-10-10T12:11:00.000Z")? => {
            serde_json::to_value(WidthState {
                messages: width_rows(&store).await?,
                jobs: table(&store, "ros_jobs").await?,
            })?
        }
        "runJobs with expectedDims 10: width change clears and re-embeds" => {
            phase.embedder.set_expected_dims(Some(10));
            Json::from(phase.embedder.run_jobs(now).await?)
        }
        "state" => serde_json::to_value(ClearedState {
            meta: meta_rows(&store).await?,
            messages: cleared_rows(&store).await?,
            jobs: table(&store, "ros_jobs").await?,
        })?,
        other => return Err(format!("unhandled outcome step {other}").into()),
    };
    Ok(actual)
}

async fn drain(embedder: &EmbedRunner, now: DateTime<Utc>) -> Result<u64, SqliteError> {
    let mut last = 0_u64;
    for _ in 0..12 {
        last = embedder.run_jobs(now).await?;
        if last == 0 {
            return Ok(0);
        }
    }
    Ok(last)
}

fn find<'a>(file: &'a ScenarioFile, name: &str) -> TestResult<&'a Scenario> {
    file.scenarios
        .iter()
        .find(|scenario| scenario.name == name)
        .ok_or_else(|| format!("missing scenario {name}").into())
}

async fn capture(
    store: &SqliteStore,
    event: &str,
    content: &str,
    now: DateTime<Utc>,
) -> TestResult {
    capture_many(store, &[(event, content)], now).await
}

async fn capture_many(
    store: &SqliteStore,
    messages: &[(&str, &str)],
    now: DateTime<Utc>,
) -> TestResult {
    let mut batch = common::batch("cli:embed", "rivet", "cli");
    for (event, content) in messages {
        batch
            .messages
            .push(common::message(event, Role::User, content));
    }
    store.capture(batch, now).await?;
    Ok(())
}

async fn message_rows(store: &SqliteStore) -> TestResult<Vec<MessageRow>> {
    store
        .with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT embed_status, embed_error, embed_failures, embedding
                       FROM ros_messages ORDER BY rowid",
                )
                .map_err(db("embed rows"))?;
            let mut rows = statement.query([]).map_err(db("embed rows"))?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(db("embed rows"))? {
                let blob: Option<Vec<u8>> = row.get(3).map_err(db("embed rows"))?;
                out.push(MessageRow {
                    embed_status: row.get(0).map_err(db("embed rows"))?,
                    embed_error: row.get(1).map_err(db("embed rows"))?,
                    embed_failures: row.get(2).map_err(db("embed rows"))?,
                    embedding_hex: blob.map(|bytes| hex::encode(bytes).to_ascii_uppercase()),
                });
            }
            Ok(out)
        })
        .await
        .map_err(Into::into)
}

async fn outcome_rows(store: &SqliteStore) -> TestResult<Vec<OutcomeRow>> {
    store.with_conn(read_outcomes).await.map_err(Into::into)
}

fn read_outcomes(conn: &mut rusqlite::Connection) -> Result<Vec<OutcomeRow>, SqliteError> {
    let mut statement = conn
        .prepare(
            "SELECT json_extract(metadata, '$.event_id'), embed_status, embed_error,
                    embed_failures, length(embedding)
               FROM ros_messages ORDER BY rowid",
        )
        .map_err(db("outcome rows"))?;
    let mut rows = statement.query([]).map_err(db("outcome rows"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(db("outcome rows"))? {
        out.push(OutcomeRow {
            event_id: row.get(0).map_err(db("outcome rows"))?,
            embed_status: row.get(1).map_err(db("outcome rows"))?,
            embed_error: row.get(2).map_err(db("outcome rows"))?,
            embed_failures: row.get(3).map_err(db("outcome rows"))?,
            bytes: row.get(4).map_err(db("outcome rows"))?,
        });
    }
    Ok(out)
}

async fn meta_rows(store: &SqliteStore) -> TestResult<Vec<MetaRow>> {
    let model = meta_key(store, "embed_model").await?;
    let dims = meta_key(store, "embed_dims").await?;
    let mut rows = Vec::new();
    if let Some(value) = model {
        rows.push(MetaRow {
            key: "embed_model".to_owned(),
            value,
        });
    }
    if let Some(value) = dims {
        rows.push(MetaRow {
            key: "embed_dims".to_owned(),
            value,
        });
    }
    Ok(rows)
}

async fn meta_key(store: &SqliteStore, key: &str) -> TestResult<Option<String>> {
    let key = key.to_owned();
    store
        .with_conn(move |conn| {
            let value = conn
                .query_row("SELECT value FROM ros_meta WHERE key = ?1", [key], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
                .map_err(db("test meta"))?;
            Ok(value)
        })
        .await
        .map_err(Into::into)
}

async fn done_rows(store: &SqliteStore) -> TestResult<Vec<DoneRow>> {
    store
        .with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT embed_status, length(embedding) FROM ros_messages
                      WHERE embedding IS NOT NULL ORDER BY rowid",
                )
                .map_err(db("done rows"))?;
            let mapped = statement
                .query_map([], |row| {
                    Ok(DoneRow {
                        embed_status: row.get(0)?,
                        bytes: row.get(1)?,
                    })
                })
                .map_err(db("done rows"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db("done rows"))?;
            Ok(mapped)
        })
        .await
        .map_err(Into::into)
}

async fn width_rows(store: &SqliteStore) -> TestResult<Vec<WidthRow>> {
    store
        .with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT json_extract(metadata, '$.event_id'), embed_status, embed_error,
                            embed_failures, length(embedding)
                       FROM ros_messages ORDER BY rowid",
                )
                .map_err(db("width rows"))?;
            let mapped = statement
                .query_map([], |row| {
                    Ok(WidthRow {
                        event_id: row.get(0)?,
                        embed_status: row.get(1)?,
                        embed_error: row.get(2)?,
                        embed_failures: row.get(3)?,
                        bytes: row.get(4)?,
                    })
                })
                .map_err(db("width rows"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db("width rows"))?;
            Ok(mapped)
        })
        .await
        .map_err(Into::into)
}

async fn cleared_rows(store: &SqliteStore) -> TestResult<Vec<ClearedRow>> {
    store
        .with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT json_extract(metadata, '$.event_id'), embed_status, embed_error,
                            length(embedding)
                       FROM ros_messages ORDER BY rowid",
                )
                .map_err(db("cleared rows"))?;
            let mapped = statement
                .query_map([], |row| {
                    Ok(ClearedRow {
                        event_id: row.get(0)?,
                        embed_status: row.get(1)?,
                        embed_error: row.get(2)?,
                        bytes: row.get(3)?,
                    })
                })
                .map_err(db("cleared rows"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db("cleared rows"))?;
            Ok(mapped)
        })
        .await
        .map_err(Into::into)
}

fn db(operation: &'static str) -> impl Fn(rusqlite::Error) -> SqliteError {
    move |source| SqliteError::Query { operation, source }
}

#[tokio::test]
async fn model_change_keeps_jobs_for_rows_without_vectors() -> TestResult {
    let phase = phase_with(FakeProvider::fnv_embeddings(8))?;
    let now = instant("2026-10-10T12:00:00.000Z")?;
    capture(phase.store(), "m1", SENTENCE, now).await?;
    drain(&phase.embedder, now).await?;
    phase
        .store()
        .enqueue_job(
            memory_sqlite::JobInsert::new(
                "embed-target",
                r#"{"targetTable":"ros_messages","targetId":"pending"}"#.to_owned(),
                now,
            )
            .key("embed-ros_messages-pending")
            .run_at(now + chrono::Duration::hours(1)),
        )
        .await?;
    phase.embedder.configure_model("fake-embed-8-v2").await?;
    phase.embedder.run_jobs(now).await?;
    let rows = table(phase.store(), "ros_jobs").await?;
    let Some(row) = rows.first() else {
        return Err(format!("model change removed an unembedded job: {rows:?}").into());
    };
    if rows.len() != 1 {
        return Err(format!("model change removed an unembedded job: {rows:?}").into());
    }
    if row.get("job_key").and_then(Json::as_str) != Some("embed-ros_messages-pending") {
        return Err(format!("model change rewrote the wrong job: {row}").into());
    }
    Ok(())
}
