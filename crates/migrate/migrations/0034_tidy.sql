DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0034_tidy.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_tidy_batches (
        id UUID PRIMARY KEY,
        proposed_by TEXT NOT NULL CHECK (proposed_by IN ('categorizer', 'backfill')),
        proposed_at TIMESTAMPTZ NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_rh_tidy_batches_by
        ON rh_tidy_batches (proposed_by, proposed_at DESC);

    CREATE TABLE IF NOT EXISTS rh_tidy_proposals (
        id UUID PRIMARY KEY,
        batch_id UUID NOT NULL REFERENCES rh_tidy_batches (id) ON DELETE CASCADE,
        position INTEGER NOT NULL CHECK (position >= 0),
        fingerprint TEXT NOT NULL CHECK (fingerprint <> ''),
        change JSONB NOT NULL,
        reason TEXT NOT NULL CHECK (reason <> ''),
        evidence JSONB NOT NULL,
        state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'approved', 'rejected')),
        decided_at TIMESTAMPTZ,
        UNIQUE (batch_id, position),
        CHECK ((state = 'pending') = (decided_at IS NULL))
    );
    CREATE UNIQUE INDEX IF NOT EXISTS ux_rh_tidy_proposals_remembered
        ON rh_tidy_proposals (fingerprint) WHERE state IN ('pending', 'rejected');
    CREATE INDEX IF NOT EXISTS idx_rh_tidy_proposals_pending
        ON rh_tidy_proposals (batch_id) WHERE state = 'pending';

    CREATE TABLE IF NOT EXISTS rh_worker_runs (
        task TEXT PRIMARY KEY CHECK (task <> ''),
        finished_at TIMESTAMPTZ NOT NULL
    );

    INSERT INTO _rivetos_migrations (name) VALUES ('0034_tidy.sql');
END
$rivet$;
