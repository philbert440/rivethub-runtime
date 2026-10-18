use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use chrono::DateTime;
use chrono::NaiveDate;
use chrono::Utc;
use kernel::tasks::TaskError;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskRecord;
use serde::Deserialize;
use wire::gateway::catalog::OutcomeBucket;
use wire::gateway::catalog::OutcomesFilter;
use wire::gateway::catalog::OutcomesResponse;
use wire::task::EvalVerdict;
use wire::task::TaskStatus;
use wire::task::TaskVerdict;

use crate::extract::Query;
use crate::extract::parse_enum;

use super::TasksApi;
use super::error::TaskHttpError;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct OutcomesQuery {
    since: Option<String>,
    until: Option<String>,
    agent_id: Option<String>,
    origin: Option<String>,
}

struct Bounds {
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    origin: Option<TaskOrigin>,
}

struct Tally {
    tasks: u64,
    completed: u64,
    failed: u64,
    verified: u64,
    refuted: u64,
    escalated: u64,
    diverged: u64,
    cost_usd: Option<f64>,
    duration_sum: u128,
    duration_count: u64,
}

pub(super) fn routes() -> Router<Arc<TasksApi>> {
    Router::new()
        .route("/", get(outcomes).fallback(super::method_not_allowed))
        .fallback(super::method_not_allowed)
}

async fn outcomes(
    State(api): State<Arc<TasksApi>>,
    Query(query): Query<OutcomesQuery>,
) -> Result<Json<OutcomesResponse>, TaskHttpError> {
    let bounds = bounds_of(&query)?;
    let records = api
        .query
        .list(&TaskListFilter::default())
        .await
        .map_err(|error| hidden(&error))?;
    let rolled = rollup(&records, &query, &bounds)?;
    Ok(Json(OutcomesResponse {
        filter: OutcomesFilter {
            since: bounds.since,
            until: bounds.until,
            agent_id: query.agent_id,
            origin: bounds.origin.map(|origin| origin.to_string()),
        },
        totals: bucket(&rolled.totals)?,
        by_agent: buckets(&rolled.by_agent)?,
        by_executor: buckets(&rolled.by_executor)?,
        by_day: buckets(&rolled.by_day)?,
    }))
}

struct Rolled {
    totals: Tally,
    by_agent: BTreeMap<String, Tally>,
    by_executor: BTreeMap<String, Tally>,
    by_day: BTreeMap<String, Tally>,
}

fn bounds_of(query: &OutcomesQuery) -> Result<Bounds, TaskHttpError> {
    Ok(Bounds {
        since: bound(query.since.as_deref(), "since")?,
        until: bound(query.until.as_deref(), "until")?,
        origin: origin_of(query.origin.as_deref())?,
    })
}

fn bound(raw: Option<&str>, field: &'static str) -> Result<Option<DateTime<Utc>>, TaskHttpError> {
    match raw {
        None => Ok(None),
        Some(text) => Ok(Some(parse_bound(text, field)?)),
    }
}

fn parse_bound(raw: &str, field: &'static str) -> Result<DateTime<Utc>, TaskHttpError> {
    if looks_like_epoch(raw) {
        return epoch_bound(raw, field);
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return Ok(parsed.with_timezone(&Utc));
    }
    calendar_day(raw, field)
}

fn looks_like_epoch(raw: &str) -> bool {
    let digits = match raw.strip_prefix('-') {
        Some(rest) => rest,
        None => raw,
    };
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn epoch_bound(raw: &str, field: &'static str) -> Result<DateTime<Utc>, TaskHttpError> {
    let Ok(millis) = raw.parse::<i64>() else {
        return Err(invalid_bound(field));
    };
    match DateTime::from_timestamp_millis(millis) {
        Some(instant) => Ok(instant),
        None => Err(invalid_bound(field)),
    }
}

fn calendar_day(raw: &str, field: &'static str) -> Result<DateTime<Utc>, TaskHttpError> {
    let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") else {
        return Err(invalid_bound(field));
    };
    let Some(naive) = date.and_hms_opt(0, 0, 0) else {
        return Err(invalid_bound(field));
    };
    match naive.and_local_timezone(Utc).single() {
        Some(instant) => Ok(instant),
        None => Err(invalid_bound(field)),
    }
}

