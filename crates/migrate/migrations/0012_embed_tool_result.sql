DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0012_embed_tool_result.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE OR REPLACE FUNCTION notify_embedding_queue() RETURNS trigger AS $func$
    BEGIN
        IF TG_TABLE_NAME = 'ros_messages' THEN
            IF (NEW.content IS NOT NULL AND LENGTH(TRIM(NEW.content)) > 0)
               OR (NEW.tool_result IS NOT NULL AND LENGTH(TRIM(NEW.tool_result)) > 0) THEN
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

    INSERT INTO _rivetos_migrations (name) VALUES ('0012_embed_tool_result.sql');
END
$rivet$;
