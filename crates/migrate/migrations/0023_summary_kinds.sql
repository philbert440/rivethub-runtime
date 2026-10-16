DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0023_summary_kinds.sql') THEN
            RETURN;
        END IF;
    END IF;

    UPDATE rh_summaries
    SET kind = CASE
        WHEN depth = 0 THEN 'leaf'
        WHEN depth = 1 THEN 'branch'
        WHEN depth >= 2 THEN 'root'
    END
    WHERE kind NOT IN ('leaf', 'branch', 'root')
      AND depth >= 0;

    IF EXISTS (
        SELECT 1 FROM rh_summaries
        WHERE kind NOT IN ('leaf', 'branch', 'root')
    ) THEN
        RAISE EXCEPTION 'legacy summary kind has no depth mapping';
    END IF;

    INSERT INTO _rivetos_migrations (name) VALUES ('0023_summary_kinds.sql');
END
$rivet$;
