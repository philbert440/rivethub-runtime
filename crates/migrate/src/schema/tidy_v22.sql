CREATE TABLE IF NOT EXISTS rh_tidy_batches (
    id TEXT PRIMARY KEY,
    proposed_by TEXT NOT NULL CHECK (proposed_by IN ('categorizer', 'backfill')),
    proposed_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS rh_tidy_batches_by ON rh_tidy_batches (proposed_by, proposed_at);

CREATE TABLE IF NOT EXISTS rh_tidy_proposals (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES rh_tidy_batches (id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    fingerprint TEXT NOT NULL CHECK (fingerprint <> ''),
    change TEXT NOT NULL,
    reason TEXT NOT NULL CHECK (reason <> ''),
    evidence TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'approved', 'rejected')),
    decided_at TEXT,
    UNIQUE (batch_id, position),
    CHECK ((state = 'pending') = (decided_at IS NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS rh_tidy_proposals_remembered
    ON rh_tidy_proposals (fingerprint) WHERE state IN ('pending', 'rejected');
CREATE INDEX IF NOT EXISTS rh_tidy_proposals_pending
    ON rh_tidy_proposals (batch_id) WHERE state = 'pending';

CREATE TABLE IF NOT EXISTS rh_worker_runs (
    task TEXT PRIMARY KEY CHECK (task <> ''),
    finished_at TEXT NOT NULL
);
