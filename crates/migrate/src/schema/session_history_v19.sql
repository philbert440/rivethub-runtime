CREATE TABLE IF NOT EXISTS rh_session_history (
    session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
    marked_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS rh_session_history_marked ON rh_session_history (marked_at);
