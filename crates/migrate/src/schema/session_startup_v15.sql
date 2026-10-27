CREATE TABLE IF NOT EXISTS rh_session_startup (
    session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
    topic_id TEXT NOT NULL,
    tokens INTEGER NOT NULL CHECK (tokens >= 0),
    sections TEXT NOT NULL CHECK (json_type(sections) = 'array'),
    created_at TEXT NOT NULL
);
