use sqlx::{ConnectOptions, PgPool};

#[sqlx::test(migrations = false)]
#[ignore = "explicit recording operation requires a real pg_dump executable on PATH"]
async fn record_pre_0020_import(pool: PgPool) -> Result<(), Box<dyn std::error::Error>> {
    for migration in migrate::POSTGRES
        .iter()
        .filter(|migration| migration.version < 20)
    {
        sqlx::raw_sql(migration.sql.clone()).execute(&pool).await?;
    }
    let directory = testkit::recording("memory-sqlite/import");
    sqlx::raw_sql(include_str!(
        "../../testkit/tests/fixtures/memory-sqlite/import/seed.txt"
    ))
    .execute(&pool)
    .await?;
    let output = std::process::Command::new("pg_dump")
        .args(memory_sqlite::import::ImportTable::pg_dump_arguments())
        .arg(pool.connect_options().to_url_lossy().as_str())
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8(output.stderr)?.into());
    }
    std::fs::write(directory.join("pre-0020.pg.txt"), output.stdout)?;
    Ok(())
}
