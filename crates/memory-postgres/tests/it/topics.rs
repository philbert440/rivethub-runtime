use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::TopicStore;
use memory_postgres::PostgresMemory;
use sqlx::PgPool;
use testkit::topic_pages;
use testkit::topic_pages::PageStores;
use testkit::topics;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn page_stores(
    pool: PgPool,
) -> Result<
    (
        tempfile::TempDir,
        PostgresMemory,
        memory_postgres::PostgresQueue,
    ),
    Box<dyn std::error::Error>,
> {
    testkit::install_graphile_worker(&pool).await?;
    let directory = tempfile::tempdir()?;
    let memory = PostgresMemory::new(pool).with_wiki_directory(directory.path().join("wiki"));
    let queue = memory.queue().await?;
    Ok((directory, memory, queue))
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_new_root_topic_gets_an_empty_bound_page(pool: PgPool) -> TestResult {
    let (_directory, memory, queue) = page_stores(pool).await?;
    topic_pages::a_new_root_topic_gets_an_empty_bound_page(PageStores {
        topics: &memory,
        pages: &memory,
        decisions: &memory,
        wiki: &memory,
        queue: &queue,
    })
    .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_rename_keeps_the_page_slug_and_follows_the_name(pool: PgPool) -> TestResult {
    let (_directory, memory, queue) = page_stores(pool).await?;
    topic_pages::a_rename_keeps_the_page_slug_and_follows_the_name(PageStores {
        topics: &memory,
        pages: &memory,
        decisions: &memory,
        wiki: &memory,
        queue: &queue,
    })
    .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_merge_folds_the_page_and_redirects_its_slug(pool: PgPool) -> TestResult {
    let (_directory, memory, queue) = page_stores(pool).await?;
    topic_pages::a_merge_folds_the_page_and_redirects_its_slug(PageStores {
        topics: &memory,
        pages: &memory,
        decisions: &memory,
        wiki: &memory,
        queue: &queue,
    })
    .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn sub_topics_are_sections_of_the_root_page(pool: PgPool) -> TestResult {
    let (_directory, memory, queue) = page_stores(pool).await?;
    topic_pages::sub_topics_are_sections_of_the_root_page(PageStores {
        topics: &memory,
        pages: &memory,
        decisions: &memory,
        wiki: &memory,
        queue: &queue,
    })
    .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn syncing_twice_changes_nothing(pool: PgPool) -> TestResult {
    let (_directory, memory, queue) = page_stores(pool).await?;
    topic_pages::syncing_twice_changes_nothing(PageStores {
        topics: &memory,
        pages: &memory,
        decisions: &memory,
        wiki: &memory,
        queue: &queue,
    })
    .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn topics_are_created_found_and_renamed(pool: PgPool) -> TestResult {
    topics::topics_are_created_found_and_renamed(&PostgresMemory::new(pool)).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn slugs_and_aliases_are_unique_across_topics(pool: PgPool) -> TestResult {
    topics::slugs_and_aliases_are_unique_across_topics(&PostgresMemory::new(pool)).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn sub_topics_stay_one_level_deep(pool: PgPool) -> TestResult {
    topics::sub_topics_stay_one_level_deep(&PostgresMemory::new(pool)).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn placements_keep_your_choice(pool: PgPool) -> TestResult {
    topics::placements_keep_your_choice(&PostgresMemory::new(pool)).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn merges_move_placements_aliases_and_slugs(pool: PgPool) -> TestResult {
    topics::merges_move_placements_aliases_and_slugs(&PostgresMemory::new(pool)).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn redirects_follow_a_second_merge(pool: PgPool) -> TestResult {
    topics::redirects_follow_a_second_merge(&PostgresMemory::new(pool)).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn suggestions_hold_until_the_session_is_placed(pool: PgPool) -> TestResult {
    topics::suggestions_hold_until_the_session_is_placed(&PostgresMemory::new(pool)).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn startup_context_is_kept_per_session(pool: PgPool) -> TestResult {
    topics::startup_context_is_kept_per_session(&PostgresMemory::new(pool)).await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn listings_count_sessions_and_take_last_activity_from_conversations(
    pool: PgPool,
) -> TestResult {
    let store = PostgresMemory::new(pool.clone());
    let busy = store.create(topics::new_topic("Busy", &[], None)?).await?;
    let quiet = store.create(topics::new_topic("Quiet", &[], None)?).await?;
    store.create(topics::new_topic("Empty", &[], None)?).await?;
    let first = topics::session("00000000-0000-4000-8000-000000000011")?;
    let second = topics::session("00000000-0000-4000-8000-000000000012")?;
    let uncaptured = topics::session("00000000-0000-4000-8000-000000000013")?;
    for (session, topic) in [
        (&first, busy.id),
        (&second, busy.id),
        (&uncaptured, quiet.id),
    ] {
        store
            .place(Placement {
                session: session.clone(),
                topic,
                by: PlacementSource::You,
                at: topics::at(1),
            })
            .await?;
    }
    for (key, agent, updated) in [
        (first.as_str(), "opus", "2026-10-02T09:00:00Z"),
        (first.as_str(), "grok", "2026-10-03T09:00:00Z"),
        (second.as_str(), "opus", "2026-10-01T09:00:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO rh_conversations (session_key, agent, created_at, updated_at)
             VALUES ($1, $2, $3::timestamptz, $3::timestamptz)",
        )
        .bind(key)
        .bind(agent)
        .bind(updated)
        .execute(&pool)
        .await?;
    }
    let listed = store.list().await?;
    let summary: Vec<(&str, u64, Option<String>)> = listed
        .iter()
        .map(|listing| {
            (
                listing.topic.name.as_str(),
                listing.session_count,
                listing.last_active.map(|at| at.to_rfc3339()),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("Busy", 2, Some("2026-10-03T09:00:00+00:00".to_owned())),
            ("Empty", 0, None),
            ("Quiet", 1, None),
        ]
    );
    let sessions = store.sessions(busy.id).await?;
    assert_eq!(
        sessions
            .iter()
            .map(|placed| placed.placement.session.clone())
            .collect::<Vec<_>>(),
        [first, second]
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn decisions_are_the_topic_page_decisions_section(pool: PgPool) -> TestResult {
    let (_directory, memory, queue) = page_stores(pool).await?;
    testkit::decisions::decisions_are_the_topic_page_decisions_section(PageStores {
        topics: &memory,
        pages: &memory,
        decisions: &memory,
        wiki: &memory,
        queue: &queue,
    })
    .await?;
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_deleted_topic_leaves_its_sessions_forming_and_its_page_unbound(
    pool: PgPool,
) -> TestResult {
    let (_directory, memory, queue) = page_stores(pool).await?;
    topic_pages::a_deleted_topic_leaves_its_sessions_forming_and_its_page_unbound(PageStores {
        topics: &memory,
        pages: &memory,
        decisions: &memory,
        wiki: &memory,
        queue: &queue,
    })
    .await
}
