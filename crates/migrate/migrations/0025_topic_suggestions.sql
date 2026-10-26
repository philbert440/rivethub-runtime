DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0025_topic_suggestions.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_session_suggestions (
        session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
        name TEXT NOT NULL CHECK (name <> ''),
        topic_id UUID REFERENCES rh_topics (id)
    );
    CREATE INDEX IF NOT EXISTS idx_rh_session_suggestions_topic
        ON rh_session_suggestions (topic_id) WHERE topic_id IS NOT NULL;

    INSERT INTO _rivetos_migrations (name) VALUES ('0025_topic_suggestions.sql');
END
$rivet$;
