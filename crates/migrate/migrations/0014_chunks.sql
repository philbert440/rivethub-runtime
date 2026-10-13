DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0014_chunks.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE IF EXISTS ros_messages ADD COLUMN IF NOT EXISTS content_hash TEXT;

    CREATE TABLE IF NOT EXISTS ros_message_chunks (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        message_id UUID NOT NULL REFERENCES ros_messages(id) ON DELETE CASCADE,
        idx INT NOT NULL,
        char_start INT NOT NULL,
        char_end INT NOT NULL,
        content TEXT NOT NULL,
        embedding halfvec(1024),
        embed_status TEXT,
        embed_error TEXT,
        embed_failures INT NOT NULL DEFAULT 0,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        UNIQUE (message_id, idx)
    );
    CREATE INDEX IF NOT EXISTS ros_message_chunks_embedding_hnsw
        ON ros_message_chunks USING hnsw (embedding halfvec_cosine_ops)
        WITH (m = 32, ef_construction = 120);
    CREATE INDEX IF NOT EXISTS idx_ros_message_chunks_message_id
        ON ros_message_chunks (message_id);

    CREATE OR REPLACE FUNCTION notify_chunk_embedding_queue() RETURNS trigger AS $func$
    BEGIN
        IF NEW.embedding IS NULL
           AND NEW.embed_status IS NULL
           AND NEW.content IS NOT NULL
           AND LENGTH(TRIM(NEW.content)) > 0 THEN
            PERFORM graphile_worker.add_job(
                'embed-target',
                json_build_object('targetTable', 'ros_message_chunks', 'targetId', NEW.id),
                job_key      := 'embed-ros_message_chunks-' || NEW.id::text,
                max_attempts := 5
            );
        END IF;
        RETURN NEW;
    END;
    $func$ LANGUAGE plpgsql;

    DROP TRIGGER IF EXISTS trg_embed_message_chunk ON ros_message_chunks;
    CREATE TRIGGER trg_embed_message_chunk
        AFTER INSERT OR UPDATE OF embedding, content ON ros_message_chunks
        FOR EACH ROW
        WHEN (NEW.embedding IS NULL AND NEW.embed_status IS NULL)
        EXECUTE FUNCTION notify_chunk_embedding_queue();

    INSERT INTO _rivetos_migrations (name) VALUES ('0014_chunks.sql');
END
$rivet$;
