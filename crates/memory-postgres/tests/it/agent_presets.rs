#![cfg(test)]

use chrono::DateTime;
use chrono::Utc;
use kernel::agent_presets::AgentPreset;
use kernel::agent_presets::AgentPresetStore;
use kernel::agent_presets::AgentPresetStoreError;
use kernel::agent_presets::AgentPresetUpdate;
use kernel::agent_presets::AgentStoreBackend;
use memory_postgres::PostgresMemory;
use memory_postgres::PostgresPresets;
use sqlx::PgPool;
use wire::harness::HarnessId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, sqlx::FromRow)]
struct StoredRow {
    node: String,
    node_base_url: String,
    shared_link: bool,
    sort_order: Option<i32>,
}

fn at(millis: i64) -> Result<DateTime<Utc>, &'static str> {
    DateTime::from_timestamp_millis(millis).ok_or("timestamp")
}

fn preset(id: &str, name: &str, node: &str, millis: i64) -> Result<AgentPreset, &'static str> {
    Ok(AgentPreset {
        id: id.to_owned(),
        name: name.to_owned(),
        color: "#abc".to_owned(),
        harness_id: Some(HarnessId::ClaudeCode),
        model: "opus".to_owned(),
        effort: "medium".to_owned(),
        system_prompt: "be brief".to_owned(),
        node: Some(node.to_owned()),
        directory: Some(format!("/home/user/agents/{id}")),
        shared_link: Some(true),
        node_base_url: Some("https://node.example.com".to_owned()),
        sort_order: None,
        created_at: at(millis)?,
        updated_at: at(millis)?,
        extra: wire::Extra::default(),
    })
}

async fn store(pool: &PgPool) -> Result<PostgresPresets, Box<dyn std::error::Error>> {
    Ok(PostgresMemory::new(pool.clone())
        .agent_presets()
        .await?
        .ok_or("rh_agent_presets is missing after migration")?)
}

async fn seeded(pool: &PgPool) -> Result<PostgresPresets, Box<dyn std::error::Error>> {
    let presets = store(pool).await?;
    presets
        .create(preset("p-000000000003", "Gamma", "node-b", 3_000)?)
        .await?;
    presets
        .create(preset("p-000000000002", "Beta", "node-a", 1_000)?)
        .await?;
    presets
        .create(preset("p-000000000001", "Alpha", "node-a", 1_000)?)
        .await?;
    Ok(presets)
}

fn ids(presets: &[AgentPreset]) -> Vec<&str> {
    presets.iter().map(|preset| preset.id.as_str()).collect()
}

