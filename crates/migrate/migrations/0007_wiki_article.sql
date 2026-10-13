DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0007_wiki_article.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE IF EXISTS ros_wiki_topics ADD COLUMN IF NOT EXISTS article TEXT NOT NULL DEFAULT '';
    ALTER TABLE IF EXISTS ros_wiki_topics ADD COLUMN IF NOT EXISTS related TEXT[] NOT NULL DEFAULT '{}';
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_topics_related
        ON ros_wiki_topics USING gin (related);

    INSERT INTO _rivetos_migrations (name) VALUES ('0007_wiki_article.sql');
END
$rivet$;
