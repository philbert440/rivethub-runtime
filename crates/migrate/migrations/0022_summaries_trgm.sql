DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0022_summaries_trgm.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE INDEX IF NOT EXISTS idx_ros_summaries_trgm
        ON rh_summaries USING gin (content gin_trgm_ops);

    INSERT INTO _rivetos_migrations (name) VALUES ('0022_summaries_trgm.sql');
END
$rivet$;
