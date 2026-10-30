DROP INDEX IF EXISTS rh_topics_slug;
CREATE UNIQUE INDEX IF NOT EXISTS rh_topics_kind_slug ON rh_topics (kind, slug);
DROP INDEX IF EXISTS rh_topic_aliases_key;
CREATE UNIQUE INDEX IF NOT EXISTS rh_topic_aliases_kind_key ON rh_topic_aliases (kind, alias_key);

CREATE TABLE IF NOT EXISTS rh_topic_tombstones (
    kind TEXT NOT NULL CHECK (kind IN ('work', 'type', 'entity')),
    slug TEXT NOT NULL CHECK (slug <> ''),
    deleted_at TEXT NOT NULL,
    PRIMARY KEY (kind, slug)
);

CREATE TABLE IF NOT EXISTS rh_wiki_entity_candidates (
    key TEXT NOT NULL CHECK (key <> ''),
    source TEXT NOT NULL CHECK (source <> ''),
    summary_id TEXT NOT NULL,
    patch TEXT NOT NULL,
    held_at TEXT NOT NULL,
    PRIMARY KEY (key, source)
);

CREATE TABLE IF NOT EXISTS rh_wiki_deferred (
    summary_id TEXT PRIMARY KEY,
    waits_session TEXT CHECK (waits_session <> ''),
    waits_conversation TEXT,
    applied_topic TEXT,
    patches TEXT NOT NULL,
    deferred_at TEXT NOT NULL,
    CHECK ((waits_session IS NULL) <> (waits_conversation IS NULL))
);
CREATE INDEX IF NOT EXISTS rh_wiki_deferred_session ON rh_wiki_deferred (waits_session);
CREATE INDEX IF NOT EXISTS rh_wiki_deferred_conversation ON rh_wiki_deferred (waits_conversation);

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
UPDATE rh_topics
   SET kind = 'entity', parent_id = (SELECT type_id FROM below WHERE below.id = rh_topics.id)
 WHERE id IN (SELECT id FROM below);

UPDATE rh_topic_aliases
   SET kind = (SELECT t.kind FROM rh_topics t WHERE t.id = rh_topic_aliases.topic_id);

DELETE FROM rh_session_placements
 WHERE topic_id IN (SELECT id FROM rh_topics WHERE kind <> 'work');
UPDATE rh_session_suggestions SET topic_id = NULL
 WHERE topic_id IN (SELECT id FROM rh_topics WHERE kind <> 'work');
DELETE FROM rh_topic_pages
 WHERE topic_id IN (SELECT id FROM rh_topics WHERE kind = 'type');

INSERT INTO rh_topics (id, kind, slug, name, created_at, updated_at)
SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)), 2)
             || '-' || substr('89ab', 1 + (abs(random()) % 4), 1) || substr(hex(randomblob(2)), 2)
             || '-' || hex(randomblob(6))),
       'type', seed.column1, seed.column2,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
  FROM (VALUES ('people', 'People'), ('agents', 'Agents'), ('hosts', 'Hosts'),
               ('hardware', 'Hardware'), ('models', 'Models'), ('services', 'Services'),
               ('software', 'Software'), ('repos', 'Repos'), ('domains', 'Domains'),
               ('databases', 'Databases')) AS seed
 WHERE NOT EXISTS (SELECT 1 FROM rh_topics t WHERE t.kind = 'type' AND t.slug = seed.column1);

INSERT INTO rh_topics (id, kind, slug, name, parent_id, created_at, updated_at)
SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)), 2)
             || '-' || substr('89ab', 1 + (abs(random()) % 4), 1) || substr(hex(randomblob(2)), 2)
             || '-' || hex(randomblob(6))),
       'type', seed.column1, seed.column2, software.id,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
  FROM (VALUES ('clis-and-tools', 'CLIs and tools'), ('apps', 'Apps'),
               ('libraries-and-frameworks', 'Libraries and frameworks'),
               ('os-and-platforms', 'OS and platforms'),
               ('plugins-and-skills', 'Plugins and skills')) AS seed
  JOIN rh_topics software ON software.kind = 'type' AND software.slug = 'software'
 WHERE NOT EXISTS (SELECT 1 FROM rh_topics t WHERE t.kind = 'type' AND t.slug = seed.column1);
