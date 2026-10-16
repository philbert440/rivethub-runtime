pub(crate) const CLAIM: &str = "WITH selected AS MATERIALIZED (
    SELECT j.id, j.job_queue_id FROM graphile_worker._private_jobs j
    JOIN graphile_worker._private_tasks t ON t.id = j.task_id
    WHERE t.identifier = ANY($1)
      AND ($2::bigint IS NULL OR j.id = $2)
      AND j.attempts < j.max_attempts
      AND (j.locked_at IS NULL OR j.locked_at < now() - interval '4 hours')
      AND j.run_at <= now()
      AND (j.job_queue_id IS NULL OR EXISTS (
          SELECT 1 FROM graphile_worker._private_job_queues q
          WHERE q.id = j.job_queue_id
            AND (q.locked_at IS NULL OR q.locked_at < now() - interval '4 hours')
          FOR UPDATE SKIP LOCKED
      ))
    ORDER BY j.priority, j.run_at, j.id
    LIMIT 1 FOR UPDATE OF j SKIP LOCKED
), locked_queue AS (
    UPDATE graphile_worker._private_job_queues q
    SET locked_at = now(), locked_by = $3
    FROM selected s WHERE q.id = s.job_queue_id
    RETURNING q.id
)
UPDATE graphile_worker._private_jobs j
SET attempts = j.attempts + 1, locked_at = now(), locked_by = $3, updated_at = now()
FROM selected s, graphile_worker._private_tasks t
WHERE j.id = s.id AND t.id = j.task_id
  AND (s.job_queue_id IS NULL OR s.job_queue_id IN (SELECT id FROM locked_queue))
RETURNING j.id, t.identifier, j.payload, j.attempts::bigint";

pub(crate) const COMPLETE: &str = "WITH removed AS (
    DELETE FROM graphile_worker._private_jobs
    WHERE id = $1 AND starts_with(locked_by, $2)
    RETURNING job_queue_id, locked_by
), unlocked AS (
    UPDATE graphile_worker._private_job_queues q
    SET locked_at = NULL, locked_by = NULL
    FROM removed r WHERE q.id = r.job_queue_id AND q.locked_by = r.locked_by
)
SELECT count(*)::bigint AS removed FROM removed";

pub(crate) const RELEASE: &str = "WITH owned AS MATERIALIZED (
    SELECT id, job_queue_id, locked_by FROM graphile_worker._private_jobs
    WHERE id = $1 AND starts_with(locked_by, $2)
    FOR UPDATE
), released AS (
    UPDATE graphile_worker._private_jobs j
    SET attempts = GREATEST(j.attempts - 1, 0),
        locked_at = NULL, locked_by = NULL, updated_at = now()
    FROM owned WHERE j.id = owned.id
    RETURNING owned.job_queue_id, owned.locked_by
), unlocked AS (
    UPDATE graphile_worker._private_job_queues q
    SET locked_at = NULL, locked_by = NULL
    FROM released r WHERE q.id = r.job_queue_id AND q.locked_by = r.locked_by
)
SELECT count(*)::bigint AS released FROM released";

pub(crate) const FAIL: &str = "WITH owned AS MATERIALIZED (
    SELECT id, job_queue_id, locked_by FROM graphile_worker._private_jobs
    WHERE id = $1 AND starts_with(locked_by, $3)
    FOR UPDATE
), failed AS (
    UPDATE graphile_worker._private_jobs j
    SET last_error = $2, locked_at = NULL, locked_by = NULL, updated_at = now(),
        run_at = greatest(now(), j.run_at) + interval '1 second' * exp(least(j.attempts, 10))
    FROM owned WHERE j.id = owned.id
    RETURNING owned.job_queue_id, owned.locked_by
), unlocked AS (
    UPDATE graphile_worker._private_job_queues q
    SET locked_at = NULL, locked_by = NULL
    FROM failed f WHERE q.id = f.job_queue_id AND q.locked_by = f.locked_by
)
SELECT count(*)::bigint AS failed FROM failed";
