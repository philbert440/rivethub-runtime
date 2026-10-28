CREATE TABLE IF NOT EXISTS rh_topic_decisions (
    id TEXT PRIMARY KEY,
    topic_id TEXT NOT NULL REFERENCES rh_topics (id),
    text TEXT NOT NULL CHECK (text <> ''),
    status TEXT NOT NULL CHECK (status IN ('active', 'superseded')),
    superseded_by TEXT,
    decided_by TEXT NOT NULL CHECK (decided_by <> ''),
    session_id TEXT CHECK (session_id <> ''),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (superseded_by IS NULL OR status = 'superseded')
);
CREATE INDEX IF NOT EXISTS rh_topic_decisions_topic ON rh_topic_decisions (topic_id, created_at);
