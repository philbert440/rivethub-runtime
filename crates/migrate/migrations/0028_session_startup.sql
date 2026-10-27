DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0028_session_startup.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_session_startup (
        session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
        topic_id UUID NOT NULL,
        tokens INTEGER NOT NULL CHECK (tokens >= 0),
        sections JSONB NOT NULL CHECK (jsonb_typeof(sections) = 'array'),
        created_at TIMESTAMPTZ NOT NULL
    );

    INSERT INTO _rivetos_migrations (name) VALUES ('0028_session_startup.sql');
END
$rivet$;
