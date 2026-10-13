DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0013_owner_user_id.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE IF EXISTS ros_conversations ADD COLUMN IF NOT EXISTS owner_user_id TEXT;
    ALTER TABLE IF EXISTS ros_messages ADD COLUMN IF NOT EXISTS owner_user_id TEXT;
    CREATE INDEX IF NOT EXISTS ros_conversations_owner_user_id_idx
        ON ros_conversations (owner_user_id);
    CREATE INDEX IF NOT EXISTS ros_messages_owner_user_id_idx
        ON ros_messages (owner_user_id);

    INSERT INTO _rivetos_migrations (name) VALUES ('0013_owner_user_id.sql');
END
$rivet$;
