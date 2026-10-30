CREATE TABLE IF NOT EXISTS rh_conversation_placements (
    conversation_id TEXT PRIMARY KEY REFERENCES ros_conversations (id) ON DELETE CASCADE,
    topic_id TEXT NOT NULL REFERENCES rh_topics (id) ON DELETE CASCADE,
    placed_by TEXT NOT NULL CHECK (placed_by IN ('you', 'categorizer')),
    placed_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS rh_conversation_placements_topic ON rh_conversation_placements (topic_id);

CREATE TABLE IF NOT EXISTS rh_filing_corrections (
    id TEXT PRIMARY KEY,
    session_id TEXT CHECK (session_id <> ''),
    conversation_id TEXT REFERENCES ros_conversations (id) ON DELETE CASCADE,
    from_topic TEXT NOT NULL REFERENCES rh_topics (id) ON DELETE CASCADE,
    to_topic TEXT REFERENCES rh_topics (id) ON DELETE CASCADE,
    corrected_at TEXT NOT NULL,
    CHECK ((session_id IS NULL) <> (conversation_id IS NULL))
);
CREATE INDEX IF NOT EXISTS rh_filing_corrections_at ON rh_filing_corrections (corrected_at);
