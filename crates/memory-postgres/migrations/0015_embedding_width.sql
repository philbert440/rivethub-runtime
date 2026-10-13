DO $rivet$
DECLARE
    v_typmod integer;
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0015_embedding_width.sql') THEN
            RETURN;
        END IF;
    END IF;

    SELECT atttypmod INTO v_typmod
      FROM pg_attribute
     WHERE attrelid = to_regclass('ros_messages')
       AND attname = 'embedding' AND NOT attisdropped;
    IF v_typmod IS NOT NULL AND v_typmod = 1028 THEN
        ALTER TABLE ros_messages ADD COLUMN IF NOT EXISTS embed_failures INTEGER DEFAULT 0;
    ELSE
        IF EXISTS (SELECT 1 FROM ros_messages WHERE embedding IS NOT NULL)
           OR EXISTS (SELECT 1 FROM ros_summaries WHERE embedding IS NOT NULL) THEN
            RAISE EXCEPTION '0015: non-null vectors exist; refusing to recast halfvec(4000) to halfvec(1024)';
        END IF;
        DROP INDEX IF EXISTS ros_messages_embedding_hnsw;
        DROP INDEX IF EXISTS ros_summaries_embedding_hnsw;
        ALTER TABLE ros_messages ALTER COLUMN embedding TYPE halfvec(1024);
        ALTER TABLE ros_summaries ALTER COLUMN embedding TYPE halfvec(1024);
        ALTER TABLE ros_messages ADD COLUMN IF NOT EXISTS embed_failures INTEGER DEFAULT 0;
    END IF;

    CREATE INDEX IF NOT EXISTS ros_messages_embedding_hnsw
        ON ros_messages USING hnsw (embedding halfvec_cosine_ops)
        WITH (m = 32, ef_construction = 120);
    CREATE INDEX IF NOT EXISTS ros_summaries_embedding_hnsw
        ON ros_summaries USING hnsw (embedding halfvec_cosine_ops)
        WITH (m = 32, ef_construction = 120);

    INSERT INTO _rivetos_migrations (name) VALUES ('0015_embedding_width.sql');
END
$rivet$;
