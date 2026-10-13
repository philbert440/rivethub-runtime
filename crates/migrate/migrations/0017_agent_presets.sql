DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0017_agent_presets.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS ros_agent_presets (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        color TEXT NOT NULL DEFAULT '',
        harness_id TEXT,
        model TEXT NOT NULL DEFAULT '',
        effort TEXT NOT NULL DEFAULT '',
        system_prompt TEXT NOT NULL DEFAULT '',
        node TEXT NOT NULL,
        directory TEXT NOT NULL,
        shared_link BOOLEAN NOT NULL DEFAULT true,
        node_base_url TEXT NOT NULL DEFAULT '',
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );
    CREATE UNIQUE INDEX IF NOT EXISTS idx_ros_agent_presets_name
        ON ros_agent_presets (lower(name));
    CREATE INDEX IF NOT EXISTS idx_ros_agent_presets_node
        ON ros_agent_presets (node);

    INSERT INTO _rivetos_migrations (name) VALUES ('0017_agent_presets.sql');
END
$rivet$;
