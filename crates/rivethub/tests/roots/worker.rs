use std::process::Stdio;
use std::time::Duration;

use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue as _;
use memory_postgres::PostgresMemory;
use sqlx::ConnectOptions as _;
use sqlx::PgPool;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use wire::JsonObject;

use super::{TestResult, provider_config, terminate, write_state};

#[derive(Debug, sqlx::FromRow)]
struct JobTally {
    n: i64,
    max_attempts: Option<i64>,
    attempts: Option<i64>,
}

fn reap_job() -> Job {
    Job {
        name: JobName::Fixed(FixedJob::ReapDeadJobs),
        payload: JsonObject::default(),
        key: None,
        key_mode: JobKeyMode::KeepRunAt,
        priority: 0,
        run_at: None,
    }
}

async fn tally(pool: &PgPool) -> Result<JobTally, sqlx::Error> {
    sqlx::query_as(
        "SELECT count(*)::bigint AS n, \
                max(j.max_attempts)::bigint AS max_attempts, \
                max(j.attempts)::bigint AS attempts \
           FROM graphile_worker._private_jobs j \
           JOIN graphile_worker._private_tasks t ON t.id = j.task_id \
          WHERE t.identifier = $1",
    )
    .bind("reap-dead-jobs")
    .fetch_one(pool)
    .await
}

async fn install_queue(pool: &PgPool) -> Result<(), sqlx::Error> {
    let present: bool =
        sqlx::query_scalar("SELECT to_regclass('graphile_worker._private_jobs') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if present {
        return Ok(());
    }
    sqlx::raw_sql(include_str!("../fixtures/graphile_worker.txt"))
        .execute(pool)
        .await?;
    Ok(())
}

async fn wait_until_reaped(pool: &PgPool, child: &mut tokio::process::Child) -> Result<(), String> {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!("worker exited before the job completed: {status}"));
                }
                Ok(None) => {}
                Err(error) => return Err(error.to_string()),
            }
            match tally(pool).await {
                Ok(row) if row.n == 0 => return Ok(()),
                Ok(_) => tokio::task::yield_now().await,
                Err(error) => return Err(error.to_string()),
            }
        }
    })
    .await;
    match result {
        Ok(inner) => inner,
        Err(_) => match tally(pool).await {
            Ok(row) => Err(format!(
                "reap-dead-jobs was not completed: {} row(s), max_attempts={:?}, attempts={:?}",
                row.n, row.max_attempts, row.attempts
            )),
            Err(error) => Err(error.to_string()),
        },
    }
}

#[sqlx::test(migrations = false)]
async fn worker_claims_a_reap_job_and_stops_on_sigterm(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    install_queue(&pool).await?;
    let memory = PostgresMemory::new(pool.clone());
    let queue = memory.queue().await?;
    if !queue.available() {
        return Err(
            "the graphile_worker schema is absent on the lane database; refusing to skip the worker proof"
                .into(),
        );
    }
    queue.enqueue(reap_job()).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    write_state(home.path(), &provider_config(home.path(), &url, ""))?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rh"))
        .args(["worker", "--role", "summarizer"])
        .env_clear()
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let logs = tokio::spawn(async move {
        let mut stderr = stderr;
        let mut text = String::new();
        stderr.read_to_string(&mut text).await?;
        Ok::<_, std::io::Error>(text)
    });
    if let Err(error) = wait_until_reaped(&pool, &mut child).await {
        let pid = child.id();
        if let Some(pid) = pid {
            terminate(pid)?;
        }
        let text = logs.await??;
        return Err(format!("{error}\n{text}").into());
    }
    let pid = child.id().ok_or("worker already stopped")?;
    terminate(pid)?;
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
    let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
    assert!(status.success(), "worker exit: {status}\n{text}");
    assert!(text.contains("SIGTERM received — shutting down"), "{text}");
    assert!(text.contains("worker stopped"), "{text}");
    Ok(())
}
