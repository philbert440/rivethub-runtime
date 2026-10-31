CREATE TABLE IF NOT EXISTS rh_session_keeps (
    session_key TEXT PRIMARY KEY CHECK (session_key <> ''),
    kept_at TEXT NOT NULL,
    kept_by TEXT
);
