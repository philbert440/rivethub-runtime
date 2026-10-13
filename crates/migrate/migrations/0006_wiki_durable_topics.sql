DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0006_wiki_durable_topics.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS ros_wiki_citations (
        topic_slug TEXT NOT NULL REFERENCES ros_wiki_topics(slug) ON DELETE CASCADE,
        summary_id UUID NOT NULL,
        kind TEXT,
        note TEXT,
        cited_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (topic_slug, summary_id)
    );
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_citations_summary
        ON ros_wiki_citations (summary_id);

    CREATE TABLE IF NOT EXISTS ros_wiki_redirects (
        from_slug TEXT PRIMARY KEY,
        to_slug TEXT NOT NULL REFERENCES ros_wiki_topics(slug) ON DELETE CASCADE,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_redirects_to
        ON ros_wiki_redirects (to_slug);

    INSERT INTO _rivetos_migrations (name) VALUES ('0006_wiki_durable_topics.sql');
END
$rivet$;
