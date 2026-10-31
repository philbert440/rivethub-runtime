DO $rivet$
DECLARE
    store_owner text := nullif(current_setting('rh.store_owner', true), '');
    owners bigint;
    derived text;
    filled boolean;
    needed boolean;
    viewed boolean := to_regclass('ros_messages') IS NOT NULL;
    tables text[] := ARRAY[
        'rh_message_chunks', 'rh_summaries', 'rh_wiki_extractions', 'rh_wiki_citations',
        'rh_wiki_provenance', 'rh_wiki_entity_candidates', 'rh_wiki_deferred',
        'rh_session_placements', 'rh_conversation_placements'
    ];
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0042_owners.sql') THEN
            RETURN;
        END IF;
    END IF;

    IF store_owner IS NULL THEN
        SELECT min(owner_user_id), count(DISTINCT owner_user_id) INTO store_owner, owners
          FROM rh_conversations WHERE owner_user_id IS NOT NULL;
        IF owners > 1 THEN
            store_owner := NULL;
        END IF;
    END IF;

    IF store_owner IS NULL THEN
        needed := EXISTS (SELECT 1 FROM rh_conversations WHERE owner_user_id IS NULL)
               OR EXISTS (SELECT 1 FROM rh_messages)
               OR EXISTS (SELECT 1 FROM rh_topics WHERE kind <> 'type');
        FOREACH derived IN ARRAY tables LOOP
            EXECUTE format('SELECT EXISTS (SELECT 1 FROM %I)', derived) INTO filled;
            needed := needed OR filled;
        END LOOP;
        IF needed THEN
            RAISE EXCEPTION 'migration 0042 cannot tell whose store this is: its conversations have no single owner. Name the owner with the rh.store_owner setting (for example a connection URL ending in ?options=-c%%20rh.store_owner%%3D<user>) and run it again.';
        END IF;
    END IF;

    UPDATE rh_conversations SET owner_user_id = store_owner WHERE owner_user_id IS NULL;
    ALTER TABLE rh_conversations ALTER COLUMN owner_user_id SET NOT NULL;
    IF store_owner IS NOT NULL THEN
        EXECUTE format('ALTER TABLE rh_conversations ALTER COLUMN owner_user_id SET DEFAULT %L', store_owner);
    END IF;

    IF viewed THEN
        DROP VIEW ros_messages;
    END IF;
    ALTER TABLE rh_messages DROP COLUMN owner_user_id;
    IF store_owner IS NULL THEN
        ALTER TABLE rh_messages ADD COLUMN owner_user_id text NOT NULL;
    ELSE
        EXECUTE format('ALTER TABLE rh_messages ADD COLUMN owner_user_id text NOT NULL DEFAULT %L', store_owner);
    END IF;
    IF viewed THEN
        CREATE VIEW ros_messages AS SELECT * FROM rh_messages;
    END IF;

    FOREACH derived IN ARRAY tables LOOP
        EXECUTE format('SELECT EXISTS (SELECT 1 FROM %I)', derived) INTO filled;
        IF filled THEN
            EXECUTE format('ALTER TABLE %I ADD COLUMN owner_user_id text NOT NULL DEFAULT %L', derived, store_owner);
            EXECUTE format('ALTER TABLE %I ALTER COLUMN owner_user_id DROP DEFAULT', derived);
        ELSE
            EXECUTE format('ALTER TABLE %I ADD COLUMN owner_user_id text NOT NULL', derived);
        END IF;
    END LOOP;

    ALTER TABLE rh_topics ADD COLUMN owner_user_id text;
    UPDATE rh_topics SET owner_user_id = store_owner WHERE kind <> 'type';
    ALTER TABLE rh_topics ADD CONSTRAINT rh_topics_owner CHECK (kind = 'type' OR owner_user_id IS NOT NULL);

    IF EXISTS (SELECT 1 FROM rh_conversations WHERE owner_user_id <> store_owner) THEN
        CREATE TEMP TABLE rh_owned_elsewhere ON COMMIT DROP AS
            SELECT id, session_key, owner_user_id FROM rh_conversations WHERE owner_user_id <> store_owner;
        UPDATE rh_messages m SET owner_user_id = c.owner_user_id
          FROM rh_owned_elsewhere c WHERE m.conversation_id = c.id;
        UPDATE rh_message_chunks k SET owner_user_id = m.owner_user_id
          FROM rh_messages m JOIN rh_owned_elsewhere c ON c.id = m.conversation_id
         WHERE k.message_id = m.id;
        UPDATE rh_summaries s SET owner_user_id = c.owner_user_id
          FROM rh_owned_elsewhere c WHERE s.conversation_id = c.id;
        UPDATE rh_wiki_extractions w SET owner_user_id = s.owner_user_id
          FROM rh_summaries s WHERE w.summary_id = s.id AND s.owner_user_id <> store_owner;
        UPDATE rh_wiki_citations w SET owner_user_id = s.owner_user_id
          FROM rh_summaries s WHERE w.summary_id = s.id AND s.owner_user_id <> store_owner;
        UPDATE rh_wiki_provenance w SET owner_user_id = s.owner_user_id
          FROM rh_summaries s WHERE w.source_id = s.id AND s.owner_user_id <> store_owner;
        UPDATE rh_wiki_entity_candidates w SET owner_user_id = s.owner_user_id
          FROM rh_summaries s WHERE w.summary_id = s.id AND s.owner_user_id <> store_owner;
        UPDATE rh_wiki_deferred w SET owner_user_id = s.owner_user_id
          FROM rh_summaries s WHERE w.summary_id = s.id AND s.owner_user_id <> store_owner;
        UPDATE rh_conversation_placements p SET owner_user_id = c.owner_user_id
          FROM rh_owned_elsewhere c WHERE p.conversation_id = c.id;
        UPDATE rh_session_placements p SET owner_user_id = c.owner_user_id
          FROM rh_owned_elsewhere c WHERE p.session_id = c.session_key;
    END IF;

    INSERT INTO _rivetos_migrations (name) VALUES ('0042_owners.sql');
END
$rivet$;
