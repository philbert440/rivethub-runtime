use migrate::POSTGRES;
use sqlx::PgPool;
use sqlx::Row as _;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn empty_database_reaches_the_recorded_shape(pool: PgPool) -> TestResult {
    POSTGRES.run(&pool).await?;

    let tracked = sqlx::query("SELECT count(*)::bigint AS n FROM _rivetos_migrations")
        .fetch_one(&pool)
        .await?;
    assert_eq!(tracked.try_get::<i64, _>("n").unwrap(), 33);

    let applied = sqlx::query("SELECT count(*)::bigint AS n FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await?;
    assert_eq!(applied.try_get::<i64, _>("n").unwrap(), 33);

    migrate::postgres_pool(&pool).await?;

    for table in [
        "rh_conversations",
        "rh_messages",
        "rh_summaries",
        "rh_summary_sources",
        "rh_tasks",
        "rh_delegation_runs_archive",
        "rh_wiki_topics",
        "rh_wiki_provenance",
        "rh_wiki_extractions",
        "rh_wiki_citations",
        "rh_wiki_redirects",
        "rh_message_chunks",
        "rh_agent_presets",
        "rh_tags",
        "rh_tag_taxonomy",
        "rh_topics",
        "rh_topic_aliases",
        "rh_topic_redirects",
        "rh_session_placements",
        "rh_session_suggestions",
        "rh_hub_state",
        "rh_session_history",
    ] {
        let present = sqlx::query("SELECT to_regclass($1) IS NOT NULL AS present")
            .bind(format!("public.{table}"))
            .fetch_one(&pool)
            .await?;
        assert!(
            present.try_get::<bool, _>("present").unwrap(),
            "{table} missing"
        );
    }

    for view in [
        "ros_conversations",
        "ros_messages",
        "ros_summaries",
        "ros_tasks",
        "ros_tags",
        "ros_task_outcomes_v",
    ] {
        let kind: String =
            sqlx::query("SELECT relkind::text AS kind FROM pg_class WHERE oid = to_regclass($1)")
                .bind(format!("public.{view}"))
                .fetch_one(&pool)
                .await?
                .try_get("kind")
                .unwrap();
        assert_eq!(kind, "v", "{view} is not a view (relkind {kind})");
    }

    for index in [
        "ux_ros_conversations_session_agent",
        "idx_ros_messages_fts",
        "idx_ros_messages_trgm",
        "idx_ros_messages_tool_result_trgm",
        "idx_ros_summaries_trgm",
        "ros_messages_embedding_hnsw",
        "ros_summaries_embedding_hnsw",
        "ux_ros_tags_entity_kv",
        "idx_ros_conversations_task",
        "idx_ros_tags_removal_state",
    ] {
        let present =
            sqlx::query("SELECT count(*)::bigint AS n FROM pg_indexes WHERE indexname = $1")
                .bind(index)
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            present.try_get::<i64, _>("n").unwrap(),
            1,
            "index {index} missing"
        );
    }

    let removal = sqlx::query(
        "SELECT count(*)::bigint AS n FROM information_schema.columns
          WHERE table_name = 'rh_tags' AND column_name = any($1)",
    )
    .bind(["removal_state", "removal_reason"])
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        removal.try_get::<i64, _>("n").unwrap(),
        2,
        "0020 tag-removal columns missing"
    );

    let defaults = sqlx::query(
        "SELECT column_name, column_default FROM information_schema.columns
          WHERE table_name = 'rh_tags' AND column_name = 'removal_reason'",
    )
    .fetch_one(&pool)
    .await?;
    let default_text: Option<String> = defaults.try_get("column_default").unwrap();
    assert_eq!(
        default_text.as_deref(),
        Some("''::text"),
        "removal_reason must default to the empty string"
    );

    for function in [
        "notify_embedding_queue",
        "notify_chunk_embedding_queue",
        "notify_task_done",
    ] {
        let present = sqlx::query(
            "SELECT count(*)::bigint AS n FROM pg_proc
              WHERE proname = $1 AND pronamespace = 'public'::regnamespace",
        )
        .bind(function)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            present.try_get::<i64, _>("n").unwrap(),
            1,
            "function {function} missing"
        );
    }

    sqlx::query(
        "INSERT INTO rh_conversations (session_key, agent) VALUES ('migrate:smoke', 'rivet')",
    )
    .execute(&pool)
    .await?;
    let via_view = sqlx::query(
        "SELECT count(*)::bigint AS n FROM ros_conversations WHERE session_key = 'migrate:smoke'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(via_view.try_get::<i64, _>("n").unwrap(), 1);
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn rivetos_migrated_database_gets_only_0021(pool: PgPool) -> TestResult {
    for migration in POSTGRES.migrations.iter().take(20) {
        let sql: &'static str = Box::leak(migration.sql.as_str().to_string().into_boxed_str());
        sqlx::raw_sql(sql).execute(&pool).await?;
    }
    let tracked = sqlx::query("SELECT count(*)::bigint AS n FROM _rivetos_migrations")
        .fetch_one(&pool)
        .await?;
    assert_eq!(tracked.try_get::<i64, _>("n").unwrap(), 20);

    sqlx::query(
        "INSERT INTO ros_conversations (session_key, agent, task_id)
         VALUES ('ts:legacy', 'rivet', '11111111-2222-3333-4444-555555555555')",
    )
    .execute(&pool)
    .await?;
    let removal = sqlx::query(
        "SELECT count(*)::bigint AS n FROM information_schema.columns
          WHERE table_name = 'ros_tags' AND column_name = 'removal_state'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        removal.try_get::<i64, _>("n").unwrap(),
        1,
        "0020_tag_removals must have applied before 0021"
    );

    POSTGRES.run(&pool).await?;

    let tracked = sqlx::query("SELECT count(*)::bigint AS n FROM _rivetos_migrations")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        tracked.try_get::<i64, _>("n").unwrap(),
        33,
        "0021 renames the tables, 0022 adds the summaries trigram index, 0023 rewrites legacy summary kinds, 0024 adds topics, 0025 adds topic suggestions, 0026 adds the hub state table, 0027 binds wiki pages to topics, 0028 keeps each session's startup context, 0029 records how it was delivered, 0030 adds topic decisions, 0031 records closed topics and sessions marked as history, 0032 adds topic kinds and entities, and 0033 files conversations and keeps filing corrections"
    );
    let summaries_trgm =
        sqlx::query("SELECT indexdef FROM pg_indexes WHERE indexname = 'idx_ros_summaries_trgm'")
            .fetch_one(&pool)
            .await?;
    let indexdef: String = summaries_trgm.try_get("indexdef").unwrap();
    assert!(
        indexdef.contains("gin") && indexdef.contains("gin_trgm_ops"),
        "summaries trigram index must be a gin_trgm index: {indexdef}"
    );

    let legacy =
        sqlx::query("SELECT id, task_id FROM ros_conversations WHERE session_key = 'ts:legacy'")
            .fetch_one(&pool)
            .await?;
    let id: uuid::Uuid = legacy.try_get("id").unwrap();
    let task: Option<uuid::Uuid> = legacy.try_get("task_id").unwrap();
    assert_eq!(
        task,
        Some(uuid::uuid!("11111111-2222-3333-4444-555555555555"))
    );

    let renamed =
        sqlx::query("SELECT id, task_id FROM rh_conversations WHERE session_key = 'ts:legacy'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(renamed.try_get::<uuid::Uuid, _>("id").unwrap(), id);

    let removal = sqlx::query(
        "SELECT count(*)::bigint AS n FROM information_schema.columns
          WHERE table_name = 'rh_tags' AND column_name = 'removal_state'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(removal.try_get::<i64, _>("n").unwrap(), 1);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn migration_0021_views_mirror_the_rh_tables(pool: PgPool) -> TestResult {
    POSTGRES.run(&pool).await?;
    let queue = testkit::queue_present(&pool).await?;

    let mut tx = testkit::capture_tx(&pool).await?;
    sqlx::query(
        "INSERT INTO rh_conversations (session_key, agent, title) VALUES ('view:one', 'rivet', 't1')",
    )
    .execute(&mut *tx)
    .await?;
    let conversation: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM rh_conversations WHERE session_key = 'view:one'")
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query(
        "INSERT INTO rh_messages (conversation_id, agent, channel, role, content)
         VALUES ($1, 'rivet', 'cli', 'user', 'through the view')",
    )
    .bind(conversation)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let through_view: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ros_messages
          WHERE conversation_id = $1 AND content = 'through the view'",
    )
    .bind(conversation)
    .fetch_one(&pool)
    .await?;
    assert_eq!(through_view, 1);

    let view_row = sqlx::query("SELECT title FROM ros_conversations WHERE id = $1")
        .bind(conversation)
        .fetch_one(&pool)
        .await?;
    assert_eq!(view_row.try_get::<String, _>("title").unwrap(), "t1");

    let mut tx = testkit::capture_tx(&pool).await?;
    let insert_through_view: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO ros_conversations (session_key, agent, title)
         VALUES ('view:two', 'rivet', 't2') RETURNING id",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO ros_messages (conversation_id, agent, channel, role, content)
         VALUES ($1, 'rivet', 'cli', 'user', 'job fires through the view')",
    )
    .bind(insert_through_view)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let stored = sqlx::query(
        "SELECT count(*)::bigint AS n FROM rh_conversations WHERE id = $1 AND title = 't2'",
    )
    .bind(insert_through_view)
    .fetch_one(&pool)
    .await?;
    assert_eq!(stored.try_get::<i64, _>("n").unwrap(), 1);

    if queue {
        let job: Option<String> = sqlx::query_scalar(
            "SELECT t.identifier FROM graphile_worker._private_jobs j
              JOIN graphile_worker._private_tasks t ON t.id = j.task_id",
        )
        .fetch_optional(&pool)
        .await?
        .flatten();
        assert_eq!(job.as_deref(), Some("embed-target"));
    }
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_missing_tracking_row_is_a_typed_error(pool: PgPool) -> TestResult {
    sqlx::query("DELETE FROM _rivetos_migrations WHERE name = '0004_task_eval.sql'")
        .execute(&pool)
        .await?;
    match migrate::postgres_pool(&pool).await {
        Err(migrate::Error::TrackingMissing { missing }) => {
            assert!(
                missing.contains("0004_task_eval.sql"),
                "the error must name the missing row: {missing}"
            );
            assert!(
                !missing.contains("0001_baseline.sql"),
                "present rows must not be named: {missing}"
            );
        }
        other => panic!("expected TrackingMissing, got {other:?}"),
    }
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn migration_0027_binds_a_page_to_the_root_topic_with_its_slug(pool: PgPool) -> TestResult {
    for migration in POSTGRES.migrations.iter().take(26) {
        let sql: &'static str = Box::leak(migration.sql.as_str().to_string().into_boxed_str());
        sqlx::raw_sql(sql).execute(&pool).await?;
    }
    sqlx::raw_sql(
        "INSERT INTO rh_topics (id, slug, name, parent_id, created_at, updated_at) VALUES
           ('00000000-0000-0000-0000-000000000001', 'mesh', 'Mesh', NULL, now(), now()),
           ('00000000-0000-0000-0000-000000000002', 'certs', 'Certs',
            '00000000-0000-0000-0000-000000000001', now(), now());
         INSERT INTO rh_wiki_topics (slug, title) VALUES ('mesh', 'Mesh'), ('certs', 'Certs');",
    )
    .execute(&pool)
    .await?;

    let last = POSTGRES.migrations.get(26).unwrap();
    let sql: &'static str = Box::leak(last.sql.as_str().to_string().into_boxed_str());
    sqlx::raw_sql(sql).execute(&pool).await?;

    let bound: Vec<(String, Option<sqlx::types::Uuid>)> =
        sqlx::query_as("SELECT slug, topic_id FROM rh_wiki_topics ORDER BY slug")
            .fetch_all(&pool)
            .await?;
    assert_eq!(
        bound,
        vec![
            ("certs".to_owned(), None),
            (
                "mesh".to_owned(),
                Some("00000000-0000-0000-0000-000000000001".parse()?)
            ),
        ],
        "only a root topic takes the page with its slug"
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn migration_0028_keeps_one_startup_context_per_session(pool: PgPool) -> TestResult {
    let insert = "INSERT INTO rh_session_startup (session_id, topic_id, tokens, sections, created_at)
                  VALUES ('claude-code:one', '00000000-0000-0000-0000-000000000001', 0, '[]', now())";
    sqlx::raw_sql(insert).execute(&pool).await?;
    assert!(
        sqlx::raw_sql(insert).execute(&pool).await.is_err(),
        "a session keeps one startup context"
    );
    let last = POSTGRES.migrations.get(27).unwrap();
    let sql: &'static str = Box::leak(last.sql.as_str().to_string().into_boxed_str());
    sqlx::raw_sql(sql).execute(&pool).await?;
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM rh_session_startup")
        .fetch_one(&pool)
        .await?;
    assert_eq!(kept, 1, "a second run of 0028 changes nothing");
    Ok(())
}