fn invalid_bound(field: &'static str) -> TaskHttpError {
    TaskHttpError::BadRequest(format!("invalid {field}"))
}

fn origin_of(raw: Option<&str>) -> Result<Option<TaskOrigin>, TaskHttpError> {
    match raw {
        None => Ok(None),
        Some(text) => match parse_enum::<TaskOrigin>(text) {
            Some(origin) => Ok(Some(origin)),
            None => Err(TaskHttpError::BadRequest(format!(
                "unknown origin \"{text}\""
            ))),
        },
    }
}

fn rollup(
    records: &[TaskRecord],
    query: &OutcomesQuery,
    bounds: &Bounds,
) -> Result<Rolled, TaskHttpError> {
    let mut rolled = Rolled {
        totals: Tally::new(),
        by_agent: BTreeMap::new(),
        by_executor: BTreeMap::new(),
        by_day: BTreeMap::new(),
    };
    for task in records {
        if included(task, query, bounds) {
            add_task(&mut rolled, task)?;
        }
    }
    Ok(rolled)
}

fn included(task: &TaskRecord, query: &OutcomesQuery, bounds: &Bounds) -> bool {
    if !kernel::tasks::terminal(task.status) || task.origin == TaskOrigin::Eval {
        return false;
    }
    if task.acceptance_criteria.is_empty() || task.spec.audit_only == Some(true) {
        return false;
    }
    if let Some(agent) = &query.agent_id
        && task.agent_id != *agent
    {
        return false;
    }
    if let Some(origin) = bounds.origin
        && task.origin != origin
    {
        return false;
    }
    match task.completed_at {
        Some(at) => within(at, bounds),
        None => bounds.since.is_none() && bounds.until.is_none(),
    }
}

fn within(at: DateTime<Utc>, bounds: &Bounds) -> bool {
    if let Some(since) = bounds.since
        && at < since
    {
        return false;
    }
    if let Some(until) = bounds.until
        && at > until
    {
        return false;
    }
    true
}

fn add_task(rolled: &mut Rolled, task: &TaskRecord) -> Result<(), TaskHttpError> {
    rolled.totals.add(task)?;
    rolled
        .by_agent
        .entry(task.agent_id.clone())
        .or_insert_with(Tally::new)
        .add(task)?;
    rolled
        .by_executor
        .entry(task.executor.to_string())
        .or_insert_with(Tally::new)
        .add(task)?;
    if let Some(at) = task.completed_at {
        rolled
            .by_day
            .entry(at.format("%Y-%m-%d").to_string())
            .or_insert_with(Tally::new)
            .add(task)?;
    }
    Ok(())
}

fn buckets(
    map: &BTreeMap<String, Tally>,
) -> Result<BTreeMap<String, OutcomeBucket>, TaskHttpError> {
    let mut out = BTreeMap::new();
    for (key, tally) in map {
        out.insert(key.clone(), bucket(tally)?);
    }
    Ok(out)
}

fn bucket(tally: &Tally) -> Result<OutcomeBucket, TaskHttpError> {
    Ok(OutcomeBucket {
        tasks: tally.tasks,
        completed: tally.completed,
        failed: tally.failed,
        verified: tally.verified,
        refuted: tally.refuted,
        escalated: tally.escalated,
        diverged: tally.diverged,
        divergence_rate: rate(tally.diverged, tally.tasks)?,
        total_cost_usd: tally.cost_usd,
        avg_duration_ms: average(tally.duration_sum, tally.duration_count)?,
    })
}

fn rate(diverged: u64, tasks: u64) -> Result<f64, TaskHttpError> {
    match tasks {
        0 => Ok(0.0),
        count => Ok(exact_f64(diverged)? / exact_f64(count)?),
    }
}

