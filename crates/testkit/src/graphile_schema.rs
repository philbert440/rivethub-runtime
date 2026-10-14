use sqlx::PgPool;

const SCHEMA: &str = include_str!("../tests/fixtures/graphile/worker-schema.txt");

pub async fn install_graphile_worker(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}
