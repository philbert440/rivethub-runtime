use crate::common;

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use kernel::owner::Owner;
use kernel::tenancy::Scope;
use kernel::tenancy::Space;
use kernel::tenancy::SpaceId;
use kernel::tenancy::SpaceKind;
use kernel::tenancy::TenancyStore as _;
use kernel::tenancy::Viewer;
use kernel::tenancy::Visibility;
use kernel::topics::NewTopic;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Topic;
use kernel::topics::TopicId;
use kernel::topics::TopicKind;
use kernel::topics::TopicName;
use kernel::topics::TopicStore as _;
use memory_postgres::CaptureOptions;
use memory_postgres::PostgresMemory;
use memory_postgres::PostgresTenancy;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use wire::capture::CaptureBatchRequest;
use wire::capture::CaptureMessage;
use wire::message::Role;
use wire::session_id::SessionId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIRST: &str = "claude-code:00000000-0000-4000-8000-0000000000b1";
const SECOND: &str = "claude-code:00000000-0000-4000-8000-0000000000b2";
const LATER: &str = "claude-code:00000000-0000-4000-8000-0000000000b3";
const PROBE: &str = "rh_rls_probe";

fn batch(session: &str, text: &str) -> CaptureBatchRequest {
    CaptureBatchRequest {
        session_key: session.to_owned(),
        agent: "rivet".to_owned(),
        channel: Some("cli".to_owned()),
        title: None,
        settings: None,
        task_id: None,
        finalize: None,
        created_at: None,
        updated_at: None,
        messages: vec![CaptureMessage {
            event_id: format!("{session}-{text}"),
            role: Role::User,
            content: text.to_owned(),
            tool_name: None,
            tool_args: None,
            tool_result: None,
            metadata: None,
            created_at: None,
        }],
    }
}

fn household() -> Result<SpaceId, Box<dyn std::error::Error>> {
    Ok(SpaceId::new("household".to_owned())?)
}

fn viewer(name: &str) -> Result<Viewer, Box<dyn std::error::Error>> {
    Ok(Viewer::new(
        Owner::new(name.to_owned())?,
        vec![household()?],
    ))
}

async fn probe(pool: &PgPool) -> Result<PgPool, Box<dyn std::error::Error>> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DO $$ BEGIN CREATE ROLE {PROBE} NOLOGIN; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
    )))
    .execute(pool)
    .await?;
    for grant in [
        format!("GRANT USAGE ON SCHEMA public TO {PROBE}"),
        format!("GRANT ALL ON ALL TABLES IN SCHEMA public TO {PROBE}"),
        format!("GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO {PROBE}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(grant))
            .execute(pool)
            .await?;
    }
    let options = pool
        .connect_options()
        .as_ref()
        .clone()
        .options([("role", PROBE)]);
    Ok(
        memory_postgres::scoped(PgPoolOptions::new().max_connections(2))
            .connect_with(options)
            .await?,
    )
}

async fn seed(
    pool: &PgPool,
) -> Result<(PostgresMemory, PostgresTenancy), Box<dyn std::error::Error>> {
    common::install_graphile(pool).await?;
    let store = PostgresMemory::new(pool.clone());
    let alice = Owner::new("alice".to_owned())?;
    for (session, text) in [(FIRST, "the first session"), (SECOND, "the second session")] {
        store
            .capture_batch(&batch(session, text), &CaptureOptions::owned_by(&alice))
            .await?;
    }
    let tenancy = PostgresTenancy::new(pool.clone());
    tenancy
        .sync_spaces(&[Space {
            id: household()?,
            kind: SpaceKind::Household,
            name: "Household".to_owned(),
            members: vec![alice, Owner::new("bob".to_owned())?],
        }])
        .await?;
    Ok((store, tenancy))
}

async fn visible(pool: &PgPool, scope: Scope, table: &str) -> Result<i64, sqlx::Error> {
    let sql = format!("SELECT count(*) FROM {table}");
    scope
        .run(async {
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql))
                .fetch_one(pool)
                .await
        })
        .await
}