fn exact_f64(value: u64) -> Result<f64, TaskHttpError> {
    const EXACT: u64 = 1_u64 << 53;
    if value > EXACT {
        return Err(TaskHttpError::Failed(
            "outcome count exceeds exact f64".to_owned(),
        ));
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "counts above 2^53 are rejected before this conversion"
    )]
    Ok(value as f64)
}

fn average(sum: u128, count: u64) -> Result<Option<u64>, TaskHttpError> {
    if count == 0 {
        return Ok(None);
    }
    let mean = sum / u128::from(count);
    match u64::try_from(mean) {
        Ok(value) => Ok(Some(value)),
        Err(_) => Err(TaskHttpError::Failed(
            "stored duration is too large".to_owned(),
        )),
    }
}

fn hidden(error: &TaskError) -> TaskHttpError {
    match error {
        TaskError::Unavailable(_) => TaskHttpError::Unavailable(error.to_string()),
        TaskError::UnavailableCatalog(_)
        | TaskError::Operation { .. }
        | TaskError::QueueAbsent
        | TaskError::Cleanup { .. }
        | TaskError::Payload(_)
        | TaskError::Stored(_)
        | TaskError::Missing { .. }
        | TaskError::MissingJob { .. }
        | TaskError::Conflict { .. }
        | TaskError::Clock => {
            tracing::error!(%error, "task outcomes failed");
            TaskHttpError::Internal
        }
    }
}

impl Tally {
    fn new() -> Self {
        Self {
            tasks: 0,
            completed: 0,
            failed: 0,
            verified: 0,
            refuted: 0,
            escalated: 0,
            diverged: 0,
            cost_usd: None,
            duration_sum: 0,
            duration_count: 0,
        }
    }

    fn add(&mut self, task: &TaskRecord) -> Result<(), TaskHttpError> {
        self.tasks = inc(self.tasks)?;
        if task.status == TaskStatus::Completed {
            self.completed = inc(self.completed)?;
        }
        if task.status == TaskStatus::Failed {
            self.failed = inc(self.failed)?;
        }
        match task.eval.as_ref().map(|eval| eval.verdict) {
            Some(EvalVerdict::Verified) => self.verified = inc(self.verified)?,
            Some(EvalVerdict::Refuted) => self.refuted = inc(self.refuted)?,
            Some(EvalVerdict::Escalated) => self.escalated = inc(self.escalated)?,
            None => {}
        }
        if diverged(task) {
            self.diverged = inc(self.diverged)?;
        }
        if let Some(cost) = task.usage.and_then(|usage| usage.cost_usd) {
            self.cost_usd = Some(match self.cost_usd {
                Some(total) => total + cost,
                None => cost,
            });
        }
        if let Some(duration) = task.duration_ms {
            self.add_duration(duration)?;
        }
        Ok(())
    }

    fn add_duration(&mut self, duration: i32) -> Result<(), TaskHttpError> {
        let Ok(widened) = u64::try_from(duration) else {
            return Err(TaskHttpError::Failed(
                "stored duration is negative".to_owned(),
            ));
        };
        self.duration_sum = match self.duration_sum.checked_add(u128::from(widened)) {
            Some(value) => value,
            None => {
                return Err(TaskHttpError::Failed(
                    "stored duration is too large".to_owned(),
                ));
            }
        };
        self.duration_count = inc(self.duration_count)?;
        Ok(())
    }
}

fn inc(value: u64) -> Result<u64, TaskHttpError> {
    match value.checked_add(1) {
        Some(next) => Ok(next),
        None => Err(TaskHttpError::Failed("outcome count overflow".to_owned())),
    }
}

fn diverged(task: &TaskRecord) -> bool {
    let completed = matches!(
        task.result.as_ref().map(|result| result.verdict),
        Some(TaskVerdict::Completed)
    );
    let refuted = matches!(
        task.eval.as_ref().map(|eval| eval.verdict),
        Some(EvalVerdict::Refuted)
    );
    completed && refuted
}
