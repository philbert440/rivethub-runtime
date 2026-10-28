DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0029_startup_delivery.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE rh_session_startup
        ADD COLUMN IF NOT EXISTS delivered_via TEXT NOT NULL DEFAULT 'none'
        CHECK (delivered_via IN ('metaAppend', 'cliFlag', 'none'));

    INSERT INTO _rivetos_migrations (name) VALUES ('0029_startup_delivery.sql');
END
$rivet$;
