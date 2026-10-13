DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0001_baseline.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS _rivetos_migrations (
        name TEXT PRIMARY KEY,
        applied_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        checksum TEXT
    );

    CREATE EXTENSION IF NOT EXISTS vector;
    CREATE EXTENSION IF NOT EXISTS pg_trgm;

    CREATE TABLE IF NOT EXISTS ros_conversations (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        session_key TEXT NOT NULL,
        agent TEXT NOT NULL,
        channel TEXT NOT NULL DEFAULT 'unknown',
        channel_id TEXT,
        bot_identity TEXT,
        title TEXT,
        settings JSONB DEFAULT '{}'::jsonb,
        active BOOLEAN DEFAULT true,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );
    CREATE INDEX IF NOT EXISTS idx_ros_conversations_session
        ON ros_conversations (session_key, active, updated_at DESC);
    CREATE INDEX IF NOT EXISTS idx_ros_conversations_agent_channel
        ON ros_conversations (agent, channel, updated_at DESC);

    CREATE TABLE IF NOT EXISTS ros_messages (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        conversation_id UUID NOT NULL REFERENCES ros_conversations(id) ON DELETE CASCADE,
        agent TEXT NOT NULL,
        channel TEXT NOT NULL,
        role TEXT NOT NULL CHECK (role IN ('system','user','assistant','tool')),
        content TEXT NOT NULL DEFAULT '',
        tool_name TEXT,
        tool_args JSONB,
        tool_result TEXT,
        metadata JSONB DEFAULT '{}'::jsonb,
        embedding halfvec(4000),
        content_tsv tsvector GENERATED ALWAYS AS (to_tsvector('english', coalesce(content, ''))) STORED,
        access_count INTEGER DEFAULT 0,
        last_accessed_at TIMESTAMPTZ,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        embed_failures INTEGER DEFAULT 0,
        embed_error TEXT,
        embed_status TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_ros_messages_conversation
        ON ros_messages (conversation_id, created_at);
    CREATE INDEX IF NOT EXISTS idx_ros_messages_agent
        ON ros_messages (agent, created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_ros_messages_created
        ON ros_messages (created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_ros_messages_fts
        ON ros_messages USING gin (content_tsv);
    CREATE INDEX IF NOT EXISTS idx_ros_messages_trgm
        ON ros_messages USING gin (content gin_trgm_ops);
    CREATE INDEX IF NOT EXISTS ros_messages_embedding_hnsw
        ON ros_messages USING hnsw (embedding halfvec_cosine_ops)
        WITH (m = 32, ef_construction = 120);

    CREATE TABLE IF NOT EXISTS ros_summaries (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        conversation_id UUID REFERENCES ros_conversations(id),
        parent_id UUID REFERENCES ros_summaries(id),
        depth INTEGER NOT NULL DEFAULT 0,
        content TEXT NOT NULL,
        kind TEXT NOT NULL DEFAULT 'leaf',
        message_count INTEGER NOT NULL DEFAULT 0,
        earliest_at TIMESTAMPTZ,
        latest_at TIMESTAMPTZ,
        embedding halfvec(4000),
        content_tsv tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED,
        model TEXT,
        access_count INTEGER DEFAULT 0,
        last_accessed_at TIMESTAMPTZ,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        embed_failures INTEGER DEFAULT 0,
        embed_error TEXT,
        embed_status TEXT,
        pipeline_version INTEGER NOT NULL DEFAULT 1
    );
    CREATE INDEX IF NOT EXISTS idx_ros_summaries_parent ON ros_summaries (parent_id);
    CREATE INDEX IF NOT EXISTS idx_ros_summaries_time ON ros_summaries (latest_at DESC);
    CREATE INDEX IF NOT EXISTS idx_ros_summaries_fts ON ros_summaries USING gin (content_tsv);
    CREATE INDEX IF NOT EXISTS idx_ros_summaries_pipeline_version
        ON ros_summaries (pipeline_version) WHERE pipeline_version < 5;
    CREATE INDEX IF NOT EXISTS ros_summaries_embedding_hnsw
        ON ros_summaries USING hnsw (embedding halfvec_cosine_ops)
        WITH (m = 32, ef_construction = 120);

    CREATE TABLE IF NOT EXISTS ros_summary_sources (
        summary_id UUID NOT NULL REFERENCES ros_summaries(id) ON DELETE CASCADE,
        message_id UUID NOT NULL REFERENCES ros_messages(id) ON DELETE RESTRICT,
        ordinal INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (summary_id, message_id)
    );

    CREATE TABLE IF NOT EXISTS ros_subagent_sessions (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        parent_session_key TEXT NOT NULL,
        agent TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'running',
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS ros_delegation_runs (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        from_agent TEXT NOT NULL,
        to_agent TEXT NOT NULL,
        task TEXT NOT NULL,
        model TEXT,
        chain_depth INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL CHECK (status IN ('completed','failed','timeout','cached','blocked')),
        response TEXT,
        iterations INTEGER,
        prompt_tokens INTEGER,
        completion_tokens INTEGER,
        duration_ms INTEGER,
        tools_used JSONB,
        cached BOOLEAN NOT NULL DEFAULT false,
        started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        completed_at TIMESTAMPTZ
    );
    CREATE INDEX IF NOT EXISTS idx_ros_delegation_runs_from
        ON ros_delegation_runs (from_agent, started_at DESC);
    CREATE INDEX IF NOT EXISTS idx_ros_delegation_runs_to
        ON ros_delegation_runs (to_agent, started_at DESC);
    CREATE INDEX IF NOT EXISTS idx_ros_delegation_runs_status
        ON ros_delegation_runs (status, started_at DESC);

    CREATE OR REPLACE FUNCTION notify_embedding_queue() RETURNS trigger AS $func$
    BEGIN
        IF TG_TABLE_NAME = 'ros_messages' THEN
            IF NEW.content IS NOT NULL AND LENGTH(TRIM(NEW.content)) > 0 THEN
                PERFORM graphile_worker.add_job(
                    'embed-target',
                    json_build_object('targetTable', TG_TABLE_NAME, 'targetId', NEW.id),
                    job_key      := 'embed-' || TG_TABLE_NAME || '-' || NEW.id::text,
                    max_attempts := 5
                );
            END IF;
        ELSE
            IF NEW.content IS NOT NULL AND LENGTH(TRIM(NEW.content)) > 0 THEN
                PERFORM graphile_worker.add_job(
                    'embed-target',
                    json_build_object('targetTable', TG_TABLE_NAME, 'targetId', NEW.id),
                    job_key      := 'embed-' || TG_TABLE_NAME || '-' || NEW.id::text,
                    max_attempts := 5
                );
            END IF;
        END IF;
        RETURN NEW;
    END;
    $func$ LANGUAGE plpgsql;

    DROP TRIGGER IF EXISTS trg_embed_message ON ros_messages;
    CREATE TRIGGER trg_embed_message
        AFTER INSERT ON ros_messages
        FOR EACH ROW EXECUTE FUNCTION notify_embedding_queue();

    DROP TRIGGER IF EXISTS trg_embed_summary ON ros_summaries;
    CREATE TRIGGER trg_embed_summary
        AFTER INSERT ON ros_summaries
        FOR EACH ROW EXECUTE FUNCTION notify_embedding_queue();

    INSERT INTO _rivetos_migrations (name) VALUES ('0001_baseline.sql');
END
$rivet$;
