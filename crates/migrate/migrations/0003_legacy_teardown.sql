DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0003_legacy_teardown.sql') THEN
            RETURN;
        END IF;
    END IF;

    IF to_regclass('ros_subagent_sessions') IS NOT NULL THEN
        INSERT INTO ros_tasks (goal, agent_id, origin, status, completed_at, result)
        SELECT
            'subagent session ' || s.id::text,
            s.agent,
            'chat',
            'completed',
            s.updated_at,
            jsonb_build_object('archived', true)
        FROM ros_subagent_sessions s
        ON CONFLICT (id) DO NOTHING;
        DROP TABLE IF EXISTS ros_subagent_sessions;
    END IF;

    IF to_regclass('ros_delegation_runs') IS NOT NULL THEN
        ALTER TABLE IF EXISTS ros_delegation_runs RENAME TO ros_delegation_runs_archive;
    END IF;

    INSERT INTO _rivetos_migrations (name) VALUES ('0003_legacy_teardown.sql');
END
$rivet$;
