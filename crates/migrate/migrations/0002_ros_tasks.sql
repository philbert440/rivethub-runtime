DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0002_ros_tasks.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS ros_tasks (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        goal TEXT NOT NULL,
        context_refs JSONB NOT NULL DEFAULT '[]',
        acceptance_criteria JSONB NOT NULL DEFAULT '[]',
        spec JSONB NOT NULL DEFAULT '{}',
        executor TEXT NOT NULL CHECK (executor IN ('chat-loop','harness-session','mesh')),
        executor_target TEXT,
        requested_by TEXT,
        agent_id TEXT NOT NULL,
        origin TEXT NOT NULL CHECK (origin IN ('heartbeat','chat','tool','mesh','api')),
        parent_task_id UUID REFERENCES ros_tasks(id) ON DELETE SET NULL,
        chain_depth INTEGER NOT NULL DEFAULT 0,
        budget JSONB NOT NULL DEFAULT '{}',
        node_affinity TEXT,
        usage JSONB,
        status TEXT NOT NULL DEFAULT 'queued'
            CHECK (status IN ('queued','running','awaiting-input','completed','failed','killed','timeout')),
        attempt INTEGER NOT NULL DEFAULT 0,
        max_attempts INTEGER NOT NULL DEFAULT 1,
        result JSONB,
        error TEXT,
        conversation_id UUID,
        session_key TEXT,
        pending_message TEXT,
        claimed_by TEXT,
        harness_session_ids JSONB NOT NULL DEFAULT '[]',
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        started_at TIMESTAMPTZ,
        last_heartbeat_at TIMESTAMPTZ,
        completed_at TIMESTAMPTZ,
        duration_ms INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_ros_tasks_active
        ON ros_tasks (status, created_at) WHERE status IN ('queued','running','awaiting-input');
    CREATE INDEX IF NOT EXISTS idx_ros_tasks_agent
        ON ros_tasks (agent_id, created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_ros_tasks_parent
        ON ros_tasks (parent_task_id) WHERE parent_task_id IS NOT NULL;
    CREATE INDEX IF NOT EXISTS idx_ros_tasks_origin
        ON ros_tasks (origin, created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_ros_tasks_node
        ON ros_tasks (node_affinity, status) WHERE node_affinity IS NOT NULL;

    CREATE OR REPLACE FUNCTION notify_task_done() RETURNS trigger AS $func$
    BEGIN
        IF NEW.status IN ('completed','failed','killed','timeout')
           AND OLD.status NOT IN ('completed','failed','killed','timeout') THEN
            PERFORM pg_notify('ros_task_done', NEW.id::text);
        END IF;
        RETURN NEW;
    END;
    $func$ LANGUAGE plpgsql;

    DROP TRIGGER IF EXISTS trg_task_done ON ros_tasks;
    CREATE TRIGGER trg_task_done
        AFTER UPDATE ON ros_tasks
        FOR EACH ROW EXECUTE FUNCTION notify_task_done();

    INSERT INTO _rivetos_migrations (name) VALUES ('0002_ros_tasks.sql');
END
$rivet$;
