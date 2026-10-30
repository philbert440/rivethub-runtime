DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0040_tasks_sessions.sql') THEN
            RETURN;
        END IF;
    END IF;

    DROP VIEW IF EXISTS ros_task_outcomes_v;
    DROP TRIGGER IF EXISTS trg_task_done ON rh_tasks;
    DROP FUNCTION IF EXISTS notify_task_done();
    ALTER TABLE rh_tasks RENAME TO rh_tasks_archive;
    UPDATE rh_tasks_archive
       SET status = 'killed',
           completed_at = COALESCE(completed_at, now()),
           error = COALESCE(error, 'archived with the tasks refactor')
     WHERE status IN ('queued', 'running', 'awaiting-input');

    IF to_regclass('graphile_worker._private_jobs') IS NOT NULL THEN
        DELETE FROM graphile_worker._private_jobs j
         USING graphile_worker._private_tasks t
         WHERE t.id = j.task_id
           AND (t.identifier = 'run-task'
                OR t.identifier LIKE 'run-task:%'
                OR t.identifier LIKE 'heartbeat-%');
    END IF;

    CREATE TABLE rh_tasks (
        id UUID PRIMARY KEY,
        goal TEXT NOT NULL CHECK (goal <> ''),
        acceptance_criteria JSONB NOT NULL DEFAULT '[]',
        budget JSONB NOT NULL DEFAULT '{}',
        status TEXT NOT NULL CHECK (status IN ('queued','running','awaiting-input','completed','failed','killed','timeout')),
        result JSONB,
        error TEXT,
        usage JSONB,
        agent TEXT NOT NULL CHECK (agent <> ''),
        preset TEXT,
        node TEXT NOT NULL CHECK (node <> ''),
        harness TEXT,
        model TEXT,
        effort TEXT,
        cwd TEXT,
        permission_mode TEXT,
        parent_session_id TEXT,
        parent_task_id UUID REFERENCES rh_tasks (id) ON DELETE SET NULL,
        parent_source TEXT CHECK (parent_source IN ('launch','request','capture')),
        depth SMALLINT NOT NULL DEFAULT 0 CHECK (depth BETWEEN 0 AND 3),
        child_session_id TEXT UNIQUE,
        requested_by TEXT,
        owner_user_id TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL,
        started_at TIMESTAMPTZ,
        completed_at TIMESTAMPTZ,
        alive_at TIMESTAMPTZ
    );
    CREATE INDEX idx_rh_tasks_parent_session ON rh_tasks (parent_session_id)
        WHERE parent_session_id IS NOT NULL;
    CREATE INDEX idx_rh_tasks_open ON rh_tasks (status, node)
        WHERE status IN ('queued','running','awaiting-input');
    CREATE INDEX idx_rh_tasks_created ON rh_tasks (created_at DESC);

    CREATE OR REPLACE FUNCTION rh_notify_task_changed() RETURNS trigger AS $func$
    BEGIN
        IF TG_OP = 'INSERT' OR NEW.status IS DISTINCT FROM OLD.status THEN
            PERFORM pg_notify('rh_task_changed', NEW.id::text);
        END IF;
        RETURN NEW;
    END;
    $func$ LANGUAGE plpgsql;

    CREATE TRIGGER trg_task_changed
        AFTER INSERT OR UPDATE ON rh_tasks
        FOR EACH ROW EXECUTE FUNCTION rh_notify_task_changed();

    INSERT INTO _rivetos_migrations (name) VALUES ('0040_tasks_sessions.sql');
END
$rivet$;
