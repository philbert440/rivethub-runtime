CREATE TABLE IF NOT EXISTS rh_tasks (
    id TEXT PRIMARY KEY,
    goal TEXT NOT NULL CHECK (goal <> ''),
    acceptance_criteria TEXT NOT NULL DEFAULT '[]',
    budget TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL CHECK (status IN ('queued','running','awaiting-input','completed','failed','killed','timeout')),
    result TEXT,
    error TEXT,
    usage TEXT,
    agent TEXT NOT NULL CHECK (agent <> ''),
    preset TEXT,
    node TEXT NOT NULL CHECK (node <> ''),
    harness TEXT,
    model TEXT,
    effort TEXT,
    cwd TEXT,
    permission_mode TEXT,
    parent_session_id TEXT,
    parent_task_id TEXT REFERENCES rh_tasks (id) ON DELETE SET NULL,
    parent_source TEXT CHECK (parent_source IN ('launch','request','capture')),
    depth INTEGER NOT NULL DEFAULT 0 CHECK (depth BETWEEN 0 AND 3),
    child_session_id TEXT UNIQUE,
    requested_by TEXT,
    owner_user_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    alive_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_rh_tasks_parent_session ON rh_tasks (parent_session_id)
    WHERE parent_session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_rh_tasks_open ON rh_tasks (status, node)
    WHERE status IN ('queued','running','awaiting-input');
CREATE INDEX IF NOT EXISTS idx_rh_tasks_created ON rh_tasks (created_at DESC);
