use sqlx::PgConnection;
use sqlx::Row;
use uuid::Uuid;

use kernel::queue::JobKeyMode;
use kernel::tasks::ClaimSelector;
use kernel::tasks::ClaimedJob;
use kernel::tasks::NewJob;
use kernel::tasks::StoredJob;
use kernel::tasks::TaskError;

pub(crate) fn map_sql(operation: &'static str, source: sqlx::Error) -> TaskError {
    if let Some(database) = source.as_database_error()
        && database
            .code()
            .as_deref()
            .is_some_and(|code| code == "42883" || code == "42P01" || code == "3F000")
    {
        return TaskError::QueueAbsent;
    }
    TaskError::Operation {
        operation,
        source: Box::new(source),
    }
}

pub(crate) async fn enqueue_job(conn: &mut PgConnection, job: &NewJob) -> Result<i64, TaskError> {
    let payload = sqlx::types::Json(&job.payload);
    let row = sqlx::query(
        "SELECT id FROM graphile_worker.add_job($1, $2::json, null::text, null::timestamptz, $3, $4, $5, null::text[], $6)",
    )
    .bind(job.name.to_string())
    .bind(payload)
    .bind(job.max_attempts)
    .bind(&job.key)
    .bind(i32::from(job.priority))
    .bind(key_mode_text(job.key_mode))
    .fetch_one(&mut *conn)
    .await
    .map_err(|source| map_sql("enqueue", source))?;
    cell(&row, "enqueue", "id")
}

pub(crate) async fn claim_job(
    conn: &mut PgConnection,
    selector: &ClaimSelector,
    worker: &str,
) -> Result<Option<ClaimedJob>, TaskError> {
    let locked_by = format!("{worker}-{}", Uuid::new_v4());
    let row = sqlx::query(crate::graphile_sql::CLAIM)
        .bind(&selector.names)
        .bind(selector.only_id)
        .bind(locked_by)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| map_sql("claim", source))?;
    row.as_ref().map(claimed).transpose()
}

pub(crate) async fn complete_job(
    conn: &mut PgConnection,
    id: i64,
    worker: &str,
) -> Result<(), TaskError> {
    let row = sqlx::query(crate::graphile_sql::COMPLETE)
        .bind(id)
        .bind(format!("{worker}-"))
        .fetch_one(&mut *conn)
        .await
        .map_err(|source| map_sql("complete", source))?;
    let removed: i64 = cell(&row, "complete", "removed")?;
    if removed == 0 {
        return Err(TaskError::MissingJob { id });
    }
    Ok(())
}

pub(crate) async fn fail_job(
    conn: &mut PgConnection,
    id: i64,
    error: &str,
    worker: &str,
) -> Result<(), TaskError> {
    let failed: i64 = sqlx::query_scalar(crate::graphile_sql::FAIL)
        .bind(id)
        .bind(error)
        .bind(format!("{worker}-"))
        .fetch_one(&mut *conn)
        .await
        .map_err(|source| map_sql("fail", source))?;
    if failed == 0 {
        return Err(TaskError::MissingJob { id });
    }
    Ok(())
}

pub(crate) async fn read_job(
    conn: &mut PgConnection,
    id: i64,
) -> Result<Option<StoredJob>, TaskError> {
    let row = sqlx::query(
        "SELECT jobs.id, known.identifier, jobs.payload::text AS payload, jobs.key,
                jobs.priority::bigint AS priority, jobs.max_attempts::bigint AS max_attempts,
                jobs.attempts::bigint AS attempts, jobs.last_error
           FROM graphile_worker._private_jobs AS jobs
           JOIN graphile_worker._private_tasks AS known ON known.id = jobs.task_id
          WHERE jobs.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|source| map_sql("read job", source))?;
    row.as_ref().map(stored).transpose()
}

fn claimed(row: &sqlx::postgres::PgRow) -> Result<ClaimedJob, TaskError> {
    let attempts: i64 = cell(row, "claim", "attempts")?;
    Ok(ClaimedJob {
        id: cell(row, "claim", "id")?,
        identifier: cell(row, "claim", "identifier")?,
        payload_text: serde_json::to_string(
            &cell::<sqlx::types::Json<wire::JsonObject>>(row, "claim", "payload")?.0,
        )
        .map_err(|source| TaskError::Payload(Box::new(source)))?,
        attempt: narrow(attempts, "attempt")?,
    })
}

fn stored(row: &sqlx::postgres::PgRow) -> Result<StoredJob, TaskError> {
    let payload: String = cell(row, "read job", "payload")?;
    let priority: i64 = cell(row, "read job", "priority")?;
    let max_attempts: i64 = cell(row, "read job", "max_attempts")?;
    let attempts: i64 = cell(row, "read job", "attempts")?;
    Ok(StoredJob {
        id: cell(row, "read job", "id")?,
        identifier: cell(row, "read job", "identifier")?,
        payload: serde_json::from_str(&payload)
            .map_err(|source| TaskError::Payload(Box::new(source)))?,
        key: cell(row, "read job", "key")?,
        priority: narrow(priority, "priority")?,
        max_attempts: narrow(max_attempts, "max_attempts")?,
        attempts: narrow(attempts, "attempts")?,
        last_error: cell(row, "read job", "last_error")?,
    })
}

fn narrow<T>(value: i64, field: &'static str) -> Result<T, TaskError>
where
    T: TryFrom<i64>,
{
    T::try_from(value).map_err(|_| TaskError::Stored(field.to_string()))
}

pub(crate) fn cell<'a, T>(
    row: &'a sqlx::postgres::PgRow,
    operation: &'static str,
    name: &'static str,
) -> Result<T, TaskError>
where
    T: sqlx::Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(name)
        .map_err(|source| map_sql(operation, source))
}

fn key_mode_text(mode: JobKeyMode) -> &'static str {
    match mode {
        JobKeyMode::KeepRunAt => "preserve_run_at",
        JobKeyMode::Replace => "replace",
    }
}
