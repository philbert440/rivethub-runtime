DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0036_unembedded_messages.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE INDEX IF NOT EXISTS idx_rh_messages_unembedded
        ON rh_messages (created_at DESC) WHERE embedding IS NULL;

    INSERT INTO _rivetos_migrations (name) VALUES ('0036_unembedded_messages.sql');
END
$rivet$;
