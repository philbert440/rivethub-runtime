CREATE TABLE IF NOT EXISTS rh_session_suggestions (
    session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    topic_id TEXT REFERENCES rh_topics (id)
);
CREATE INDEX IF NOT EXISTS rh_session_suggestions_topic
    ON rh_session_suggestions (topic_id) WHERE topic_id IS NOT NULL;
