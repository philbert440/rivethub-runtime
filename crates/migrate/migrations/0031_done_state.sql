DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0031_done_state.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE rh_topics ADD COLUMN IF NOT EXISTS closed_at TIMESTAMPTZ;

    CREATE TABLE IF NOT EXISTS rh_session_history (
        session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
        marked_at TIMESTAMPTZ NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_rh_session_history_marked ON rh_session_history (marked_at DESC);

    INSERT INTO _rivetos_migrations (name) VALUES ('0031_done_state.sql');
END
$rivet$;
