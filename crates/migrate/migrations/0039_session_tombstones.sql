DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0039_session_tombstones.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_session_tombstones (
        session_key TEXT PRIMARY KEY CHECK (session_key <> ''),
        deleted_at TIMESTAMPTZ NOT NULL,
        deleted_by TEXT
    );

    INSERT INTO _rivetos_migrations (name) VALUES ('0039_session_tombstones.sql');
END
$rivet$;
