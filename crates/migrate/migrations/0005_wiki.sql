DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0005_wiki.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS ros_wiki_topics (
        slug TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        aliases TEXT[] NOT NULL DEFAULT '{}',
        tags TEXT[] NOT NULL DEFAULT '{}',
        entities TEXT[] NOT NULL DEFAULT '{}',
        current_state TEXT NOT NULL DEFAULT '',
        search_text TEXT NOT NULL DEFAULT '',
        content_tsv tsvector GENERATED ALWAYS AS (to_tsvector('english', search_text)) STORED,
        embedding halfvec(1024),
        embed_status TEXT,
        embed_failures INTEGER DEFAULT 0,
        embed_error TEXT,
        history_count INTEGER NOT NULL DEFAULT 0,
        git_sha TEXT,
        last_verified_at TIMESTAMPTZ,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_topics_fts
        ON ros_wiki_topics USING gin (content_tsv);
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_topics_trgm
        ON ros_wiki_topics USING gin (search_text gin_trgm_ops);
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_topics_aliases
        ON ros_wiki_topics USING gin (aliases);
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_topics_entities
        ON ros_wiki_topics USING gin (entities);
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_topics_updated
        ON ros_wiki_topics (updated_at DESC);
    CREATE INDEX IF NOT EXISTS ros_wiki_topics_embedding_hnsw
        ON ros_wiki_topics USING hnsw (embedding halfvec_cosine_ops)
        WITH (m = 32, ef_construction = 120);

    CREATE TABLE IF NOT EXISTS ros_wiki_provenance (
        topic_slug TEXT NOT NULL REFERENCES ros_wiki_topics(slug) ON DELETE CASCADE,
        source_kind TEXT NOT NULL CHECK (source_kind IN ('summary','message','conversation','task')),
        source_id UUID NOT NULL,
        conversation_id UUID,
        git_sha TEXT,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (topic_slug, source_kind, source_id)
    );
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_provenance_source
        ON ros_wiki_provenance (source_kind, source_id);

    CREATE TABLE IF NOT EXISTS ros_wiki_extractions (
        summary_id UUID PRIMARY KEY REFERENCES ros_summaries(id) ON DELETE CASCADE,
        status TEXT NOT NULL CHECK (status IN ('done','skipped','failed')),
        pipeline_version INTEGER NOT NULL,
        topics_touched TEXT[] NOT NULL DEFAULT '{}',
        git_sha TEXT,
        error TEXT,
        extracted_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );
    CREATE INDEX IF NOT EXISTS idx_ros_wiki_extractions_status
        ON ros_wiki_extractions (status, extracted_at DESC);

    INSERT INTO _rivetos_migrations (name) VALUES ('0005_wiki.sql');
END
$rivet$;
