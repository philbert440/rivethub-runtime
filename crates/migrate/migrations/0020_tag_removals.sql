DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0020_tag_removals.sql') THEN
            RETURN;
        END IF;
    END IF;

    ALTER TABLE ros_tags ADD COLUMN IF NOT EXISTS removal_state TEXT;
    ALTER TABLE ros_tags ADD COLUMN IF NOT EXISTS removal_reason TEXT NOT NULL DEFAULT '';
    CREATE INDEX IF NOT EXISTS idx_ros_tags_removal_state
        ON ros_tags (removal_state) WHERE removal_state = 'suggested';

    INSERT INTO _rivetos_migrations (name) VALUES ('0020_tag_removals.sql');
END
$rivet$;
