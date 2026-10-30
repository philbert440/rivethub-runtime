use memory_postgres::PostgresTasks;
use sqlx::PgPool;
use testkit::tasks;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_task_runs_once_on_its_node_and_only_a_kill_ends_it_early(pool: PgPool) -> TestResult {
    tasks::a_task_runs_once_on_its_node_and_only_a_kill_ends_it_early(&PostgresTasks::from_pool(
        pool,
    ))
    .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn a_running_task_is_reaped_only_when_its_liveness_goes_stale(pool: PgPool) -> TestResult {
    tasks::a_running_task_is_reaped_only_when_its_liveness_goes_stale(&PostgresTasks::from_pool(
        pool,
    ))
    .await
}
