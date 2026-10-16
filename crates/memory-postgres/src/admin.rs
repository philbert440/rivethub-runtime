use std::collections::BTreeMap;

use chrono::DateTime;
use chrono::Utc;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use uuid::Uuid;
use wire::JsonObject;

use crate::PostgresMemory;
use crate::error::Error;
use crate::graphile::GraphileQueue;
use crate::project;
use crate::project::ProjectTag;
use crate::queue::QueueHealth;

const QUEUE_TASKS: [FixedJob; 5] = [
    FixedJob::ExtractWiki,
    FixedJob::SummarizeConversation,
    FixedJob::EmbedTarget,
    FixedJob::SynthesizeToolCall,
    FixedJob::SuggestTags,
];

impl PostgresMemory {
    pub async fn retry_failed(
        &self,
        tasks: &[String],
        error: Option<&str>,
        limit: Option<u32>,
        dry_run: bool,
    ) -> Result<u64, Error> {
        self.queue_ready().await?;
        let limit = limit.map(i64::from);
        if dry_run {
            count_rows(&self.pool, RETRY_COUNT, tasks, error, limit, "retry failed").await
        } else {
            let result = sqlx::query(RETRY_UPDATE)
                .bind(tasks)
                .bind(error)
                .bind(limit)
                .execute(&self.pool)
                .await
                .map_err(|source| Error::operation("retry failed", source))?;
            Ok(result.rows_affected())
        }
    }

    pub async fn requeue_failed(
        &self,
        task: &JobName,
        limit: u32,
        dry_run: bool,
    ) -> Result<u64, Error> {
        self.queue_ready().await?;
        if limit == 0 {
            return Err(Error::DumpMessage("requeue limit must be positive".into()));
        }
        let ids = requeue_ids(&self.pool, task, limit.min(1000)).await?;
        if dry_run || ids.is_empty() {
            return Ok(u64::try_from(ids.len())?);
        }
        unlock_stale(&self.pool, &ids).await?;
        reschedule(&self.pool, &ids).await
    }

    pub async fn backfill_tool_synth(
        &self,
        limit: Option<u32>,
        dry_run: bool,
    ) -> Result<u64, Error> {
        self.queue_ready().await?;
        let ids = tool_messages(&self.pool, limit.map(i64::from)).await?;
        if dry_run {
            return Ok(u64::try_from(ids.len())?);
        }
        let queue = GraphileQueue::new(self.pool.clone());
        let mut enqueued = 0u64;
        for id in ids {
            queue.enqueue(&tool_job(id)?).await?;
            enqueued += 1;
        }
        Ok(enqueued)
    }

    pub async fn queue_status(&self) -> Result<Vec<QueueHealth>, Error> {
        let queue = self.queue().await?;
        let mut rows = Vec::with_capacity(QUEUE_TASKS.len());
        for task in QUEUE_TASKS {
            let name = JobName::Fixed(task);
            let health = queue.queue_health(&name).await?;
            rows.push(match health {
                Some(health) => health,
                None => empty_health(name),
            });
        }
        Ok(rows)
    }

    pub async fn backfill_project_rules(&self, since: Option<DateTime<Utc>>) -> Result<u64, Error> {
        let found = directories(&self.pool, since).await?;
        let mut grouped: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
        for row in found {
            grouped.entry(row.cwd).or_default().push(row.id);
        }
        let mut applied = 0u64;
        for (cwd, ids) in grouped {
            let Some(tag) = project::project_rule_tag(Some(cwd), true).await? else {
                continue;
            };
            for id in ids {
                applied += insert_rule(&self.pool, id, &tag).await?;
            }
        }
        Ok(applied)
    }

    async fn queue_ready(&self) -> Result<(), Error> {
        if GraphileQueue::schema_present(&self.pool).await? {
            Ok(())
        } else {
            Err(Error::QueueAbsent)
        }
    }
}

fn empty_health(task: JobName) -> QueueHealth {
    QueueHealth {
        task,
        pending: 0,
        running: 0,
        scheduled: 0,
        dead: 0,
        recent_dead: 0,
        oldest_pending_minutes: None,
        last_error: None,
    }
}

fn tool_job(id: Uuid) -> Result<Job, Error> {
    Ok(Job {
        name: JobName::Fixed(FixedJob::SynthesizeToolCall),
        payload: JsonObject::from_serialize(&kernel::memory::jobs::SynthesizeToolCall {
            message_id: id,
        })?,
        key: Some(format!("tool-synth-{id}")),
        key_mode: JobKeyMode::KeepRunAt,
        priority: 0,
        run_at: None,
        max_attempts: Some(3),
    })
}

async fn count_rows(
    pool: &sqlx::PgPool,
    sql: &'static str,
    tasks: &[String],
    error: Option<&str>,
    limit: Option<i64>,
    operation: &'static str,
) -> Result<u64, Error> {
    let row: CountRow = sqlx::query_as(sql)
        .bind(tasks)
        .bind(error)
        .bind(limit)
        .fetch_one(pool)
        .await
        .map_err(|source| Error::operation(operation, source))?;
    u64::try_from(row.count).map_err(|source| Error::protocol(source.to_string()))
}

async fn requeue_ids(pool: &sqlx::PgPool, task: &JobName, limit: u32) -> Result<Vec<i64>, Error> {
    let rows: Vec<IdRow> = sqlx::query_as(REQUEUE_IDS)
        .bind(task.to_string())
        .bind(i64::from(limit))
        .fetch_all(pool)
        .await
        .map_err(|source| Error::operation("requeue", source))?;
    Ok(rows.into_iter().map(|row| row.id).collect())
}

