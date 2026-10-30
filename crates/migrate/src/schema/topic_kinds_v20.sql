ALTER TABLE rh_topics ADD COLUMN kind TEXT NOT NULL DEFAULT 'work'
    CHECK (kind IN ('work', 'type', 'entity'));
ALTER TABLE rh_topic_aliases ADD COLUMN kind TEXT NOT NULL DEFAULT 'work'
    CHECK (kind IN ('work', 'type', 'entity'));
