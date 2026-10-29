DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0032_entities.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE rh_topics ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'work'
        CHECK (kind IN ('work', 'type', 'entity'));
    DROP INDEX IF EXISTS ux_rh_topics_slug;
    CREATE UNIQUE INDEX IF NOT EXISTS ux_rh_topics_kind_slug ON rh_topics (kind, slug);

    ALTER TABLE rh_topic_aliases ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'work'
        CHECK (kind IN ('work', 'type', 'entity'));
    DROP INDEX IF EXISTS ux_rh_topic_aliases_key;
    CREATE UNIQUE INDEX IF NOT EXISTS ux_rh_topic_aliases_kind_key ON rh_topic_aliases (kind, alias_key);

    CREATE TABLE IF NOT EXISTS rh_topic_tombstones (
        kind TEXT NOT NULL CHECK (kind IN ('work', 'type', 'entity')),
        slug TEXT NOT NULL CHECK (slug <> ''),
        deleted_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (kind, slug)
    );

    CREATE TABLE IF NOT EXISTS rh_wiki_entity_candidates (
        key TEXT NOT NULL CHECK (key <> ''),
        source TEXT NOT NULL CHECK (source <> ''),
        summary_id UUID NOT NULL,
        patch JSONB NOT NULL,
        held_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (key, source)
    );

    CREATE TABLE IF NOT EXISTS rh_wiki_deferred (
        summary_id UUID PRIMARY KEY,
        waits_session TEXT CHECK (waits_session <> ''),
        waits_conversation UUID,
        patches JSONB NOT NULL,
        deferred_at TIMESTAMPTZ NOT NULL,
        CHECK ((waits_session IS NULL) <> (waits_conversation IS NULL))
    );
    CREATE INDEX IF NOT EXISTS idx_rh_wiki_deferred_session ON rh_wiki_deferred (waits_session);
    CREATE INDEX IF NOT EXISTS idx_rh_wiki_deferred_conversation ON rh_wiki_deferred (waits_conversation);

    UPDATE rh_topics SET kind = 'type'
     WHERE parent_id IS NULL AND slug IN ('people', 'hosts', 'tools') AND kind = 'work';
    UPDATE rh_topics SET slug = 'software', name = 'Software'
     WHERE kind = 'type' AND slug = 'tools'
       AND NOT EXISTS (SELECT 1 FROM rh_topics WHERE kind = 'type' AND slug = 'software');

    WITH RECURSIVE below (id, type_id) AS (
        SELECT t.id, t.parent_id FROM rh_topics t
          JOIN rh_topics p ON p.id = t.parent_id
         WHERE p.kind = 'type' AND t.kind = 'work'
        UNION ALL
        SELECT t.id, b.type_id FROM rh_topics t JOIN below b ON t.parent_id = b.id
    )
    UPDATE rh_topics t SET kind = 'entity', parent_id = b.type_id
      FROM below b WHERE t.id = b.id;

    UPDATE rh_topic_aliases a SET kind = t.kind FROM rh_topics t WHERE t.id = a.topic_id;

    DELETE FROM rh_session_placements p USING rh_topics t
     WHERE t.id = p.topic_id AND t.kind <> 'work';
    UPDATE rh_session_suggestions s SET topic_id = NULL FROM rh_topics t
     WHERE t.id = s.topic_id AND t.kind <> 'work';
    UPDATE rh_wiki_topics w SET topic_id = NULL FROM rh_topics t
     WHERE t.id = w.topic_id AND t.kind = 'type';

    INSERT INTO rh_topics (id, kind, slug, name, created_at, updated_at)
    SELECT gen_random_uuid(), 'type', seed.slug, seed.name, now(), now()
      FROM (VALUES ('people', 'People'), ('agents', 'Agents'), ('hosts', 'Hosts'),
                   ('hardware', 'Hardware'), ('models', 'Models'), ('services', 'Services'),
                   ('software', 'Software'), ('repos', 'Repos'), ('domains', 'Domains'),
                   ('databases', 'Databases')) AS seed (slug, name)
     WHERE NOT EXISTS (SELECT 1 FROM rh_topics t WHERE t.kind = 'type' AND t.slug = seed.slug);

    INSERT INTO rh_topics (id, kind, slug, name, parent_id, created_at, updated_at)
    SELECT gen_random_uuid(), 'type', seed.slug, seed.name, software.id, now(), now()
      FROM (VALUES ('clis-and-tools', 'CLIs and tools'), ('apps', 'Apps'),
                   ('libraries-and-frameworks', 'Libraries and frameworks'),
                   ('os-and-platforms', 'OS and platforms'),
                   ('plugins-and-skills', 'Plugins and skills')) AS seed (slug, name)
      JOIN rh_topics software ON software.kind = 'type' AND software.slug = 'software'
     WHERE NOT EXISTS (SELECT 1 FROM rh_topics t WHERE t.kind = 'type' AND t.slug = seed.slug);

    INSERT INTO _rivetos_migrations (name) VALUES ('0032_entities.sql');
END
$rivet$;
