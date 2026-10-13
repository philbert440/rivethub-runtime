DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0019_tags.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS ros_tags (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        entity_type TEXT NOT NULL CHECK (entity_type IN ('conversation','summary')),
        entity_id UUID NOT NULL,
        key TEXT NOT NULL CHECK (char_length(key) BETWEEN 1 AND 64 AND key NOT LIKE '%:%'),
        value TEXT NOT NULL CHECK (char_length(value) BETWEEN 1 AND 128),
        display TEXT NOT NULL DEFAULT '',
        source TEXT NOT NULL,
        state TEXT NOT NULL,
        confidence DOUBLE PRECISION CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
        proposed_by TEXT NOT NULL DEFAULT '',
        reason TEXT NOT NULL DEFAULT '',
        decided_by TEXT,
        decided_at TIMESTAMPTZ,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );
    CREATE UNIQUE INDEX IF NOT EXISTS ux_ros_tags_entity_kv
        ON ros_tags (entity_type, entity_id, key, value);
    CREATE INDEX IF NOT EXISTS idx_ros_tags_kv_state
        ON ros_tags (key, value, state);
    CREATE INDEX IF NOT EXISTS idx_ros_tags_pending
        ON ros_tags (created_at DESC) WHERE state = 'suggested';

    CREATE TABLE IF NOT EXISTS ros_tag_taxonomy (
        key TEXT NOT NULL CHECK (char_length(key) BETWEEN 1 AND 64 AND key NOT LIKE '%:%'),
        value TEXT NOT NULL CHECK (char_length(value) BETWEEN 1 AND 128),
        display TEXT NOT NULL DEFAULT '',
        parent_value TEXT CHECK (parent_value IS NULL OR char_length(parent_value) BETWEEN 1 AND 128),
        aliases TEXT[] NOT NULL DEFAULT '{}',
        state TEXT NOT NULL DEFAULT 'accepted',
        source TEXT NOT NULL DEFAULT 'user',
        reason TEXT NOT NULL DEFAULT '',
        decided_at TIMESTAMPTZ,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        PRIMARY KEY (key, value),
        CHECK (parent_value IS NULL OR parent_value <> value)
    );
    CREATE INDEX IF NOT EXISTS idx_ros_tag_taxonomy_parent
        ON ros_tag_taxonomy (key, parent_value);

    INSERT INTO _rivetos_migrations (name) VALUES ('0019_tags.sql');
END
$rivet$;
