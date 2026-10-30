ALTER TABLE rh_session_suggestions ADD COLUMN parent_id TEXT REFERENCES rh_topics (id) ON DELETE SET NULL;
