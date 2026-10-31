DO $rivet$
DECLARE
    tbl text;
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0050_memory_counts.sql') THEN
            RETURN;
        END IF;
    END IF;
    PERFORM set_config('rh.maintenance', 'on', true);

    CREATE TABLE rh_backfills (name text PRIMARY KEY);

    CREATE TABLE rh_message_access (
        message_id uuid PRIMARY KEY REFERENCES rh_messages (id) ON DELETE CASCADE,
        access_count integer NOT NULL,
        last_accessed_at timestamptz NOT NULL
    );
    CREATE TABLE rh_summary_access (
        summary_id uuid PRIMARY KEY REFERENCES rh_summaries (id) ON DELETE CASCADE,
        access_count integer NOT NULL,
        last_accessed_at timestamptz NOT NULL
    );
    CREATE POLICY rh_read ON rh_message_access FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR EXISTS (SELECT 1 FROM rh_messages m WHERE m.id = message_id));
    CREATE POLICY rh_read ON rh_summary_access FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR EXISTS (SELECT 1 FROM rh_summaries s WHERE s.id = summary_id));

    CREATE TABLE rh_message_counts (
        conversation_id uuid NOT NULL,
        owner_user_id text NOT NULL,
        tool_name text NOT NULL,
        total bigint NOT NULL,
        tool_calls bigint NOT NULL,
        embedded bigint NOT NULL,
        queued bigint NOT NULL,
        failed bigint NOT NULL,
        PRIMARY KEY (conversation_id, owner_user_id, tool_name)
    );
    CREATE TABLE rh_message_counts_pending (conversation_id uuid PRIMARY KEY);
    CREATE POLICY rh_read ON rh_message_counts FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR conversation_id IN (SELECT c.id FROM rh_conversations c WHERE c.visibility <> 'private'));

    FOREACH tbl IN ARRAY ARRAY['rh_message_access', 'rh_summary_access', 'rh_message_counts'] LOOP
        EXECUTE format('CREATE POLICY rh_write ON %I USING ((SELECT rh_scope_maintenance()))', tbl);
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', tbl);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', tbl);
    END LOOP;

    CREATE FUNCTION rh_message_count_add(changed jsonb) RETURNS void
        LANGUAGE plpgsql VOLATILE
        AS $$
        BEGIN
            INSERT INTO rh_message_counts AS c
                   (conversation_id, owner_user_id, tool_name, total, tool_calls, embedded, queued, failed)
            SELECT d.conversation_id, d.owner_user_id, d.tool_name,
                   sum(d.total), sum(d.tool_calls), sum(d.embedded), sum(d.queued), sum(d.failed)
              FROM jsonb_to_recordset(changed) AS d(conversation_id uuid, owner_user_id text, tool_name text,
                       total bigint, tool_calls bigint, embedded bigint, queued bigint, failed bigint)
             WHERE NOT EXISTS (SELECT 1 FROM rh_message_counts_pending p
                                WHERE p.conversation_id = d.conversation_id)
             GROUP BY d.conversation_id, d.owner_user_id, d.tool_name
            HAVING sum(d.total) <> 0 OR sum(d.tool_calls) <> 0 OR sum(d.embedded) <> 0
                OR sum(d.queued) <> 0 OR sum(d.failed) <> 0
             ORDER BY d.conversation_id, d.owner_user_id, d.tool_name
            ON CONFLICT (conversation_id, owner_user_id, tool_name) DO UPDATE
               SET total = c.total + EXCLUDED.total, tool_calls = c.tool_calls + EXCLUDED.tool_calls,
                   embedded = c.embedded + EXCLUDED.embedded, queued = c.queued + EXCLUDED.queued,
                   failed = c.failed + EXCLUDED.failed;
        END $$;

    CREATE FUNCTION rh_message_count_row(m rh_messages, sign integer) RETURNS jsonb
        LANGUAGE sql IMMUTABLE
        AS $$ SELECT jsonb_build_object(
                  'conversation_id', m.conversation_id, 'owner_user_id', m.owner_user_id,
                  'tool_name', coalesce(m.tool_name, ''), 'total', sign,
                  'tool_calls', sign * (m.role = 'tool' OR m.tool_name IS NOT NULL)::int,
                  'embedded', sign * (m.embedding IS NOT NULL)::int,
                  'queued', sign * coalesce(m.embedding IS NULL
                      AND m.embed_status IS DISTINCT FROM 'unembeddable'
                      AND m.embed_status IS DISTINCT FROM 'failed'
                      AND (m.content <> '' OR m.tool_result <> ''), false)::int,
                  'failed', sign * coalesce(m.embed_status = 'failed', false)::int) $$;

    CREATE FUNCTION rh_message_counts_inserted() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            PERFORM rh_message_count_add((SELECT jsonb_agg(rh_message_count_row(n, 1)) FROM new_rows n));
            PERFORM rh_restore(previous);
            RETURN NULL;
        END $$;
    CREATE FUNCTION rh_message_counts_updated() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            PERFORM rh_message_count_add((SELECT jsonb_agg(d.r) FROM (
                SELECT rh_message_count_row(n, 1) AS r FROM new_rows n
                UNION ALL SELECT rh_message_count_row(o, -1) FROM old_rows o) d));
            PERFORM rh_restore(previous);
            RETURN NULL;
        END $$;
    CREATE FUNCTION rh_message_counts_deleted() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            PERFORM rh_message_count_add((SELECT jsonb_agg(rh_message_count_row(o, -1)) FROM old_rows o));
            PERFORM rh_restore(previous);
            RETURN NULL;
        END $$;
    CREATE TRIGGER rh_message_counts_inserted AFTER INSERT ON rh_messages
        REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION rh_message_counts_inserted();
    CREATE TRIGGER rh_message_counts_updated AFTER UPDATE ON rh_messages
        REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
        FOR EACH STATEMENT EXECUTE FUNCTION rh_message_counts_updated();
    CREATE TRIGGER rh_message_counts_deleted AFTER DELETE ON rh_messages
        REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION rh_message_counts_deleted();
    INSERT INTO rh_message_counts_pending (conversation_id) SELECT id FROM rh_conversations;
    IF FOUND THEN
        INSERT INTO rh_backfills (name) VALUES ('message-counts');
    END IF;

    CREATE FUNCTION rh_backfill_message_counts(budget integer) RETURNS bigint
        LANGUAGE plpgsql VOLATILE
        AS $$
        DECLARE
            previous text := rh_elevate();
            picked uuid[];
            left_over bigint;
        BEGIN
            SELECT array_agg(sized.conversation_id) INTO picked
              FROM (SELECT p.conversation_id,
                           sum(p.messages) OVER (ORDER BY p.conversation_id) - p.messages AS before
                      FROM (SELECT w.conversation_id,
                                   (SELECT count(*) FROM rh_messages m
                                     WHERE m.conversation_id = w.conversation_id) AS messages
                              FROM rh_message_counts_pending w
                             ORDER BY w.conversation_id LIMIT 200) p) sized
             WHERE sized.before < budget;
            LOCK TABLE rh_messages IN SHARE MODE;
            DELETE FROM rh_message_counts_pending WHERE conversation_id = ANY (picked);
            INSERT INTO rh_message_counts
                   (conversation_id, owner_user_id, tool_name, total, tool_calls, embedded, queued, failed)
            SELECT m.conversation_id, m.owner_user_id, coalesce(m.tool_name, ''), count(*),
                   count(*) FILTER (WHERE m.role = 'tool' OR m.tool_name IS NOT NULL),
                   count(m.embedding),
                   count(*) FILTER (WHERE m.embedding IS NULL AND m.embed_status IS DISTINCT FROM 'unembeddable'
                                      AND m.embed_status IS DISTINCT FROM 'failed'
                                      AND (m.content <> '' OR m.tool_result <> '')),
                   count(*) FILTER (WHERE m.embed_status = 'failed')
              FROM rh_messages m WHERE m.conversation_id = ANY (picked)
             GROUP BY 1, 2, 3
            ON CONFLICT (conversation_id, owner_user_id, tool_name) DO UPDATE
               SET total = EXCLUDED.total, tool_calls = EXCLUDED.tool_calls, embedded = EXCLUDED.embedded,
                   queued = EXCLUDED.queued, failed = EXCLUDED.failed;
            SELECT count(*) INTO left_over FROM rh_message_counts_pending;
            IF left_over = 0 THEN
                DELETE FROM rh_backfills WHERE name = 'message-counts';
            END IF;
            PERFORM rh_restore(previous);
            RETURN left_over;
        END $$;

    ALTER TABLE rh_message_chunks ADD COLUMN said boolean NOT NULL DEFAULT false;
    CREATE OR REPLACE FUNCTION rh_chunk_conversation_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            SELECT m.conversation_id, m.role IN ('user', 'assistant')
              INTO NEW.conversation_id, NEW.said
              FROM rh_messages m WHERE m.id = NEW.message_id;
            NEW.said := coalesce(NEW.said, false);
            PERFORM rh_restore(previous);
            RETURN NEW;
        END $$;
    PERFORM set_config('max_parallel_maintenance_workers', '0', true);
    CREATE INDEX rh_message_chunks_said_hnsw ON rh_message_chunks
        USING hnsw (embedding halfvec_cosine_ops) WITH (m = '32', ef_construction = '120') WHERE said;
    IF EXISTS (SELECT 1 FROM rh_message_chunks) THEN
        INSERT INTO rh_backfills (name) VALUES ('chunk-said');
    END IF;

    CREATE FUNCTION rh_backfill_chunk_said(after uuid, batch integer) RETURNS uuid
        LANGUAGE plpgsql VOLATILE
        AS $$
        DECLARE
            previous text := rh_elevate();
            last uuid;
        BEGIN
            WITH scanned AS (
                SELECT k.id, k.message_id, k.said FROM rh_message_chunks k
                 WHERE after IS NULL OR k.id > after ORDER BY k.id LIMIT batch),
            marked AS (
                UPDATE rh_message_chunks c SET said = true
                  FROM scanned s JOIN rh_messages m ON m.id = s.message_id
                 WHERE c.id = s.id AND NOT s.said AND m.role IN ('user', 'assistant'))
            SELECT s.id INTO last FROM scanned s ORDER BY s.id DESC LIMIT 1;
            IF last IS NULL THEN
                DELETE FROM rh_backfills WHERE name = 'chunk-said';
            END IF;
            PERFORM rh_restore(previous);
            RETURN last;
        END $$;

    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        INSERT INTO _rivetos_migrations (name) VALUES ('0050_memory_counts.sql');
    END IF;
END
$rivet$;
