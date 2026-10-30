DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0033_conversation_filing.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_conversation_placements (
        conversation_id UUID PRIMARY KEY REFERENCES rh_conversations (id) ON DELETE CASCADE,
        topic_id UUID NOT NULL REFERENCES rh_topics (id) ON DELETE CASCADE,
        placed_by TEXT NOT NULL CHECK (placed_by IN ('you', 'categorizer')),
        placed_at TIMESTAMPTZ NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_rh_conversation_placements_topic
        ON rh_conversation_placements (topic_id);

    CREATE TABLE IF NOT EXISTS rh_filing_corrections (
        id UUID PRIMARY KEY,
        session_id TEXT CHECK (session_id <> ''),
        conversation_id UUID REFERENCES rh_conversations (id) ON DELETE CASCADE,
        from_topic UUID NOT NULL REFERENCES rh_topics (id) ON DELETE CASCADE,
        to_topic UUID REFERENCES rh_topics (id) ON DELETE CASCADE,
        corrected_at TIMESTAMPTZ NOT NULL,
        CHECK ((session_id IS NULL) <> (conversation_id IS NULL))
    );
    CREATE INDEX IF NOT EXISTS idx_rh_filing_corrections_at
        ON rh_filing_corrections (corrected_at DESC);

    ALTER TABLE rh_session_suggestions
        ADD COLUMN IF NOT EXISTS parent_id UUID REFERENCES rh_topics (id) ON DELETE SET NULL;

    INSERT INTO _rivetos_migrations (name) VALUES ('0033_conversation_filing.sql');
END
$rivet$;