async fn unscoped(pool: &PgPool, table: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn without_a_scope_the_store_sees_nothing(pool: PgPool) -> TestResult {
    seed(&pool).await?;
    let probe = probe(&pool).await?;
    for table in ["rh_conversations", "rh_messages", "rh_spaces"] {
        assert_eq!(unscoped(&probe, table).await?, 0, "{table}");
        assert_eq!(visible(&probe, Scope::Closed, table).await?, 0, "{table}");
    }
    assert_eq!(
        visible(&probe, Scope::Maintenance, "rh_conversations").await?,
        2
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_shared_session_shows_to_the_space_and_a_private_one_does_not(
    pool: PgPool,
) -> TestResult {
    seed(&pool).await?;
    let probe = probe(&pool).await?;
    let alice = viewer("alice")?;
    let bob = viewer("bob")?;
    let owners = Scope::Viewer(alice.clone());
    let members = Scope::Viewer(bob.clone());
    assert_eq!(visible(&probe, owners.clone(), "rh_messages").await?, 2);
    assert_eq!(visible(&probe, members.clone(), "rh_messages").await?, 0);

    let tenancy = PostgresTenancy::new(probe.clone());
    let second = SessionId::try_from(SECOND.to_owned())?;
    let shared = Visibility::Space(household()?);
    let marked = owners
        .clone()
        .run(tenancy.mark_session(&alice, &second, Some(&shared), Utc::now()))
        .await?;
    assert_eq!(marked.effective, shared);
    assert_eq!(
        visible(&probe, members.clone(), "rh_conversations").await?,
        1
    );
    assert_eq!(visible(&probe, members.clone(), "rh_messages").await?, 1);

    let refused = members
        .clone()
        .run(tenancy.mark_session(&bob, &second, Some(&Visibility::Private), Utc::now()))
        .await;
    assert!(refused.is_err(), "only the owner marks a session");
    let renamed = members
        .clone()
        .run(async {
            sqlx::query("UPDATE rh_conversations SET title = 'taken' WHERE session_key = $1")
                .bind(SECOND)
                .execute(&probe)
                .await
        })
        .await?;
    assert_eq!(
        renamed.rows_affected(),
        0,
        "a member reads a shared row but cannot write it"
    );

    owners
        .clone()
        .run(tenancy.mark_session(&alice, &second, Some(&Visibility::Private), Utc::now()))
        .await?;
    assert_eq!(visible(&probe, members, "rh_messages").await?, 0);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_session_follows_its_topic_unless_it_is_marked(pool: PgPool) -> TestResult {
    let (store, _) = seed(&pool).await?;
    let probe = probe(&pool).await?;
    let alice = viewer("alice")?;
    let members = Scope::Viewer(viewer("bob")?);
    let now = Utc::now();
    let topic = store
        .create(Topic::create(
            TopicId::random(),
            NewTopic {
                kind: TopicKind::Work,
                name: TopicName::new("Kitchen")?,
                aliases: Vec::new(),
                parent: None,
                directory: None,
                owner: Some(alice.principal().clone()),
            },
            now,
        )?)
        .await?;
    for session in [FIRST, SECOND] {
        store
            .place(Placement {
                session: SessionId::try_from(session.to_owned())?,
                topic: topic.id,
                by: PlacementSource::You,
                at: now,
                owner: Some(alice.principal().clone()),
            })
            .await?;
    }
    let tenancy = PostgresTenancy::new(probe.clone());
    let shared = Visibility::Space(household()?);
    let owners = Scope::Viewer(alice.clone());
    let marked = owners
        .clone()
        .run(tenancy.mark_topic(&alice, topic.id, Some(&shared)))
        .await?;
    assert_eq!(marked.effective, shared);
    assert_eq!(
        visible(&probe, members.clone(), "rh_topics WHERE kind = 'work'").await?,
        1
    );
    assert_eq!(
        visible(&probe, members.clone(), "rh_conversations").await?,
        2
    );
    assert_eq!(
        visible(&probe, members.clone(), "rh_session_placements").await?,
        2
    );

    let first = SessionId::try_from(FIRST.to_owned())?;
    owners
        .clone()
        .run(tenancy.mark_session(&alice, &first, Some(&Visibility::Private), Utc::now()))
        .await?;
    assert_eq!(
        visible(&probe, members.clone(), "rh_conversations").await?,
        1
    );

    owners
        .clone()
        .run(tenancy.mark_topic(&alice, topic.id, None))
        .await?;
    let second = SessionId::try_from(SECOND.to_owned())?;
    owners
        .run(tenancy.mark_session(&alice, &second, Some(&shared), Utc::now()))
        .await?;
    assert_eq!(
        visible(&probe, members.clone(), "rh_topics WHERE kind = 'work'").await?,
        0
    );
    assert_eq!(
        visible(&probe, members.clone(), "rh_conversations").await?,
        1
    );
    assert_eq!(
        visible(&probe, members, "rh_session_placements").await?,
        0,
        "a shared session in a private topic shows without its topic"
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_shared_default_applies_to_sessions_started_after_it(pool: PgPool) -> TestResult {
    let (store, tenancy) = seed(&pool).await?;
    let started: DateTime<Utc> = sqlx::query_scalar("SELECT max(created_at) FROM rh_conversations")
        .fetch_one(&pool)
        .await?;
    let alice = Owner::new("alice".to_owned())?;
    let shared = Visibility::Space(household()?);
    tenancy
        .set_default_visibility(&alice, &shared, started + Duration::microseconds(1))
        .await?;
    store
        .capture_batch(
            &batch(LATER, "a later session"),
            &CaptureOptions::owned_by(&alice),
        )
        .await?;
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT session_key, visibility FROM rh_conversations ORDER BY session_key")
            .fetch_all(&pool)
            .await?;
    assert_eq!(
        rows,
        vec![
            (FIRST.to_owned(), "private".to_owned()),
            (SECOND.to_owned(), "private".to_owned()),
            (LATER.to_owned(), "household".to_owned()),
        ]
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn text_search_keeps_its_index_under_row_security(pool: PgPool) -> TestResult {
    seed(&pool).await?;
    let probe = probe(&pool).await?;
    let plan: Vec<String> = Scope::Viewer(viewer("bob")?)
        .run(async {
            let mut tx = probe.begin().await?;
            sqlx::query("SET LOCAL enable_seqscan = off")
                .execute(&mut *tx)
                .await?;
            sqlx::query_scalar(
                "EXPLAIN (COSTS off) SELECT id FROM rh_messages \
                 WHERE content_tsv @@ plainto_tsquery('english', 'session')",
            )
            .fetch_all(&mut *tx)
            .await
        })
        .await?;
    assert!(
        plan.iter()
            .any(|line| line.contains("idx_ros_messages_fts")),
        "{plan:?}"
    );
    Ok(())
}
