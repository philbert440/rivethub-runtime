DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0018_agent_preset_sort_order.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE IF EXISTS ros_agent_presets ADD COLUMN IF NOT EXISTS sort_order INTEGER;

    INSERT INTO _rivetos_migrations (name) VALUES ('0018_agent_preset_sort_order.sql');
END
$rivet$;
