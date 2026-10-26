DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0024_topics.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_topics (
        id UUID PRIMARY KEY,
        slug TEXT NOT NULL CHECK (slug <> ''),
        name TEXT NOT NULL CHECK (name <> ''),
        parent_id UUID REFERENCES rh_topics (id),
        directory_kind TEXT CHECK (directory_kind IN ('local', 'shared')),
        directory_node TEXT,
        directory_path TEXT,
        created_at TIMESTAMPTZ NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL,
        CHECK (parent_id IS NULL OR parent_id <> id),
        CHECK (
            (directory_kind IS NULL AND directory_node IS NULL AND directory_path IS NULL)
            OR (directory_kind = 'local' AND directory_node IS NOT NULL AND directory_path IS NOT NULL)
            OR (directory_kind = 'shared' AND directory_node IS NULL AND directory_path IS NOT NULL)
        )
    );
    CREATE UNIQUE INDEX IF NOT EXISTS ux_rh_topics_slug ON rh_topics (slug);
    CREATE INDEX IF NOT EXISTS idx_rh_topics_parent ON rh_topics (parent_id) WHERE parent_id IS NOT NULL;

    CREATE TABLE IF NOT EXISTS rh_topic_aliases (
        topic_id UUID NOT NULL REFERENCES rh_topics (id) ON DELETE CASCADE,
        alias TEXT NOT NULL CHECK (alias <> ''),
        alias_key TEXT NOT NULL CHECK (alias_key <> '')
    );
    CREATE UNIQUE INDEX IF NOT EXISTS ux_rh_topic_aliases_key ON rh_topic_aliases (alias_key);
    CREATE INDEX IF NOT EXISTS idx_rh_topic_aliases_topic ON rh_topic_aliases (topic_id);

    CREATE TABLE IF NOT EXISTS rh_topic_redirects (
        slug TEXT PRIMARY KEY CHECK (slug <> ''),
        topic_id UUID NOT NULL REFERENCES rh_topics (id) ON DELETE CASCADE,
        created_at TIMESTAMPTZ NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_rh_topic_redirects_topic ON rh_topic_redirects (topic_id);

    CREATE TABLE IF NOT EXISTS rh_session_placements (
        session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
        topic_id UUID NOT NULL REFERENCES rh_topics (id),
        placed_by TEXT NOT NULL CHECK (placed_by IN ('you', 'categorizer')),
        placed_at TIMESTAMPTZ NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_rh_session_placements_topic ON rh_session_placements (topic_id);

    INSERT INTO _rivetos_migrations (name) VALUES ('0024_topics.sql');
END
$rivet$;
