DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0026_hub_state.sql') THEN
            RETURN;
        END IF;
    END IF;

    CREATE TABLE IF NOT EXISTS rh_hub_state (
        user_id TEXT PRIMARY KEY,
        version BIGINT NOT NULL CHECK (version > 0),
        state JSONB NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );

    INSERT INTO _rivetos_migrations (name) VALUES ('0026_hub_state.sql');
END
$rivet$;
