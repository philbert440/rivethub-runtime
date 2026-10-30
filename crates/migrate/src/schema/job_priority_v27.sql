CREATE TABLE IF NOT EXISTS rh_job_priorities (
    job_id TEXT PRIMARY KEY NOT NULL REFERENCES ros_jobs (id) ON DELETE CASCADE,
    priority INTEGER NOT NULL CHECK (priority <> 0)
);
CREATE INDEX IF NOT EXISTS rh_job_priorities_rank ON rh_job_priorities (priority);