#[sqlx::test(migrations = false)]
async fn a_database_without_the_preset_table_is_not_a_preset_store(pool: PgPool) -> TestResult {
    assert!(PostgresPresets::detect(pool).await?.is_none());
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_migrated_database_is_the_postgres_preset_store(pool: PgPool) -> TestResult {
    let presets = store(&pool).await?;
    assert_eq!(presets.backend(), AgentStoreBackend::Postgres);
    assert_eq!(presets.list(None).await?, Vec::new());
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn list_is_mesh_wide_in_creation_order_and_filters_by_node(pool: PgPool) -> TestResult {
    let presets = seeded(&pool).await?;
    let all = presets.list(None).await?;
    assert_eq!(
        ids(&all),
        ["p-000000000001", "p-000000000002", "p-000000000003"]
    );
    assert_eq!(
        all.iter()
            .map(|preset| preset.node.as_deref())
            .collect::<Vec<_>>(),
        [Some("node-a"), Some("node-a"), Some("node-b")]
    );
    let on_a = presets.list(Some("node-a")).await?;
    assert_eq!(ids(&on_a), ["p-000000000001", "p-000000000002"]);
    let on_b = presets.list(Some("node-b")).await?;
    assert_eq!(ids(&on_b), ["p-000000000003"]);
    assert_eq!(presets.list(Some("node-c")).await?, Vec::new());
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn create_stores_every_field_and_get_reads_it_back(pool: PgPool) -> TestResult {
    let presets = store(&pool).await?;
    let mut wanted = preset("p-0000000000aa", "Kit", "node-a", 5_000)?;
    wanted.sort_order = Some(7);
    let created = presets.create(wanted.clone()).await?;
    assert_eq!(created, wanted);
    assert_eq!(presets.get("p-0000000000aa").await?, Some(wanted));
    assert_eq!(presets.get("p-missing").await?, None);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn create_stores_an_absent_node_base_url_as_empty(pool: PgPool) -> TestResult {
    let presets = store(&pool).await?;
    let mut bare = preset("p-0000000000bb", "Bare", "node-a", 5_000)?;
    bare.node_base_url = None;
    bare.shared_link = None;
    bare.harness_id = None;
    let created = presets.create(bare).await?;
    assert_eq!(created.node_base_url.as_deref(), Some(""));
    assert_eq!(created.shared_link, Some(true));
    assert_eq!(created.harness_id, None);
    let row = sqlx::query_as::<_, StoredRow>(
        "SELECT node, node_base_url, shared_link, sort_order FROM rh_agent_presets WHERE id = $1",
    )
    .bind("p-0000000000bb")
    .fetch_one(&pool)
    .await?;
    assert_eq!(row.node, "node-a");
    assert_eq!(row.node_base_url, "");
    assert!(row.shared_link);
    assert_eq!(row.sort_order, None);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn create_requires_node_and_directory(pool: PgPool) -> TestResult {
    let presets = store(&pool).await?;
    let mut no_node = preset("p-0000000000cc", "Nodeless", "node-a", 5_000)?;
    no_node.node = None;
    assert!(matches!(
        presets.create(no_node).await,
        Err(AgentPresetStoreError::Unavailable(_))
    ));
    let mut no_directory = preset("p-0000000000dd", "Homeless", "node-a", 5_000)?;
    no_directory.directory = None;
    assert!(matches!(
        presets.create(no_directory).await,
        Err(AgentPresetStoreError::Unavailable(_))
    ));
    assert_eq!(presets.list(None).await?, Vec::new());
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn lookup_matches_the_id_or_the_name_without_case(pool: PgPool) -> TestResult {
    let presets = seeded(&pool).await?;
    let by_id = presets.lookup("p-000000000002").await?.ok_or("by id")?;
    assert_eq!(by_id.name, "Beta");
    let by_name = presets.lookup("gAMMA").await?.ok_or("by name")?;
    assert_eq!(by_name.id, "p-000000000003");
    assert_eq!(presets.lookup("delta").await?, None);
    presets
        .create(preset("p-00000000000e", "p-000000000001", "node-a", 9_000)?)
        .await?;
    let preferred = presets.lookup("p-000000000001").await?.ok_or("id first")?;
    assert_eq!(preferred.name, "Alpha");
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn update_changes_only_the_provided_fields(pool: PgPool) -> TestResult {
    let presets = seeded(&pool).await?;
    let before = presets.get("p-000000000001").await?.ok_or("seeded")?;
    let changes = AgentPresetUpdate {
        name: Some("Alpha Prime".to_owned()),
        model: Some("sonnet".to_owned()),
        sort_order: Some(Some(4)),
        node: Some("node-z".to_owned()),
        ..AgentPresetUpdate::default()
    };
    let after = presets
        .update("p-000000000001", changes)
        .await?
        .ok_or("updated")?;
    assert_eq!(after.name, "Alpha Prime");
    assert_eq!(after.model, "sonnet");
    assert_eq!(after.sort_order, Some(4));
    assert_eq!(after.node.as_deref(), Some("node-a"));
    assert!(after.updated_at > before.updated_at);
    assert_eq!(after.created_at, before.created_at);
    let unchanged = AgentPreset {
        name: before.name.clone(),
        model: before.model.clone(),
        sort_order: before.sort_order,
        updated_at: before.updated_at,
        ..after.clone()
    };
    assert_eq!(unchanged, before);
    assert_eq!(presets.get("p-000000000001").await?, Some(after));
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn update_clears_and_sets_the_nullable_fields(pool: PgPool) -> TestResult {
    let presets = seeded(&pool).await?;
    let cleared = presets
        .update(
            "p-000000000002",
            AgentPresetUpdate {
                harness_id: Some(None),
                sort_order: Some(None),
                shared_link: Some(false),
                directory: Some("/home/user/elsewhere".to_owned()),
                ..AgentPresetUpdate::default()
            },
        )
        .await?
        .ok_or("cleared")?;
    assert_eq!(cleared.harness_id, None);
    assert_eq!(cleared.sort_order, None);
    assert_eq!(cleared.shared_link, Some(false));
    assert_eq!(cleared.directory.as_deref(), Some("/home/user/elsewhere"));
    let set = presets
        .update(
            "p-000000000002",
            AgentPresetUpdate {
                harness_id: Some(Some(HarnessId::KimiCode)),
                ..AgentPresetUpdate::default()
            },
        )
        .await?
        .ok_or("set")?;
    assert_eq!(set.harness_id, Some(HarnessId::KimiCode));
    assert_eq!(set.shared_link, Some(false));
    assert_eq!(
        presets
            .update("p-missing", AgentPresetUpdate::default())
            .await?,
        None
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn delete_removes_the_row_only_once(pool: PgPool) -> TestResult {
    let presets = seeded(&pool).await?;
    assert!(presets.delete("p-000000000002").await?);
    assert!(!presets.delete("p-000000000002").await?);
    assert_eq!(presets.get("p-000000000002").await?, None);
    assert_eq!(
        ids(&presets.list(None).await?),
        ["p-000000000001", "p-000000000003"]
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_duplicate_name_is_name_taken_on_create_and_update(pool: PgPool) -> TestResult {
    let presets = seeded(&pool).await?;
    let created = presets
        .create(preset("p-0000000000ff", "alpha", "node-b", 9_000)?)
        .await;
    assert!(
        matches!(&created, Err(AgentPresetStoreError::NameTaken { name }) if name == "alpha"),
        "{created:?}"
    );
    let renamed = presets
        .update(
            "p-000000000003",
            AgentPresetUpdate {
                name: Some("BETA".to_owned()),
                ..AgentPresetUpdate::default()
            },
        )
        .await;
    assert!(
        matches!(&renamed, Err(AgentPresetStoreError::NameTaken { name }) if name == "BETA"),
        "{renamed:?}"
    );
    assert_eq!(presets.list(None).await?.len(), 3);
    assert_eq!(
        presets
            .get("p-000000000003")
            .await?
            .map(|preset| preset.name),
        Some("Gamma".to_owned())
    );
    Ok(())
}
