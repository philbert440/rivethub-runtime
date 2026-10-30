#![cfg(test)]

use crate::wiki_support;

use kernel::wiki::builder::{WikiBuilderStore, WikiRouting};
use kernel::wiki::page::{WikiAction, WikiPatch};
use memory_postgres::PostgresMemory;
use sqlx::PgPool;
use wiki_support::{TestResult, now, patch, seed, slug};

fn proposal(proposed: &str, title: &str) -> WikiPatch {
    WikiPatch {
        slug: slug(proposed),
        title: title.to_owned(),
        aliases: Vec::new(),
        entities: Vec::new(),
        summary: format!("{title} was discussed."),
        article: None,
        history_entry: None,
        ..patch()
    }
}

async fn store_with_deploys(
    pool: &PgPool,
    wiki: &tempfile::TempDir,
) -> Result<PostgresMemory, Box<dyn std::error::Error>> {
    let store = PostgresMemory::new(pool.clone()).with_wiki_directory(wiki.path().to_path_buf());
    let job = seed(pool).await?;
    store
        .apply_wiki(&job, vec![patch()], &WikiRouting::default(), now())
        .await?;
    Ok(store)
}

async fn apply(
    store: &PostgresMemory,
    pool: &PgPool,
    patches: Vec<WikiPatch>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let job = seed(pool).await?;
    Ok(store
        .apply_wiki(&job, patches, &WikiRouting::default(), now())
        .await?)
}

async fn slugs(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT slug FROM rh_wiki_topics ORDER BY slug COLLATE \"C\"")
        .fetch_all(pool)
        .await
}

async fn aliases(pool: &PgPool, topic: &str) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT aliases FROM rh_wiki_topics WHERE slug = $1")
        .bind(topic)
        .fetch_one(pool)
        .await
}

async fn title(pool: &PgPool, topic: &str) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT title FROM rh_wiki_topics WHERE slug = $1")
        .bind(topic)
        .fetch_one(pool)
        .await
}

