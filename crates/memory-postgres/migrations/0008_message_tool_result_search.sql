DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0008_message_tool_result_search.sql') THEN
            RETURN;
        END IF;
    END IF;

    DROP INDEX IF EXISTS idx_ros_messages_fts;
    ALTER TABLE IF EXISTS ros_messages DROP COLUMN IF EXISTS content_tsv;
    IF to_regclass('ros_messages') IS NOT NULL
       AND NOT EXISTS (
           SELECT 1 FROM pg_attribute
            WHERE attrelid = to_regclass('ros_messages')
              AND attname = 'content_tsv' AND NOT attisdropped
       ) THEN
        ALTER TABLE ros_messages
            ADD COLUMN content_tsv tsvector GENERATED ALWAYS AS (
                to_tsvector('english',
                    left(coalesce(content, ''), 300000) || ' ' || left(coalesce(tool_result, ''), 32768))
            ) STORED;
    END IF;
    CREATE INDEX IF NOT EXISTS idx_ros_messages_fts ON ros_messages USING gin (content_tsv);
    CREATE INDEX IF NOT EXISTS idx_ros_messages_tool_result_trgm
        ON ros_messages USING gin (tool_result gin_trgm_ops)
        WHERE tool_result IS NOT NULL AND length(tool_result) > 0;

    INSERT INTO _rivetos_migrations (name) VALUES ('0008_message_tool_result_search.sql');
END
$rivet$;
