CREATE TABLE IF NOT EXISTS rh_topics (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL CHECK (slug <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    parent_id TEXT REFERENCES rh_topics (id),
    directory_kind TEXT CHECK (directory_kind IN ('local', 'shared')),
    directory_node TEXT,
    directory_path TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (parent_id IS NULL OR parent_id <> id),
    CHECK (
        (directory_kind IS NULL AND directory_node IS NULL AND directory_path IS NULL)
        OR (directory_kind = 'local' AND directory_node IS NOT NULL AND directory_path IS NOT NULL)
        OR (directory_kind = 'shared' AND directory_node IS NULL AND directory_path IS NOT NULL)
    )
);
CREATE UNIQUE INDEX IF NOT EXISTS rh_topics_slug ON rh_topics (slug);
CREATE INDEX IF NOT EXISTS rh_topics_parent ON rh_topics (parent_id) WHERE parent_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS rh_topic_aliases (
    topic_id TEXT NOT NULL REFERENCES rh_topics (id) ON DELETE CASCADE,
    alias TEXT NOT NULL CHECK (alias <> ''),
    alias_key TEXT NOT NULL CHECK (alias_key <> '')
);
CREATE UNIQUE INDEX IF NOT EXISTS rh_topic_aliases_key ON rh_topic_aliases (alias_key);
CREATE INDEX IF NOT EXISTS rh_topic_aliases_topic ON rh_topic_aliases (topic_id);

CREATE TABLE IF NOT EXISTS rh_topic_redirects (
    slug TEXT PRIMARY KEY CHECK (slug <> ''),
    topic_id TEXT NOT NULL REFERENCES rh_topics (id) ON DELETE CASCADE,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS rh_topic_redirects_topic ON rh_topic_redirects (topic_id);

CREATE TABLE IF NOT EXISTS rh_session_placements (
    session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
    topic_id TEXT NOT NULL REFERENCES rh_topics (id),
    placed_by TEXT NOT NULL CHECK (placed_by IN ('you', 'categorizer')),
    placed_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS rh_session_placements_topic ON rh_session_placements (topic_id);
