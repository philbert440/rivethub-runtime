DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0047_flagged_jobs.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_job_failures (
        job_key TEXT PRIMARY KEY CHECK (job_key <> ''),
        failures INTEGER NOT NULL CHECK (failures > 0),
        first_failed_at TIMESTAMPTZ NOT NULL,
        last_failed_at TIMESTAMPTZ NOT NULL
    );

    CREATE TABLE IF NOT EXISTS rh_flagged_jobs (
        id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        task TEXT NOT NULL CHECK (task <> ''),
        job_key TEXT UNIQUE,
        payload JSONB NOT NULL,
        priority SMALLINT NOT NULL,
        max_attempts SMALLINT NOT NULL,
        failures INTEGER NOT NULL CHECK (failures > 0),
        last_error TEXT NOT NULL,
        first_failed_at TIMESTAMPTZ NOT NULL,
        last_failed_at TIMESTAMPTZ NOT NULL,
        flagged_at TIMESTAMPTZ NOT NULL,
        dismissed_at TIMESTAMPTZ
    );
    CREATE INDEX IF NOT EXISTS idx_rh_flagged_jobs_open
        ON rh_flagged_jobs (last_failed_at DESC) WHERE dismissed_at IS NULL;

    INSERT INTO _rivetos_migrations (name) VALUES ('0047_flagged_jobs.sql');
END
$rivet$;
