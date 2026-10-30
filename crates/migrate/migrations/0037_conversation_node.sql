DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0037_conversation_node.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE rh_conversations ADD COLUMN IF NOT EXISTS node TEXT CHECK (node <> '');

    INSERT INTO _rivetos_migrations (name) VALUES ('0037_conversation_node.sql');
END
$rivet$;
