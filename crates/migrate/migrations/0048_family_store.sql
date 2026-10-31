DO $rivet$
DECLARE
    store_owner text := nullif(current_setting('rh.store_owner', true), '');
    owners bigint;
    tbl text;
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0048_family_store.sql') THEN
            RETURN;
        END IF;
    END IF;
    PERFORM set_config('rh.maintenance', 'on', true);

    IF store_owner IS NULL THEN
        SELECT min(owner_user_id), count(DISTINCT owner_user_id) INTO store_owner, owners FROM rh_conversations;
        IF owners <> 1 THEN
            store_owner := NULL;
        END IF;
    END IF;

    IF store_owner IS NULL THEN
        ALTER TABLE rh_conversations ALTER COLUMN owner_user_id SET DEFAULT rh_scope_principal();
        ALTER TABLE rh_messages ALTER COLUMN owner_user_id SET DEFAULT rh_scope_principal();
    ELSE
        EXECUTE format('ALTER TABLE rh_conversations ALTER COLUMN owner_user_id SET DEFAULT coalesce(rh_scope_principal(), %L)', store_owner);
        EXECUTE format('ALTER TABLE rh_messages ALTER COLUMN owner_user_id SET DEFAULT coalesce(rh_scope_principal(), %L)', store_owner);
    END IF;

    ALTER TABLE rh_tags ADD COLUMN owner_user_id text;
    UPDATE rh_tags t SET owner_user_id = coalesce(
        CASE t.entity_type
            WHEN 'conversation' THEN (SELECT c.owner_user_id FROM rh_conversations c WHERE c.id = t.entity_id)
            WHEN 'message' THEN (SELECT m.owner_user_id FROM rh_messages m WHERE m.id = t.entity_id)
            WHEN 'summary' THEN (SELECT s.owner_user_id FROM rh_summaries s WHERE s.id = t.entity_id)
            WHEN 'topic' THEN (SELECT o.owner_user_id FROM rh_topics o WHERE o.id = t.entity_id)
        END, store_owner);
    CREATE INDEX rh_tags_owner ON rh_tags (owner_user_id);

    ALTER TABLE rh_tidy_batches ADD COLUMN owner_user_id text;
    UPDATE rh_tidy_batches SET owner_user_id = store_owner;
    IF store_owner IS NULL THEN
        ALTER TABLE rh_tidy_batches ALTER COLUMN owner_user_id SET DEFAULT rh_scope_principal();
    ELSE
        EXECUTE format('ALTER TABLE rh_tidy_batches ALTER COLUMN owner_user_id SET DEFAULT coalesce(rh_scope_principal(), %L)', store_owner);
    END IF;

    CREATE FUNCTION rh_tag_owner_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            IF NEW.owner_user_id IS NULL THEN
                NEW.owner_user_id := CASE NEW.entity_type
                    WHEN 'conversation' THEN (SELECT c.owner_user_id FROM rh_conversations c WHERE c.id = NEW.entity_id)
                    WHEN 'message' THEN (SELECT m.owner_user_id FROM rh_messages m WHERE m.id = NEW.entity_id)
                    WHEN 'summary' THEN (SELECT s.owner_user_id FROM rh_summaries s WHERE s.id = NEW.entity_id)
                    WHEN 'topic' THEN (SELECT o.owner_user_id FROM rh_topics o WHERE o.id = NEW.entity_id)
                END;
            END IF;
            PERFORM rh_restore(previous);
            RETURN NEW;
        END $$;
    CREATE TRIGGER rh_tag_owner BEFORE INSERT ON rh_tags
        FOR EACH ROW EXECUTE FUNCTION rh_tag_owner_trigger();

    CREATE OR REPLACE FUNCTION rh_wiki_owner_trigger() RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        DECLARE
            previous text := rh_elevate();
        BEGIN
            IF NEW.owner_user_id IS NULL THEN
                NEW.owner_user_id := coalesce(nullif(current_setting('rh.principal', true), ''),
                    (SELECT min(c.owner_user_id) FROM rh_conversations c HAVING count(DISTINCT c.owner_user_id) = 1));
            END IF;
            PERFORM rh_restore(previous);
            RETURN NEW;
        END $$;

    CREATE TABLE rh_store_merges (
        source text PRIMARY KEY CHECK (source <> ''),
        owner_user_id text NOT NULL,
        first_at timestamptz NOT NULL,
        last_at timestamptz NOT NULL,
        runs integer NOT NULL DEFAULT 1,
        rows jsonb NOT NULL DEFAULT '{}'::jsonb
    );
    CREATE TABLE rh_merge_map (
        source text NOT NULL REFERENCES rh_store_merges (source) ON DELETE CASCADE,
        kind text NOT NULL,
        source_id text NOT NULL,
        target_id text NOT NULL,
        PRIMARY KEY (source, kind, source_id)
    );

    CREATE POLICY rh_own ON rh_tasks USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal()));
    CREATE POLICY rh_own ON rh_tags USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal()));
    CREATE POLICY rh_own ON rh_tidy_batches USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal()));
    CREATE POLICY rh_own ON rh_tidy_proposals USING (
        (SELECT rh_scope_maintenance()) OR EXISTS (SELECT 1 FROM rh_tidy_batches b WHERE b.id = batch_id));
    CREATE POLICY rh_own ON rh_store_merges USING ((SELECT rh_scope_maintenance()));
    CREATE POLICY rh_own ON rh_merge_map USING ((SELECT rh_scope_maintenance()));
    FOREACH tbl IN ARRAY ARRAY['rh_tasks', 'rh_tags', 'rh_tidy_batches', 'rh_tidy_proposals',
                               'rh_store_merges', 'rh_merge_map'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', tbl);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', tbl);
    END LOOP;

    INSERT INTO _rivetos_migrations (name) VALUES ('0048_family_store.sql');
END
$rivet$;
