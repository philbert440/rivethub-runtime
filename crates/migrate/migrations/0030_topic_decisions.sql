DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0030_topic_decisions.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_topic_decisions (
        id UUID PRIMARY KEY,
        topic_id UUID NOT NULL REFERENCES rh_topics (id),
        text TEXT NOT NULL CHECK (text <> ''),
        status TEXT NOT NULL CHECK (status IN ('active', 'superseded')),
        superseded_by UUID,
        decided_by TEXT NOT NULL CHECK (decided_by <> ''),
        session_id TEXT CHECK (session_id <> ''),
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL,
        CHECK (superseded_by IS NULL OR status = 'superseded')
    );
    CREATE INDEX IF NOT EXISTS idx_rh_topic_decisions_topic ON rh_topic_decisions (topic_id, created_at);

    INSERT INTO _rivetos_migrations (name) VALUES ('0030_topic_decisions.sql');
END
$rivet$;
