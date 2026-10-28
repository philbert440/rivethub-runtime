CREATE TABLE rh_topic_pages_v17 (
    page_slug TEXT NOT NULL PRIMARY KEY REFERENCES ros_wiki_topics (slug) ON DELETE CASCADE,
    topic_id TEXT UNIQUE REFERENCES rh_topics (id) ON DELETE SET NULL,
    sections TEXT NOT NULL DEFAULT '[]'
);
INSERT INTO rh_topic_pages_v17 (page_slug, topic_id, sections)
    SELECT page_slug, topic_id, sections FROM rh_topic_pages;
DROP TABLE rh_topic_pages;
ALTER TABLE rh_topic_pages_v17 RENAME TO rh_topic_pages;
