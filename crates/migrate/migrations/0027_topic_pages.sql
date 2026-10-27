DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0027_topic_pages.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE rh_wiki_topics
        ADD COLUMN IF NOT EXISTS topic_id UUID REFERENCES rh_topics (id) ON DELETE SET NULL;
    CREATE UNIQUE INDEX IF NOT EXISTS ux_rh_wiki_topics_topic
        ON rh_wiki_topics (topic_id) WHERE topic_id IS NOT NULL;

    WITH candidates AS (
        SELECT t.id AS topic_id, w.slug AS page_slug, 0 AS rank
          FROM rh_topics t JOIN rh_wiki_topics w ON w.slug = t.slug
         WHERE t.parent_id IS NULL
        UNION ALL
        SELECT t.id, w.slug, 1
          FROM rh_topics t
          JOIN rh_topic_aliases a ON a.topic_id = t.id
          JOIN rh_wiki_topics w ON w.slug = a.alias_key
         WHERE t.parent_id IS NULL
        UNION ALL
        SELECT t.id, w.slug, 2
          FROM rh_topics t
          JOIN rh_topic_redirects r ON r.topic_id = t.id
          JOIN rh_wiki_topics w ON w.slug = r.slug
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
    UPDATE rh_wiki_topics w
       SET topic_id = p.topic_id
      FROM per_page p
     WHERE p.pick = 1 AND w.slug = p.page_slug AND w.topic_id IS NULL;

    INSERT INTO _rivetos_migrations (name) VALUES ('0027_topic_pages.sql');
END
$rivet$;