async fn redirect_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM rh_wiki_redirects")
        .fetch_one(pool)
        .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_exact_slug_turns_a_create_into_an_update(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    let touched = apply(
        &store,
        &pool,
        vec![proposal("acmekit-deploys", "AcmeKit deploys")],
    )
    .await?;
    assert_eq!(touched, vec!["acmekit-deploys"]);
    assert_eq!(slugs(&pool).await?, vec!["acmekit-deploys"]);
    assert_eq!(
        aliases(&pool, "acmekit-deploys").await?,
        vec!["acmekit-rollout"]
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_follows_redirects_and_keeps_the_proposed_slug_as_alias(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    sqlx::query(
        "INSERT INTO rh_wiki_redirects (from_slug, to_slug) VALUES ('legacy-shipping', 'acmekit-deploys')",
    )
    .execute(&pool)
    .await?;
    let touched = apply(
        &store,
        &pool,
        vec![proposal("legacy-shipping", "Legacy shipping")],
    )
    .await?;
    assert_eq!(touched, vec!["acmekit-deploys"]);
    assert_eq!(slugs(&pool).await?, vec!["acmekit-deploys"]);
    assert_eq!(
        aliases(&pool, "acmekit-deploys").await?,
        vec!["acmekit-rollout", "legacy-shipping"]
    );
    assert_eq!(redirect_count(&pool).await?, 1);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_resolves_an_alias(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    let touched = apply(
        &store,
        &pool,
        vec![proposal("acmekit-rollout", "AcmeKit rollout")],
    )
    .await?;
    assert_eq!(touched, vec!["acmekit-deploys"]);
    assert_eq!(slugs(&pool).await?, vec!["acmekit-deploys"]);
    assert_eq!(title(&pool, "acmekit-deploys").await?, "AcmeKit deploys");
    assert_eq!(redirect_count(&pool).await?, 0);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_folds_a_product_prefixed_plural_into_the_same_topic(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    let touched = apply(
        &store,
        &pool,
        vec![proposal(
            "rivethub-acmekit-deploy",
            "RivetHub AcmeKit deploy",
        )],
    )
    .await?;
    assert_eq!(touched, vec!["acmekit-deploys"]);
    assert_eq!(slugs(&pool).await?, vec!["acmekit-deploys"]);
    assert!(
        aliases(&pool, "acmekit-deploys")
            .await?
            .contains(&"rivethub-acmekit-deploy".to_owned())
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_keeps_pages_that_only_share_entities_apart(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    let chain = [
        (
            "monday-cutover",
            "Monday cutover",
            ["project:acmekit", "host:build-box"],
        ),
        (
            "label-printer-jam",
            "Label printer jam",
            ["host:build-box", "person:dana"],
        ),
        ("lunch-rota", "Lunch rota", ["person:dana", "place:canteen"]),
    ];
    let mut touched = Vec::new();
    for (proposed, title, entities) in chain {
        let mut unrelated = proposal(proposed, title);
        unrelated.entities = entities.map(str::to_owned).to_vec();
        touched.extend(apply(&store, &pool, vec![unrelated]).await?);
    }
    assert_eq!(
        touched,
        vec!["monday-cutover", "label-printer-jam", "lunch-rota"]
    );
    assert_eq!(
        slugs(&pool).await?,
        vec![
            "acmekit-deploys",
            "label-printer-jam",
            "lunch-rota",
            "monday-cutover"
        ]
    );
    assert_eq!(title(&pool, "acmekit-deploys").await?, "AcmeKit deploys");
    let entities: Vec<String> =
        sqlx::query_scalar("SELECT entities FROM rh_wiki_topics WHERE slug = 'acmekit-deploys'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(entities, vec!["project:acmekit", "tool:deploy_script"]);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_folds_a_session_shard_into_its_stem_parent_as_a_note(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    let before: (String, String) = sqlx::query_as(
        "SELECT current_state, article FROM rh_wiki_topics WHERE slug = 'acmekit-deploys'",
    )
    .fetch_one(&pool)
    .await?;
    let mut shard = proposal("acmekit-deploys-session-12", "Session twelve tagging");
    shard.article = Some("# Session twelve tagging\nThe tagger reads its env prefix.".to_owned());
    shard.entities = vec!["tool:tagger".to_owned()];
    let touched = apply(&store, &pool, vec![shard]).await?;
    assert_eq!(touched, vec!["acmekit-deploys"]);
    assert_eq!(slugs(&pool).await?, vec!["acmekit-deploys"]);
    let after: (String, String, String, Vec<String>) = sqlx::query_as(
        "SELECT current_state, article, title, entities FROM rh_wiki_topics WHERE slug = 'acmekit-deploys'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!((after.0, after.1), before);
    assert_eq!(after.2, "AcmeKit deploys");
    assert_eq!(after.3, vec!["project:acmekit", "tool:deploy_script"]);
    let page = std::fs::read_to_string(wiki.path().join("topics/acmekit-deploys.md"))?;
    assert!(
        page.contains("Session twelve tagging")
            && page.contains("The tagger reads its env prefix.")
    );
    assert!(
        aliases(&pool, "acmekit-deploys")
            .await?
            .contains(&"acmekit-deploys-session-12".to_owned())
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_merges_a_strong_search_hit(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    let touched = apply(&store, &pool, vec![proposal("blue-green", "Blue green")]).await?;
    assert_eq!(touched, vec!["acmekit-deploys"]);
    assert_eq!(slugs(&pool).await?, vec!["acmekit-deploys"]);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_creates_below_the_search_threshold(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    let query = "blue-greenish Blue greenish";
    let trigram: bool = sqlx::query_scalar(
        "SELECT $1 <% search_text FROM rh_wiki_topics WHERE slug = 'acmekit-deploys'",
    )
    .bind(query)
    .fetch_one(&pool)
    .await?;
    let full_text: bool = sqlx::query_scalar(
        "SELECT content_tsv @@ websearch_to_tsquery('english', $1) FROM rh_wiki_topics WHERE slug = 'acmekit-deploys'",
    )
    .bind(query)
    .fetch_one(&pool)
    .await?;
    assert!(trigram && !full_text, "one retriever alone scores 1/21");
    let touched = apply(
        &store,
        &pool,
        vec![proposal("blue-greenish", "Blue greenish")],
    )
    .await?;
    assert_eq!(touched, vec!["blue-greenish"]);
    assert_eq!(
        slugs(&pool).await?,
        vec!["acmekit-deploys", "blue-greenish"]
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_update_of_an_unknown_topic_creates_it(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = PostgresMemory::new(pool.clone()).with_wiki_directory(wiki.path().to_path_buf());
    let mut proposed = proposal("orphan-notes", "Orphan notes");
    proposed.action = WikiAction::Update;
    let touched = apply(&store, &pool, vec![proposed]).await?;
    assert_eq!(touched, vec!["orphan-notes"]);
    assert_eq!(slugs(&pool).await?, vec!["orphan-notes"]);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_picks_the_first_check_when_two_topics_match(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = PostgresMemory::new(pool.clone()).with_wiki_directory(wiki.path().to_path_buf());
    let mut alpha = proposal("alpha-topic", "Alpha topic");
    alpha.aliases = vec![slug("shared-name")];
    let beta = proposal("shared-name-notes", "Shared name notes");
    apply(&store, &pool, vec![alpha, beta]).await?;
    let touched = apply(&store, &pool, vec![proposal("shared-name", "Shared name")]).await?;
    assert_eq!(touched, vec!["alpha-topic"]);
    assert_eq!(
        slugs(&pool).await?,
        vec!["alpha-topic", "shared-name-notes"]
    );
    assert_eq!(aliases(&pool, "alpha-topic").await?, vec!["shared-name"]);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn gate_keeps_an_alias_that_is_another_topics_slug(pool: PgPool) -> TestResult {
    let wiki = tempfile::tempdir()?;
    let store = store_with_deploys(&pool, &wiki).await?;
    apply(
        &store,
        &pool,
        vec![proposal("release-train", "Release train")],
    )
    .await?;
    let mut proposed = patch();
    proposed.aliases = vec![slug("release-train")];
    let touched = apply(&store, &pool, vec![proposed]).await?;
    assert_eq!(touched, vec!["acmekit-deploys"]);
    assert_eq!(
        slugs(&pool).await?,
        vec!["acmekit-deploys", "release-train"]
    );
    assert_eq!(
        aliases(&pool, "acmekit-deploys").await?,
        vec!["acmekit-rollout", "release-train"]
    );
    assert_eq!(redirect_count(&pool).await?, 0);
    Ok(())
}
