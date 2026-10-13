DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0004_task_eval.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE IF EXISTS ros_tasks ADD COLUMN IF NOT EXISTS eval JSONB;
    ALTER TABLE IF EXISTS ros_tasks ADD COLUMN IF NOT EXISTS eval_attempt INTEGER NOT NULL DEFAULT 0;

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'ros_tasks_origin_check') THEN
        ALTER TABLE ros_tasks DROP CONSTRAINT ros_tasks_origin_check;
    END IF;
    ALTER TABLE ros_tasks ADD CONSTRAINT ros_tasks_origin_check
        CHECK (origin IN ('heartbeat','chat','tool','mesh','api','eval'));

    CREATE OR REPLACE VIEW ros_task_outcomes_v AS
    SELECT
        t.id,
        t.agent_id,
        t.executor,
        t.executor_target,
        t.origin,
        date_trunc('day', t.completed_at AT TIME ZONE 'UTC') AS day,
        t.status,
        t.result->>'verdict'                    AS executor_verdict,
        t.eval->>'verdict'                      AS eval_verdict,
        COALESCE((t.result->>'verdict') = 'completed'
                 AND (t.eval->>'verdict') = 'refuted', false) AS diverged,
        (t.usage->>'costUsd')::numeric          AS cost_usd,
        t.eval_attempt,
        t.duration_ms
    FROM ros_tasks t
    WHERE t.status IN ('completed','failed','killed','timeout')
      AND t.origin <> 'eval'
      AND (t.spec->>'auditOnly') IS DISTINCT FROM 'true'
      AND jsonb_array_length(t.acceptance_criteria) > 0;

    INSERT INTO _rivetos_migrations (name) VALUES ('0004_task_eval.sql');
END
$rivet$;
