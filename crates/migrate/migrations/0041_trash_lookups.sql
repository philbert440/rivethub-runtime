DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0041_trash_lookups.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE INDEX IF NOT EXISTS idx_rh_summaries_conversation ON rh_summaries (conversation_id);
    CREATE INDEX IF NOT EXISTS idx_rh_wiki_provenance_conversation ON rh_wiki_provenance (conversation_id);

    INSERT INTO _rivetos_migrations (name) VALUES ('0041_trash_lookups.sql');
END
$rivet$;
