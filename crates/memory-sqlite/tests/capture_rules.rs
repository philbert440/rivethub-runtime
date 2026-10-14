#[expect(
    clippy::redundant_pub_crate,
    reason = "private shared test module: unreachable_pub requires pub(crate) here, the nursery lint disagrees"
)]
mod common;

use common::TestResult;
use common::batch;
use common::fresh_store;
use common::instant;
use common::table;

async fn stored_updated_at(store: &memory_sqlite::SqliteStore) -> TestResult<String> {
    let rows = table(store, "ros_conversations").await?;
    let row = rows.first().ok_or("one conversation")?;
    Ok(row
        .get("updated_at")
        .and_then(testkit::Json::as_str)
        .ok_or("updated_at text")?
        .to_owned())
}

#[tokio::test]
async fn updated_at_only_moves_forward() -> TestResult {
    let fresh = fresh_store(None)?;
    let now = instant(common::RECORDED_START)?;
    let stamped = |updated_at: &str| {
        let mut capture = batch("old-task", "rivet", "cli");
        capture.updated_at = Some(updated_at.to_owned());
        capture
    };
    fresh
        .store
        .capture(stamped("2025-03-01T01:00:00+01:00"), now)
        .await?;
    fresh
        .store
        .capture(stamped("2024-06-01T00:00:00Z"), now)
        .await?;
    assert_eq!(
        stored_updated_at(&fresh.store).await?,
        "2025-03-01T00:00:00.000Z"
    );
    fresh
        .store
        .capture(batch("old-task", "rivet", "cli"), now)
        .await?;
    assert_eq!(
        stored_updated_at(&fresh.store).await?,
        common::RECORDED_START
    );
    Ok(())
}
