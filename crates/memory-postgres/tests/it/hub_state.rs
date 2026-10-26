#![cfg(test)]

use kernel::hub_state::HubStateStore;
use kernel::hub_state::HubStateWrite;
use memory_postgres::PostgresHubState;
use memory_postgres::PostgresMemory;
use sqlx::PgPool;
use wire::gateway::hub_state::HubState;
use wire::gateway::hub_state::HubStateResponse;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const KEY: &str = "node-a::claude-code:00000000-0000-4000-8000-000000000001";

fn pinned(key: &str) -> HubState {
    HubState {
        pinned: vec![key.to_owned()],
        ..HubState::default()
    }
}

async fn store(pool: &PgPool) -> Result<PostgresHubState, Box<dyn std::error::Error>> {
    Ok(PostgresMemory::new(pool.clone())
        .hub_state()
        .await?
        .ok_or("rh_hub_state is missing after migration")?)
}

#[sqlx::test(migrations = false)]
async fn a_database_without_the_table_is_not_a_hub_state_store(pool: PgPool) -> TestResult {
    assert!(PostgresHubState::detect(pool).await?.is_none());
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn versions_advance_and_stale_writes_conflict(pool: PgPool) -> TestResult {
    let hub = store(&pool).await?;
    assert_eq!(hub.load("owner").await?, HubStateResponse::default());

    let first = hub.replace("owner", 0, pinned(KEY)).await?;
    let saved = HubStateResponse {
        version: 1,
        state: pinned(KEY),
    };
    assert_eq!(first, HubStateWrite::Saved(saved.clone()));
    assert_eq!(hub.load("owner").await?, saved);

    assert_eq!(
        hub.replace("owner", 0, HubState::default()).await?,
        HubStateWrite::Conflict(saved.clone())
    );
    assert_eq!(
        hub.replace("owner", 4, HubState::default()).await?,
        HubStateWrite::Conflict(saved.clone())
    );

    let second = hub.replace("owner", 1, HubState::default()).await?;
    assert_eq!(
        second,
        HubStateWrite::Saved(HubStateResponse {
            version: 2,
            state: HubState::default(),
        })
    );
    assert_eq!(hub.load("alice").await?, HubStateResponse::default());
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn two_nodes_on_one_database_share_the_document(pool: PgPool) -> TestResult {
    let desktop = store(&pool).await?;
    let phone = store(&pool).await?;
    desktop.replace("owner", 0, pinned(KEY)).await?;
    assert_eq!(phone.load("owner").await?.version, 1);
    let (left, right) = tokio::join!(
        desktop.replace("owner", 1, HubState::default()),
        phone.replace("owner", 1, pinned(KEY)),
    );
    let outcomes = [left?, right?];
    let saved = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, HubStateWrite::Saved(_)))
        .count();
    assert_eq!(saved, 1, "{outcomes:?}");
    assert_eq!(desktop.load("owner").await?.version, 2);
    Ok(())
}
