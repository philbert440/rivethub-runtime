DO $rivet$
DECLARE
    pages_owner text;
    owners bigint;
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0049_wiki_scopes.sql') THEN
            RETURN;
        END IF;
    END IF;
    PERFORM set_config('rh.maintenance', 'on', true);

    CREATE TABLE rh_wiki_scopes (
        scope text PRIMARY KEY CHECK (scope ~ '^[A-Za-z0-9._-]+$'),
        owner_user_id text NOT NULL,
        visibility text NOT NULL,
        key_prefix text NOT NULL,
        directory text NOT NULL CHECK (directory !~ '(^/|\.\.)'),
        UNIQUE (owner_user_id, visibility),
        UNIQUE (directory)
    );

    ALTER TABLE rh_wiki_topics ADD COLUMN scope text REFERENCES rh_wiki_scopes (scope);
    ALTER TABLE rh_wiki_topics ADD COLUMN local_slug text;

    SELECT min(owner_user_id), count(DISTINCT owner_user_id) INTO pages_owner, owners
      FROM rh_wiki_topics
     WHERE owner_user_id IS NOT NULL
       AND NOT EXISTS (SELECT 1 FROM rh_merge_map m WHERE m.kind = 'wiki-page' AND m.target_id = rh_wiki_topics.slug);
    IF owners > 1 THEN
        RAISE EXCEPTION 'migration 0049 found wiki pages of % owners outside any merge; it can give only one of them the root wiki', owners;
    END IF;
    IF pages_owner IS NOT NULL THEN
        INSERT INTO rh_wiki_scopes (scope, owner_user_id, visibility, key_prefix, directory)
        VALUES (pages_owner, pages_owner, 'private', '', '');
        UPDATE rh_wiki_topics SET scope = pages_owner, local_slug = slug
         WHERE owner_user_id = pages_owner
           AND NOT EXISTS (SELECT 1 FROM rh_merge_map m WHERE m.kind = 'wiki-page' AND m.target_id = rh_wiki_topics.slug);
    END IF;
    INSERT INTO rh_wiki_scopes (scope, owner_user_id, visibility, key_prefix, directory)
    SELECT DISTINCT w.owner_user_id, w.owner_user_id, 'private', w.owner_user_id || '-', 'users/' || w.owner_user_id
      FROM rh_wiki_topics w JOIN rh_merge_map m ON m.kind = 'wiki-page' AND m.target_id = w.slug
     WHERE w.owner_user_id IS NOT NULL
    ON CONFLICT DO NOTHING;
    UPDATE rh_wiki_topics w SET scope = w.owner_user_id, local_slug = m.source_id
      FROM rh_merge_map m WHERE m.kind = 'wiki-page' AND m.target_id = w.slug AND w.scope IS NULL;
    CREATE UNIQUE INDEX rh_wiki_topics_scope_local ON rh_wiki_topics (scope, local_slug);

    DROP POLICY rh_write ON rh_wiki_topics;
    CREATE POLICY rh_write ON rh_wiki_topics USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR (visibility <> 'private' AND visibility = ANY ((SELECT rh_scope_spaces())::text[])));
    DROP POLICY rh_read ON rh_wiki_citations;
    CREATE POLICY rh_read ON rh_wiki_citations FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR EXISTS (SELECT 1 FROM rh_summaries s WHERE s.id = summary_id));
    DROP POLICY rh_read ON rh_wiki_provenance;
    CREATE POLICY rh_read ON rh_wiki_provenance FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR EXISTS (SELECT 1 FROM rh_summaries s WHERE s.id = source_id));
    CREATE POLICY rh_read ON rh_wiki_scopes FOR SELECT USING (
        (SELECT rh_scope_maintenance()) OR owner_user_id = (SELECT rh_scope_principal())
        OR (visibility <> 'private' AND visibility = ANY ((SELECT rh_scope_spaces())::text[])));
    CREATE POLICY rh_write ON rh_wiki_scopes USING ((SELECT rh_scope_maintenance()));
    ALTER TABLE rh_wiki_scopes ENABLE ROW LEVEL SECURITY;
    ALTER TABLE rh_wiki_scopes FORCE ROW LEVEL SECURITY;

    INSERT INTO _rivetos_migrations (name) VALUES ('0049_wiki_scopes.sql');
END
$rivet$;
