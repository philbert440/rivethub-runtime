DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0009_conversation_session_key_unique.sql') THEN
            RETURN;
        END IF;
    END IF;

    IF to_regclass('ros_conversations') IS NOT NULL THEN
        CREATE TEMP TABLE _ros_conv_dedup_map_0009 ON COMMIT DROP AS
        SELECT dupe.id AS dupe_id, keeper.id AS keeper_id
        FROM ros_conversations dupe
        JOIN ros_conversations keeper
          ON keeper.session_key = dupe.session_key
         AND keeper.agent = dupe.agent
         AND (keeper.created_at, keeper.id) < (dupe.created_at, dupe.id);

        UPDATE ros_wiki_provenance p
           SET source_id = m.keeper_id
          FROM _ros_conv_dedup_map_0009 m
         WHERE p.source_kind = 'conversation' AND p.source_id = m.dupe_id;

        UPDATE ros_tasks t
           SET conversation_id = m.keeper_id
          FROM _ros_conv_dedup_map_0009 m
         WHERE t.conversation_id = m.dupe_id;

        DELETE FROM ros_conversations c
         USING _ros_conv_dedup_map_0009 m
         WHERE c.id = m.dupe_id;
    END IF;

    CREATE UNIQUE INDEX IF NOT EXISTS ux_ros_conversations_session_agent
        ON ros_conversations (session_key, agent);

    INSERT INTO _rivetos_migrations (name) VALUES ('0009_conversation_session_key_unique.sql');
END
$rivet$;
