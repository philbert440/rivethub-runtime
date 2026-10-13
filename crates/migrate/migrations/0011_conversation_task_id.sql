DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0011_conversation_task_id.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE IF EXISTS ros_conversations ADD COLUMN IF NOT EXISTS task_id UUID;
    CREATE INDEX IF NOT EXISTS idx_ros_conversations_task
        ON ros_conversations (task_id, created_at) WHERE task_id IS NOT NULL;

    INSERT INTO _rivetos_migrations (name) VALUES ('0011_conversation_task_id.sql');
END
$rivet$;
