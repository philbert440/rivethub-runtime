DO $rivet$
BEGIN
    IF to_regclass('_rivetos_migrations') IS NOT NULL THEN
        IF EXISTS (SELECT 1 FROM _rivetos_migrations WHERE name = '0021_rh_tables.sql') THEN
            RETURN;
        END IF;
    END IF;

    DROP VIEW IF EXISTS ros_task_outcomes_v;

    ALTER TABLE IF EXISTS ros_conversations RENAME TO rh_conversations;
    ALTER TABLE IF EXISTS ros_messages RENAME TO rh_messages;
    ALTER TABLE IF EXISTS ros_summaries RENAME TO rh_summaries;
    ALTER TABLE IF EXISTS ros_summary_sources RENAME TO rh_summary_sources;
    ALTER TABLE IF EXISTS ros_tasks RENAME TO rh_tasks;
    ALTER TABLE IF EXISTS ros_delegation_runs_archive RENAME TO rh_delegation_runs_archive;
    ALTER TABLE IF EXISTS ros_wiki_topics RENAME TO rh_wiki_topics;
    ALTER TABLE IF EXISTS ros_wiki_provenance RENAME TO rh_wiki_provenance;
    ALTER TABLE IF EXISTS ros_wiki_extractions RENAME TO rh_wiki_extractions;
    ALTER TABLE IF EXISTS ros_wiki_citations RENAME TO rh_wiki_citations;
    ALTER TABLE IF EXISTS ros_wiki_redirects RENAME TO rh_wiki_redirects;
    ALTER TABLE IF EXISTS ros_message_chunks RENAME TO rh_message_chunks;
    ALTER TABLE IF EXISTS ros_agent_presets RENAME TO rh_agent_presets;
    ALTER TABLE IF EXISTS ros_tags RENAME TO rh_tags;
    ALTER TABLE IF EXISTS ros_tag_taxonomy RENAME TO rh_tag_taxonomy;

    CREATE OR REPLACE FUNCTION notify_embedding_queue() RETURNS trigger AS $func$
    DECLARE
        v_table text;
    BEGIN
        IF current_setting('rivet.defer_embed_enqueue', true) = 'on' THEN
            RETURN NEW;
        END IF;

        v_table := CASE TG_TABLE_NAME
            WHEN 'rh_messages' THEN 'ros_messages'
            WHEN 'rh_summaries' THEN 'ros_summaries'
            ELSE TG_TABLE_NAME
        END;

        IF v_table = 'ros_messages' THEN
            IF (NEW.content IS NOT NULL AND LENGTH(TRIM(NEW.content)) > 0)
               OR (NEW.tool_result IS NOT NULL AND LENGTH(TRIM(NEW.tool_result)) > 0) THEN
                PERFORM graphile_worker.add_job(
                    'embed-target',
                    json_build_object('targetTable', v_table, 'targetId', NEW.id),
                    job_key      := 'embed-' || v_table || '-' || NEW.id::text,
                    max_attempts := 5
                );
            END IF;
        ELSE
            IF NEW.content IS NOT NULL AND LENGTH(TRIM(NEW.content)) > 0 THEN
                PERFORM graphile_worker.add_job(
                    'embed-target',
                    json_build_object('targetTable', v_table, 'targetId', NEW.id),
                    job_key      := 'embed-' || v_table || '-' || NEW.id::text,
                    max_attempts := 5
                );
            END IF;
        END IF;
        RETURN NEW;
    END;
    $func$ LANGUAGE plpgsql;

    CREATE OR REPLACE FUNCTION notify_chunk_embedding_queue() RETURNS trigger AS $func$
    BEGIN
        IF NEW.embedding IS NULL
           AND NEW.embed_status IS NULL
           AND NEW.content IS NOT NULL
           AND LENGTH(TRIM(NEW.content)) > 0 THEN
            PERFORM graphile_worker.add_job(
                'embed-target',
                json_build_object('targetTable', 'ros_message_chunks', 'targetId', NEW.id),
                job_key      := 'embed-ros_message_chunks-' || NEW.id::text,
                max_attempts := 5
            );
        END IF;
        RETURN NEW;
    END;
    $func$ LANGUAGE plpgsql;

    CREATE VIEW ros_conversations AS SELECT * FROM rh_conversations;
    CREATE VIEW ros_messages AS SELECT * FROM rh_messages;
    CREATE VIEW ros_summaries AS SELECT * FROM rh_summaries;
    CREATE VIEW ros_summary_sources AS SELECT * FROM rh_summary_sources;
    CREATE VIEW ros_tasks AS SELECT * FROM rh_tasks;
    CREATE VIEW ros_delegation_runs_archive AS SELECT * FROM rh_delegation_runs_archive;
    CREATE VIEW ros_wiki_topics AS SELECT * FROM rh_wiki_topics;
    CREATE VIEW ros_wiki_provenance AS SELECT * FROM rh_wiki_provenance;
    CREATE VIEW ros_wiki_extractions AS SELECT * FROM rh_wiki_extractions;
    CREATE VIEW ros_wiki_citations AS SELECT * FROM rh_wiki_citations;
    CREATE VIEW ros_wiki_redirects AS SELECT * FROM rh_wiki_redirects;
    CREATE VIEW ros_message_chunks AS SELECT * FROM rh_message_chunks;
    CREATE VIEW ros_agent_presets AS SELECT * FROM rh_agent_presets;
    CREATE VIEW ros_tags AS SELECT * FROM rh_tags;
    CREATE VIEW ros_tag_taxonomy AS SELECT * FROM rh_tag_taxonomy;

    CREATE VIEW ros_task_outcomes_v AS
    SELECT
        t.id,
        t.agent_id,
        t.executor,
        t.executor_target,
        t.origin,
        date_trunc('day', t.completed_at AT TIME ZONE 'UTC') AS day,
        t.status,
        t.result->>'verdict'                    AS executor_verdict,
        t.eval->>'verdict'                      AS eval_verdict,
        COALESCE((t.result->>'verdict') = 'completed'
                 AND (t.eval->>'verdict') = 'refuted', false) AS diverged,
        (t.usage->>'costUsd')::numeric          AS cost_usd,
        t.eval_attempt,
        t.duration_ms
    FROM rh_tasks t
    WHERE t.status IN ('completed','failed','killed','timeout')
      AND t.origin <> 'eval'
      AND (t.spec->>'auditOnly') IS DISTINCT FROM 'true'
      AND jsonb_array_length(t.acceptance_criteria) > 0;

    INSERT INTO _rivetos_migrations (name) VALUES ('0021_rh_tables.sql');
END
$rivet$;
