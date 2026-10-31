DO $rivet$
DECLARE
    store_owner text := nullif(current_setting('rh.store_owner', true), '');
    owners bigint;
    leaky text[];
    tbl text;
    owned text[] := ARRAY[
        'rh_wiki_extractions', 'rh_wiki_citations', 'rh_wiki_provenance',
        'rh_wiki_entity_candidates', 'rh_wiki_deferred', 'rh_session_marks'
    ];
    secured text[] := ARRAY[
        'rh_conversations', 'rh_messages', 'rh_message_chunks', 'rh_summaries',
        'rh_wiki_extractions', 'rh_wiki_citations', 'rh_wiki_provenance',
        'rh_wiki_entity_candidates', 'rh_wiki_deferred', 'rh_wiki_topics',
        'rh_topics', 'rh_topic_aliases', 'rh_topic_decisions',
        'rh_session_placements', 'rh_conversation_placements',
        'rh_session_marks', 'rh_user_settings', 'rh_spaces', 'rh_space_members'
    ];
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0044_visibility.sql') THEN
            RETURN;
        END IF;
    END IF;

    IF (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) THEN
        ALTER FUNCTION ts_match_vq(tsvector, tsquery) LEAKPROOF;
        ALTER FUNCTION textlike(text, text) LEAKPROOF;
        ALTER FUNCTION texticlike(text, text) LEAKPROOF;
        ALTER FUNCTION textregexeq(text, text) LEAKPROOF;
        ALTER FUNCTION texticregexeq(text, text) LEAKPROOF;
    END IF;
    SELECT array_agg(p.oid::regprocedure::text ORDER BY p.proname) INTO leaky
      FROM pg_proc p
     WHERE p.oid IN ('ts_match_vq(tsvector,tsquery)'::regprocedure, 'textlike(text,text)'::regprocedure,
                     'texticlike(text,text)'::regprocedure, 'textregexeq(text,text)'::regprocedure,
                     'texticregexeq(text,text)'::regprocedure)
       AND NOT p.proleakproof;
    IF leaky IS NOT NULL THEN
        RAISE EXCEPTION 'migration 0044 turns on row-level security, and search keeps its text indexes under it only when these operators are LEAKPROOF: %. A superuser marks them once per database, for example: ALTER FUNCTION ts_match_vq(tsvector, tsquery) LEAKPROOF; and the same for each listed function. Then run the migration again.', array_to_string(leaky, ', ');
    END IF;

    IF store_owner IS NULL THEN
        SELECT min(owner_user_id), count(DISTINCT owner_user_id) INTO store_owner, owners FROM rh_conversations;
        IF owners <> 1 THEN
            store_owner := NULL;
        END IF;
    END IF;

    CREATE TABLE rh_spaces (
        id text PRIMARY KEY CHECK (id ~ '^[A-Za-z0-9._-]+$' AND id <> 'private'),
        kind text NOT NULL CHECK (kind IN ('household', 'team', 'compartment')),
        name text NOT NULL CHECK (name <> '')
    );
    CREATE TABLE rh_space_members (
        space_id text NOT NULL REFERENCES rh_spaces (id) ON DELETE CASCADE,
        user_id text NOT NULL CHECK (user_id <> ''),
        role text NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'reader', 'admin')),
        PRIMARY KEY (space_id, user_id)
    );
    CREATE INDEX rh_space_members_user ON rh_space_members (user_id);
    CREATE TABLE rh_user_settings (
        user_id text PRIMARY KEY CHECK (user_id <> ''),
        default_visibility text NOT NULL DEFAULT 'private',
        default_since timestamptz NOT NULL,
        updated_at timestamptz NOT NULL
    );
    CREATE TABLE rh_session_marks (
        session_key text NOT NULL CHECK (session_key <> ''),
        owner_user_id text NOT NULL,
        visibility text NOT NULL,
        marked_at timestamptz NOT NULL,
        PRIMARY KEY (session_key, owner_user_id)
    );

    ALTER TABLE rh_conversations ADD COLUMN visibility text NOT NULL DEFAULT 'private';
    CREATE INDEX rh_conversations_shared ON rh_conversations (visibility) WHERE visibility <> 'private';
    ALTER TABLE rh_topics ADD COLUMN visibility text NOT NULL DEFAULT 'private';
    ALTER TABLE rh_topics ADD COLUMN visibility_mark text;
    ALTER TABLE rh_message_chunks ADD COLUMN conversation_id uuid;
    IF store_owner IS NULL THEN
        ALTER TABLE rh_wiki_topics ADD COLUMN owner_user_id text;
    ELSE
        EXECUTE format('ALTER TABLE rh_wiki_topics ADD COLUMN owner_user_id text DEFAULT %L', store_owner);
        ALTER TABLE rh_wiki_topics ALTER COLUMN owner_user_id DROP DEFAULT;
    END IF;
    ALTER TABLE rh_wiki_topics ADD COLUMN visibility text NOT NULL DEFAULT 'private';

    CREATE FUNCTION rh_scope_principal() RETURNS text LANGUAGE sql STABLE PARALLEL SAFE
        AS $$ SELECT nullif(current_setting('rh.principal', true), '') $$;
    CREATE FUNCTION rh_scope_spaces() RETURNS text[] LANGUAGE sql STABLE PARALLEL SAFE
        AS $$ SELECT coalesce(string_to_array(nullif(current_setting('rh.spaces', true), ''), ','), '{}'::text[]) $$;
    CREATE FUNCTION rh_scope_maintenance() RETURNS boolean LANGUAGE sql STABLE PARALLEL SAFE
        AS $$ SELECT coalesce(current_setting('rh.maintenance', true), '') = 'on' $$;

    CREATE FUNCTION rh_member_visibility(owner text, wanted text) RETURNS text
        LANGUAGE sql STABLE
        AS $$ SELECT CASE
                 WHEN wanted IS NULL OR wanted = 'private' THEN 'private'
                 WHEN EXISTS (SELECT 1 FROM rh_space_members m WHERE m.space_id = wanted AND m.user_id = owner) THEN wanted
                 ELSE 'private' END $$;
    CREATE FUNCTION rh_owner_default(owner text, created timestamptz) RETURNS text
        LANGUAGE sql STABLE
        AS $$ SELECT coalesce((SELECT CASE WHEN created >= s.default_since THEN s.default_visibility ELSE 'private' END
                                 FROM rh_user_settings s WHERE s.user_id = owner), 'private') $$;
    CREATE FUNCTION rh_topic_mark(topic uuid) RETURNS text
        LANGUAGE sql STABLE
        AS $$ WITH RECURSIVE up (id, parent_id, mark, depth) AS (
                  SELECT t.id, t.parent_id, t.visibility_mark, 0 FROM rh_topics t WHERE t.id = topic
                  UNION ALL
                  SELECT t.id, t.parent_id, t.visibility_mark, up.depth + 1
                    FROM rh_topics t JOIN up ON t.id = up.parent_id
                   WHERE up.mark IS NULL AND up.depth < 64)
              SELECT mark FROM up WHERE mark IS NOT NULL ORDER BY depth LIMIT 1 $$;
    CREATE FUNCTION rh_topic_visibility(topic uuid) RETURNS text
        LANGUAGE sql STABLE
        AS $$ SELECT CASE WHEN t.owner_user_id IS NULL THEN 'private'
                          ELSE rh_member_visibility(t.owner_user_id,
                                   coalesce(rh_topic_mark(t.id), rh_owner_default(t.owner_user_id, t.created_at))) END
                FROM rh_topics t WHERE t.id = topic $$;
    CREATE FUNCTION rh_session_visibility(session text, owner text, created timestamptz) RETURNS text
        LANGUAGE sql STABLE
        AS $$ SELECT rh_member_visibility(owner, coalesce(
                  (SELECT m.visibility FROM rh_session_marks m WHERE m.session_key = session AND m.owner_user_id = owner),
                  (SELECT rh_topic_mark(p.topic_id) FROM rh_session_placements p WHERE p.session_id = session),
                  rh_owner_default(owner, created))) $$;

    CREATE FUNCTION rh_elevate() RETURNS text
        LANGUAGE plpgsql VOLATILE
        AS $$ DECLARE previous text := coalesce(current_setting('rh.maintenance', true), '');
              BEGIN PERFORM set_config('rh.maintenance', 'on', true); RETURN previous; END $$;
    CREATE FUNCTION rh_restore(previous text) RETURNS void
        LANGUAGE plpgsql VOLATILE
        AS $$ BEGIN PERFORM set_config('rh.maintenance', previous, true); END $$;

    CREATE FUNCTION rh_refresh_sessions(keys text[]) RETURNS void
        LANGUAGE plpgsql VOLATILE
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            UPDATE rh_conversations c SET visibility = v.visibility
              FROM (SELECT id, rh_session_visibility(session_key, owner_user_id, created_at) AS visibility
                      FROM rh_conversations WHERE session_key = ANY (keys)) v
             WHERE c.id = v.id AND c.visibility IS DISTINCT FROM v.visibility;
            PERFORM rh_restore(previous);
        END $$;
    CREATE FUNCTION rh_refresh_topics(roots uuid[]) RETURNS void
        LANGUAGE plpgsql VOLATILE
        AS $$
        DECLARE
            previous text := rh_elevate();
            tree uuid[];
        BEGIN
            WITH RECURSIVE down (id, depth) AS (
                SELECT t.id, 0 FROM rh_topics t WHERE t.id = ANY (roots)
                UNION
                SELECT t.id, down.depth + 1 FROM rh_topics t JOIN down ON t.parent_id = down.id WHERE down.depth < 64)
            SELECT array_agg(DISTINCT id) INTO tree FROM down;
            IF tree IS NOT NULL THEN
                UPDATE rh_topics t SET visibility = rh_topic_visibility(t.id)
                 WHERE t.id = ANY (tree) AND t.visibility IS DISTINCT FROM rh_topic_visibility(t.id);
                PERFORM rh_refresh_sessions(ARRAY(SELECT p.session_id FROM rh_session_placements p
                                                   WHERE p.topic_id = ANY (tree)));
            END IF;
            PERFORM rh_restore(previous);
        END $$;
    CREATE FUNCTION rh_refresh_owner(owner text) RETURNS void
        LANGUAGE plpgsql VOLATILE
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            UPDATE rh_topics t SET visibility = rh_topic_visibility(t.id)
             WHERE t.owner_user_id = owner AND t.visibility IS DISTINCT FROM rh_topic_visibility(t.id);
            UPDATE rh_conversations c SET visibility = rh_session_visibility(c.session_key, c.owner_user_id, c.created_at)
             WHERE c.owner_user_id = owner
               AND c.visibility IS DISTINCT FROM rh_session_visibility(c.session_key, c.owner_user_id, c.created_at);
            PERFORM rh_restore(previous);
        END $$;

    CREATE FUNCTION rh_conversation_visibility_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            NEW.visibility := rh_session_visibility(NEW.session_key, NEW.owner_user_id, NEW.created_at);
            PERFORM rh_restore(previous);
            RETURN NEW;
        END $$;
    CREATE TRIGGER rh_conversation_visibility BEFORE INSERT OR UPDATE OF session_key, owner_user_id ON rh_conversations
        FOR EACH ROW EXECUTE FUNCTION rh_conversation_visibility_trigger();
    CREATE FUNCTION rh_topic_visibility_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            NEW.visibility := CASE WHEN NEW.owner_user_id IS NULL THEN 'private'
                ELSE rh_member_visibility(NEW.owner_user_id, coalesce(NEW.visibility_mark, rh_topic_mark(NEW.parent_id),
                         rh_owner_default(NEW.owner_user_id, NEW.created_at))) END;
            PERFORM rh_restore(previous);
            RETURN NEW;
        END $$;
    CREATE TRIGGER rh_topic_visibility BEFORE INSERT ON rh_topics
        FOR EACH ROW EXECUTE FUNCTION rh_topic_visibility_trigger();
    CREATE FUNCTION rh_topic_marked_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$ BEGIN PERFORM rh_refresh_topics(ARRAY[NEW.id]); RETURN NULL; END $$;
    CREATE TRIGGER rh_topic_marked AFTER UPDATE OF visibility_mark, parent_id, owner_user_id ON rh_topics
        FOR EACH ROW EXECUTE FUNCTION rh_topic_marked_trigger();
    CREATE FUNCTION rh_session_changed_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$ BEGIN
            IF TG_OP = 'DELETE' THEN
                PERFORM rh_refresh_sessions(ARRAY[OLD.session_key]);
            ELSE
                PERFORM rh_refresh_sessions(ARRAY[NEW.session_key]);
            END IF;
            RETURN NULL;
        END $$;
    CREATE TRIGGER rh_session_marked AFTER INSERT OR UPDATE OR DELETE ON rh_session_marks
        FOR EACH ROW EXECUTE FUNCTION rh_session_changed_trigger();
    CREATE FUNCTION rh_session_placed_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$ BEGIN
            IF TG_OP = 'DELETE' THEN
                PERFORM rh_refresh_sessions(ARRAY[OLD.session_id]);
            ELSE
                PERFORM rh_refresh_sessions(ARRAY[NEW.session_id]);
            END IF;
            RETURN NULL;
        END $$;
    CREATE TRIGGER rh_session_placed AFTER INSERT OR UPDATE OR DELETE ON rh_session_placements
        FOR EACH ROW EXECUTE FUNCTION rh_session_placed_trigger();
    CREATE FUNCTION rh_owner_changed_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$ BEGIN
            IF TG_OP = 'DELETE' THEN
                PERFORM rh_refresh_owner(OLD.user_id);
            ELSE
                PERFORM rh_refresh_owner(NEW.user_id);
            END IF;
            RETURN NULL;
        END $$;
    CREATE TRIGGER rh_owner_settings AFTER INSERT OR UPDATE OR DELETE ON rh_user_settings
        FOR EACH ROW EXECUTE FUNCTION rh_owner_changed_trigger();
    CREATE TRIGGER rh_owner_membership AFTER INSERT OR UPDATE OR DELETE ON rh_space_members
        FOR EACH ROW EXECUTE FUNCTION rh_owner_changed_trigger();
    CREATE FUNCTION rh_chunk_conversation_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            NEW.conversation_id := (SELECT m.conversation_id FROM rh_messages m WHERE m.id = NEW.message_id);
            PERFORM rh_restore(previous);
            RETURN NEW;
        END $$;
    CREATE TRIGGER rh_chunk_conversation BEFORE INSERT ON rh_message_chunks
        FOR EACH ROW EXECUTE FUNCTION rh_chunk_conversation_trigger();
    CREATE FUNCTION rh_wiki_owner_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            IF NEW.owner_user_id IS NULL THEN
                NEW.owner_user_id := (SELECT min(c.owner_user_id) FROM rh_conversations c
                                      HAVING count(DISTINCT c.owner_user_id) = 1);
            END IF;
            PERFORM rh_restore(previous);
            RETURN NEW;
        END $$;
    CREATE TRIGGER rh_wiki_owner BEFORE INSERT ON rh_wiki_topics
        FOR EACH ROW EXECUTE FUNCTION rh_wiki_owner_trigger();

    CREATE POLICY rh_read ON rh_conversations FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR (visibility <> 'private' AND visibility = ANY ((SELECT rh_scope_spaces())::text[])));
    CREATE POLICY rh_read ON rh_topics FOR SELECT USING (
        owner_user_id IS NULL OR (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR (visibility <> 'private' AND visibility = ANY ((SELECT rh_scope_spaces())::text[])));
    CREATE POLICY rh_read ON rh_wiki_topics FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR (visibility <> 'private' AND visibility = ANY ((SELECT rh_scope_spaces())::text[])));
    FOREACH tbl IN ARRAY ARRAY['rh_messages', 'rh_message_chunks', 'rh_summaries'] LOOP
        EXECUTE format('CREATE POLICY rh_read ON %I FOR SELECT USING (
            (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
            OR conversation_id IN (SELECT c.id FROM rh_conversations c WHERE c.visibility <> %L))', tbl, 'private');
    END LOOP;
    FOREACH tbl IN ARRAY owned LOOP
        EXECUTE format('CREATE POLICY rh_read ON %I FOR SELECT USING (
            (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal()))', tbl);
    END LOOP;
    FOREACH tbl IN ARRAY ARRAY['rh_topic_aliases', 'rh_topic_decisions'] LOOP
        EXECUTE format('CREATE POLICY rh_read ON %I FOR SELECT USING (
            (SELECT rh_scope_maintenance()) OR EXISTS (SELECT 1 FROM rh_topics t WHERE t.id = topic_id))', tbl);
        EXECUTE format('CREATE POLICY rh_write ON %I USING (
            (SELECT rh_scope_maintenance()) OR EXISTS (SELECT 1 FROM rh_topics t WHERE t.id = topic_id))', tbl);
    END LOOP;
    CREATE POLICY rh_read ON rh_session_placements FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR (EXISTS (SELECT 1 FROM rh_conversations c WHERE c.session_key = session_id AND c.visibility <> 'private')
            AND EXISTS (SELECT 1 FROM rh_topics t WHERE t.id = topic_id)));
    CREATE POLICY rh_read ON rh_conversation_placements FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR (EXISTS (SELECT 1 FROM rh_conversations c WHERE c.id = conversation_id AND c.visibility <> 'private')
            AND EXISTS (SELECT 1 FROM rh_topics t WHERE t.id = topic_id)));
    FOREACH tbl IN ARRAY ARRAY[
        'rh_conversations', 'rh_messages', 'rh_message_chunks', 'rh_summaries', 'rh_wiki_topics', 'rh_topics',
        'rh_session_placements', 'rh_conversation_placements'] || owned LOOP
        EXECUTE format('CREATE POLICY rh_write ON %I USING (
            (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal()))', tbl);
    END LOOP;
    CREATE POLICY rh_own ON rh_user_settings USING (
        (SELECT rh_scope_maintenance()) OR user_id = (SELECT rh_scope_principal()));
    CREATE POLICY rh_read ON rh_spaces FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR id = ANY ((SELECT rh_scope_spaces())::text[]));
    CREATE POLICY rh_read ON rh_space_members FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR space_id = ANY ((SELECT rh_scope_spaces())::text[]));
    CREATE POLICY rh_write ON rh_spaces USING ((SELECT rh_scope_maintenance()));
    CREATE POLICY rh_write ON rh_space_members USING ((SELECT rh_scope_maintenance()));

    FOREACH tbl IN ARRAY secured LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', tbl);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', tbl);
    END LOOP;

    INSERT INTO _rivetos_migrations (name) VALUES ('0044_visibility.sql');
END
$rivet$;
