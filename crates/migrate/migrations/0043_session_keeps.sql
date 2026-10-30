DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0043_session_keeps.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_session_keeps (
        session_key TEXT PRIMARY KEY CHECK (session_key <> ''),
        kept_at TIMESTAMPTZ NOT NULL,
        kept_by TEXT
    );

    INSERT INTO _rivetos_migrations (name) VALUES ('0043_session_keeps.sql');
END
$rivet$;
