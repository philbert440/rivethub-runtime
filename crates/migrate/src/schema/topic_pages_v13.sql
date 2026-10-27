CREATE TABLE IF NOT EXISTS rh_topic_pages (
    topic_id TEXT PRIMARY KEY REFERENCES rh_topics (id) ON DELETE CASCADE,
    page_slug TEXT NOT NULL UNIQUE REFERENCES ros_wiki_topics (slug) ON DELETE CASCADE
);

INSERT OR IGNORE INTO rh_topic_pages (topic_id, page_slug)
WITH candidates AS (
    SELECT t.id AS topic_id, w.slug AS page_slug, 0 AS rank
      FROM rh_topics t JOIN ros_wiki_topics w ON w.slug = t.slug
     WHERE t.parent_id IS NULL
    UNION ALL
    SELECT t.id, w.slug, 1
      FROM rh_topics t
      JOIN rh_topic_aliases a ON a.topic_id = t.id
      JOIN ros_wiki_topics w ON w.slug = a.alias_key
     WHERE t.parent_id IS NULL
    UNION ALL
    SELECT t.id, w.slug, 2
      FROM rh_topics t
      JOIN rh_topic_redirects r ON r.topic_id = t.id
      JOIN ros_wiki_topics w ON w.slug = r.slug
     WHERE t.parent_id IS NULL
), per_topic AS (
    SELECT topic_id, page_slug, rank,
           row_number() OVER (PARTITION BY topic_id ORDER BY rank, page_slug) AS pick
      FROM candidates
), per_page AS (
    SELECT topic_id, page_slug,
           row_number() OVER (PARTITION BY page_slug ORDER BY rank, topic_id) AS pick
      FROM per_topic
     WHERE pick = 1
)
SELECT topic_id, page_slug FROM per_page WHERE pick = 1;