async fn unlock_stale(pool: &sqlx::PgPool, ids: &[i64]) -> Result<(), Error> {
    sqlx::query(UNLOCK_STALE)
        .bind(ids)
        .execute(pool)
        .await
        .map_err(|source| Error::operation("requeue unlock", source))?;
    Ok(())
}

async fn reschedule(pool: &sqlx::PgPool, ids: &[i64]) -> Result<u64, Error> {
    let row: CountRow = sqlx::query_as(RESCHEDULE)
        .bind(ids)
        .bind(10_i32)
        .fetch_one(pool)
        .await
        .map_err(|source| Error::operation("requeue", source))?;
    u64::try_from(row.count).map_err(|source| Error::protocol(source.to_string()))
}

async fn tool_messages(pool: &sqlx::PgPool, limit: Option<i64>) -> Result<Vec<Uuid>, Error> {
    let rows: Vec<MessageRow> = sqlx::query_as(TOOL_MESSAGES)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|source| Error::operation("tool synth", source))?;
    Ok(rows.into_iter().map(|row| row.id).collect())
}

async fn directories(
    pool: &sqlx::PgPool,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<CwdRow>, Error> {
    sqlx::query_as(DIRECTORIES)
        .bind(since)
        .fetch_all(pool)
        .await
        .map_err(|source| Error::operation("project directories", source))
}

async fn insert_rule(pool: &sqlx::PgPool, id: Uuid, tag: &ProjectTag) -> Result<u64, Error> {
    let result = sqlx::query(INSERT_RULE)
        .bind(id)
        .bind(&tag.key)
        .bind(&tag.value)
        .bind(&tag.display)
        .execute(pool)
        .await
        .map_err(|source| Error::operation("project rule", source))?;
    Ok(result.rows_affected())
}

#[derive(Debug, sqlx::FromRow)]
struct CountRow {
    count: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct IdRow {
    id: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct MessageRow {
    id: Uuid,
}

#[derive(Debug, sqlx::FromRow)]
struct CwdRow {
    id: Uuid,
    cwd: String,
}

const RETRY_COUNT: &str = "\
SELECT count(*)::bigint AS count FROM (
    SELECT j2.id
      FROM graphile_worker._private_jobs j2
      JOIN graphile_worker._private_tasks t ON t.id = j2.task_id
     WHERE t.identifier = ANY($1::text[])
       AND j2.attempts >= j2.max_attempts
       AND ($2::text IS NULL OR strpos(coalesce(j2.last_error, ''), $2) > 0)
     ORDER BY j2.updated_at ASC
     LIMIT $3
) chosen";

const RETRY_UPDATE: &str = "\
WITH chosen AS (
    SELECT j2.id
      FROM graphile_worker._private_jobs j2
      JOIN graphile_worker._private_tasks t ON t.id = j2.task_id
     WHERE t.identifier = ANY($1::text[])
       AND j2.attempts >= j2.max_attempts
       AND ($2::text IS NULL OR strpos(coalesce(j2.last_error, ''), $2) > 0)
     ORDER BY j2.updated_at ASC
     LIMIT $3
)
UPDATE graphile_worker._private_jobs j
   SET attempts = 0,
       last_error = NULL,
       run_at = now(),
       updated_at = now(),
       locked_at = NULL,
       locked_by = NULL
  FROM chosen
 WHERE j.id = chosen.id";

const REQUEUE_IDS: &str = "\
SELECT j.id
  FROM graphile_worker._private_jobs j
  JOIN graphile_worker._private_tasks t ON t.id = j.task_id
 WHERE t.identifier = $1
   AND j.attempts >= j.max_attempts
   AND j.key IS NOT NULL
   AND j.updated_at < now() - interval '24 hours'
   AND (j.locked_at IS NULL OR j.locked_at < now() - interval '4 hours')
 ORDER BY j.run_at ASC
 LIMIT $2";

const UNLOCK_STALE: &str = "\
UPDATE graphile_worker._private_jobs
   SET locked_at = NULL, locked_by = NULL
 WHERE id = ANY($1::bigint[])
   AND locked_at < now() - interval '4 hours'";

const RESCHEDULE: &str = "\
SELECT count(*)::bigint AS count
  FROM graphile_worker.reschedule_jobs($1::bigint[], now(), $2, 0)";

const TOOL_MESSAGES: &str = "\
SELECT m.id
  FROM rh_messages m
 WHERE m.role = 'assistant'
   AND btrim(m.content) = ''
   AND m.tool_name IS NOT NULL
   AND NOT EXISTS (
        SELECT 1
          FROM graphile_worker._private_jobs j
          JOIN graphile_worker._private_tasks t ON t.id = j.task_id
         WHERE t.identifier = 'synthesize-tool-call'
           AND j.key = 'tool-synth-' || m.id::text
   )
 ORDER BY m.created_at ASC
 LIMIT $1";

const DIRECTORIES: &str = "\
SELECT id, settings->>'cwd' AS cwd
  FROM rh_conversations
 WHERE nullif(btrim(settings->>'cwd'), '') IS NOT NULL
   AND ($1::timestamptz IS NULL OR created_at >= $1 OR updated_at >= $1)";

const INSERT_RULE: &str = "\
INSERT INTO rh_tags (entity_type, entity_id, key, value, display, source, state)
VALUES ('conversation', $1, $2, $3, $4, 'rule', 'accepted')
ON CONFLICT (entity_type, entity_id, key, value) DO NOTHING";
