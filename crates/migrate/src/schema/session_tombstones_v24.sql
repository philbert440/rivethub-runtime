CREATE TABLE IF NOT EXISTS rh_session_tombstones (
    session_key TEXT PRIMARY KEY CHECK (session_key <> ''),
    deleted_at TEXT NOT NULL,
    deleted_by TEXT
);
