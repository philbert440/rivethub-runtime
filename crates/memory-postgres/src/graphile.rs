use crate::error::Error;
use chrono::DateTime;
use chrono::Utc;
use kernel::queue::ClaimedJob;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use sqlx::PgPool;
use std::str::FromStr;
use wire::JsonObject;

#[derive(Debug, sqlx::FromRow)]
struct QueuedJob {
    id: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct RemovedJobs {
    removed: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct DeadJob {
    id: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct RescheduledJobs {
    rescheduled: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct ReapedJobs {
    count: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct HealthRow {
    total: i64,
    pending: i64,
    running: i64,
    scheduled: i64,
    dead: i64,
    recent_dead: i64,
    oldest_pending: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ClaimedRow {
    id: i64,
    identifier: String,
    payload: sqlx::types::Json<JsonObject>,
    attempts: i32,
}

#[derive(Debug)]
pub struct GraphileQueue {
    pool: PgPool,
    worker: String,
}

impl GraphileQueue {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self {
            pool,
            worker: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub(crate) async fn schema_present(pool: &PgPool) -> Result<bool, Error> {
        let record = sqlx::query!(
            "select to_regclass('graphile_worker._private_jobs') is not null
              and to_regprocedure('graphile_worker.add_job(text,json,text,timestamptz,int,text,int,text[],text)')
                is not null as \"present!\""
        )
        .fetch_one(pool)
        .await
        .map_err(|source| Error::operation("queue probe", source))?;
        Ok(record.present)
    }
    pub(crate) async fn enqueue(&self, job: &Job) -> Result<JobId, Error> {
        let payload = serde_json::to_string(&job.payload)
            .map_err(|source| Error::protocol(source.to_string()))?;
        let queued: QueuedJob = sqlx::query_as(
            "select (graphile_worker.add_job($1, $2::json, null, $3, $4, $5, $6, null::text[], $7)).id",
        )
        .bind(job.name.to_string())
        .bind(payload)
        .bind(job.run_at)
        .bind(max_attempts(&job.name))
        .bind(job.key.clone())
        .bind(i32::from(job.priority))
        .bind(key_mode(job.key_mode))
        .fetch_one(&self.pool)
        .await
        .map_err(|source| Error::operation("enqueue", source))?;
        Ok(JobId::new(queued.id))
    }

    pub(crate) async fn claim(&self, tasks: &[JobName]) -> Result<Option<ClaimedJob>, Error> {
        let tasks: Vec<String> = tasks.iter().map(ToString::to_string).collect();
        let locked_by = format!("{}-{}", self.worker, uuid::Uuid::new_v4());
        let row: Option<ClaimedRow> = sqlx::query_as(
            "with selected as (
                select j.id from graphile_worker._private_jobs j
                join graphile_worker._private_tasks t on t.id = j.task_id
                where t.identifier = any($1)
                  and j.attempts < j.max_attempts
                  and (j.locked_at is null or j.locked_at < now() - interval '4 hours')
                  and j.run_at <= now()
                order by j.priority asc, j.run_at asc, j.id asc
                limit 1
                for update skip locked
             )
             update graphile_worker._private_jobs j
                set attempts = j.attempts + 1,
                    locked_at = now(),
                    locked_by = $2,
                    updated_at = now()
               from selected s, graphile_worker._private_tasks t
              where j.id = s.id and t.id = j.task_id
              returning j.id, t.identifier, j.payload, j.attempts::integer",
        )
        .bind(&tasks)
        .bind(&locked_by)
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| Error::operation("claim", source))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let name = JobName::from_str(&row.identifier).map_err(|job_error| {
            Error::protocol(format!(
                "queued task {} is not a kernel job name: {job_error}",
                row.identifier
            ))
        })?;
        let attempt =
            u32::try_from(row.attempts).map_err(|source| Error::protocol(source.to_string()))?;
        Ok(Some(ClaimedJob {
            id: JobId::new(row.id),
            name,
            payload: row.payload.0,
            attempt,
        }))
    }

    pub(crate) async fn complete(&self, id: JobId) -> Result<u64, Error> {
        let removed: RemovedJobs = sqlx::query_as(
            "with removed as (delete from graphile_worker._private_jobs
             where id = $1 and starts_with(locked_by, $2) returning id)
             select count(*)::bigint as removed from removed",
        )
        .bind(i64::from(id))
        .bind(format!("{}-", self.worker))
        .fetch_one(&self.pool)
        .await
        .map_err(|source| Error::operation("complete", source))?;
        count(removed.removed)
    }

    pub(crate) async fn fail(&self, id: JobId, error: &str) -> Result<(), Error> {
        sqlx::query(
            "update graphile_worker._private_jobs
                set last_error = $2,
                    locked_at = null,
                    locked_by = null,
                    updated_at = now(),
                    run_at = case when attempts < max_attempts
                                  then now() + ('1 second'::interval
                                        * least(power(2, attempts)::int, 3600))
                                  else run_at end
              where id = $1",
        )
        .bind(i64::from(id))
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(|source| Error::operation("fail", source))?;
        Ok(())
    }

    pub(crate) async fn dead_job_ids(
        &self,
        task: &JobName,
        limit: u32,
    ) -> Result<Vec<JobId>, Error> {
        let rows: Vec<DeadJob> = sqlx::query_as(
            "select j.id from graphile_worker._private_jobs j
             join graphile_worker._private_tasks t on t.id = j.task_id
             where t.identifier = $1 and j.attempts >= j.max_attempts
               and j.key is not null
               and j.updated_at < now() - interval '24 hours'
               and (j.locked_at is null or j.locked_at < now() - interval '4 hours')
             order by j.run_at asc limit $2",
        )
        .bind(task.to_string())
        .bind(i64::from(limit.min(200)))
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("dead jobs", source))?;
        Ok(rows.into_iter().map(|job| JobId::new(job.id)).collect())
    }

    pub(crate) async fn reschedule_dead(
        &self,
        task: &JobName,
        limit: u32,
        priority: i16,
    ) -> Result<u64, Error> {
        let ids = self.dead_job_ids(task, limit).await?;
        if ids.is_empty() {
            return Ok(0);
        }
        let raw: Vec<i64> = ids.iter().map(|id| i64::from(*id)).collect();
        let rescheduled: RescheduledJobs = sqlx::query_as(
            "select count(*)::bigint as rescheduled from graphile_worker.reschedule_jobs($1::bigint[], now(), $2, 0)",
        )
        .bind(&raw)
        .bind(i32::from(priority))
        .fetch_one(&self.pool)
        .await
        .map_err(|source| Error::operation("reschedule dead", source))?;
        count(rescheduled.rescheduled)
    }

    pub(crate) async fn revive_before(
        &self,
        task: &JobName,
        dead_before: DateTime<Utc>,
    ) -> Result<u64, Error> {
        let rows: Vec<DeadJob> = sqlx::query_as(
            "select j.id from graphile_worker._private_jobs j
             join graphile_worker._private_tasks t on t.id = j.task_id
             where t.identifier = $1 and j.attempts >= j.max_attempts
               and j.key is not null
               and j.updated_at < $2
               and (j.locked_at is null or j.locked_at < now() - interval '4 hours')
             order by j.run_at asc limit 200",
        )
        .bind(task.to_string())
        .bind(dead_before)
        .fetch_all(&self.pool)
        .await
        .map_err(|source| Error::operation("revive dead jobs", source))?;
        if rows.is_empty() {
            return Ok(0);
        }
        let raw: Vec<i64> = rows.iter().map(|job| job.id).collect();
        let rescheduled: RescheduledJobs = sqlx::query_as(
            "select count(*)::bigint as rescheduled from graphile_worker.reschedule_jobs($1::bigint[], now(), 0, 0)",
        )
        .bind(&raw)
        .fetch_one(&self.pool)
        .await
        .map_err(|source| Error::operation("revive dead jobs", source))?;
        count(rescheduled.rescheduled)
    }

    pub(crate) async fn reap_dead_corpses(
        &self,
        allowlist: &[JobName],
        limit: u32,
    ) -> Result<u64, Error> {
        let allowlist: Vec<String> = allowlist.iter().map(ToString::to_string).collect();
        let reaped: ReapedJobs = sqlx::query_as(
            "with deleted as (
                delete from graphile_worker._private_jobs
                 where ctid in (
                   select ctid from graphile_worker._private_jobs j
                    where j.key is null
                      and j.attempts >= j.max_attempts
                      and j.locked_at is null
                      and j.updated_at < now() - interval '7 days'
                      and j.task_id in (select id from graphile_worker._private_tasks where identifier = any($1::text[]))
                    order by j.updated_at asc
                    limit $2
                 )
                 returning 1
             ) select count(*)::bigint as count from deleted",
        )
        .bind(&allowlist)
        .bind(i64::from(limit.min(200)))
        .fetch_one(&self.pool)
        .await
        .map_err(|source| Error::operation("reap dead jobs", source))?;
        count(reaped.count)
    }

    pub(crate) async fn queue_health(
        &self,
        task: &JobName,
    ) -> Result<Option<crate::queue::QueueHealth>, Error> {
        let Some(row) = sqlx::query_as::<_, HealthRow>(
            "select
                count(*)::bigint as total,
                count(*) filter (where j.attempts < j.max_attempts
                                  and (j.locked_at is null or j.locked_at < now() - interval '4 hours')
                                  and j.run_at <= now())::bigint as pending,
                count(*) filter (where j.locked_at >= now() - interval '4 hours')::bigint as running,
                count(*) filter (where j.locked_at is null and j.run_at > now()
                                  and j.attempts < j.max_attempts)::bigint as scheduled,
                count(*) filter (where j.attempts >= j.max_attempts)::bigint as dead,
                count(*) filter (where j.attempts >= j.max_attempts
                                  and j.created_at > now() - interval '24 hours')::bigint as recent_dead,
                min(j.run_at) filter (where j.attempts < j.max_attempts
                                  and (j.locked_at is null or j.locked_at < now() - interval '4 hours')
                                  and j.run_at <= now()) as oldest_pending,
                (select left(j2.last_error, 120) from graphile_worker._private_jobs j2
                  join graphile_worker._private_tasks t2 on t2.id = j2.task_id
                  where t2.identifier = $1 and j2.attempts >= j2.max_attempts
                  order by j2.updated_at desc limit 1) as last_error
             from graphile_worker._private_jobs j
             join graphile_worker._private_tasks t on t.id = j.task_id
             where t.identifier = $1",
        )
        .bind(task.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| Error::operation("queue health", source))?
        else {
            return Ok(None);
        };
        if row.total == 0 {
            return Ok(None);
        }
        let oldest_pending_minutes = match row.oldest_pending {
            Some(at) => Some(
                u64::try_from((Utc::now() - at).num_minutes().max(0))
                    .map_err(|source| Error::protocol(source.to_string()))?,
            ),
            None => None,
        };
        Ok(Some(crate::queue::QueueHealth {
            task: task.clone(),
            pending: count(row.pending)?,
            running: count(row.running)?,
            scheduled: count(row.scheduled)?,
            dead: count(row.dead)?,
            recent_dead: count(row.recent_dead)?,
            oldest_pending_minutes,
            last_error: row.last_error,
        }))
    }
}

fn count(value: i64) -> Result<u64, Error> {
    u64::try_from(value.max(0)).map_err(|source| Error::protocol(source.to_string()))
}

fn key_mode(mode: JobKeyMode) -> &'static str {
    match mode {
        JobKeyMode::KeepRunAt => "preserve_run_at",
        JobKeyMode::Replace => "replace",
    }
}

fn max_attempts(name: &JobName) -> i32 {
    match name {
        JobName::Fixed(fixed) => match fixed {
            FixedJob::EmbedTarget => 5,
            FixedJob::CompactConversation | FixedJob::SynthesizeToolCall => 3,
            FixedJob::SuggestTags | FixedJob::ExtractWiki => 2,
            FixedJob::RunTask => 1,
            FixedJob::EnqueueUnembedded
            | FixedJob::EnqueueUnchunked
            | FixedJob::EnqueueIdle
            | FixedJob::EnqueueWikiBackfill
            | FixedJob::ConsolidateWiki
            | FixedJob::RecompileWiki
            | FixedJob::EnqueueStaleWiki
            | FixedJob::EnqueueStaleCompaction
            | FixedJob::ReapDeadJobs => 25,
        },
        JobName::RunTaskOn { .. } | JobName::Heartbeat { .. } => 1,
    }
}
