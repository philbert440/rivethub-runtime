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
    let payload = serde_json::to_string(&job.payload)
        .map_err(|source| TaskError::Payload(Box::new(source)))?;
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
) -> Result<Option<ClaimedJob>, TaskError> {
    let locked_by = format!("tasks-{}", Uuid::new_v4());
    let row = sqlx::query(
        "WITH selected AS (
            SELECT jobs.id FROM graphile_worker._private_jobs AS jobs
            JOIN graphile_worker._private_tasks AS known ON known.id = jobs.task_id
            WHERE known.identifier = ANY($1)
              AND jobs.attempts < jobs.max_attempts
              AND (jobs.locked_at IS NULL OR jobs.locked_at < now() - interval '4 hours')
              AND jobs.run_at <= now()
              AND ($2::bigint IS NULL OR jobs.id = $2)
            ORDER BY jobs.priority ASC, jobs.run_at ASC, jobs.id ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
         )
         UPDATE graphile_worker._private_jobs AS jobs
            SET attempts = jobs.attempts + 1, locked_at = now(), locked_by = $3, updated_at = now()
           FROM selected, graphile_worker._private_tasks AS known
          WHERE jobs.id = selected.id AND known.id = jobs.task_id
          RETURNING jobs.id, known.identifier, jobs.payload::text AS payload,
                    jobs.attempts::bigint AS attempts",
    )
    .bind(&selector.names)
    .bind(selector.only_id)
    .bind(locked_by)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|source| map_sql("claim", source))?;
    row.as_ref().map(claimed).transpose()
}

pub(crate) async fn complete_job(conn: &mut PgConnection, id: i64) -> Result<(), TaskError> {
    let row = sqlx::query(
        "WITH unlocked AS (
            UPDATE graphile_worker._private_jobs
               SET locked_at = NULL, locked_by = NULL, updated_at = now()
             WHERE id = $1
            RETURNING id
         )
         SELECT count(*)::bigint AS removed
           FROM graphile_worker.complete_jobs(
               COALESCE((SELECT array_agg(id) FROM unlocked), ARRAY[]::bigint[])
           )",
    )
    .bind(id)
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
) -> Result<(), TaskError> {
    let result = sqlx::query(
        "UPDATE graphile_worker._private_jobs
            SET last_error = $2, locked_at = null, locked_by = null, updated_at = now(),
                run_at = CASE WHEN attempts < max_attempts
                              THEN now() + ('1 second'::interval * least(power(2, attempts)::int, 3600))
                              ELSE run_at END
          WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("fail", source))?;
    if result.rows_affected() == 0 {
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
        payload_text: cell(row, "claim", "payload")?,
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
