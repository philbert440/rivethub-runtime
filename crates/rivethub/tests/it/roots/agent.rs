use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue as _;
use kernel::tasks::TaskJobPayload;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore as _;
use memory_postgres::PostgresTasks;
use sqlx::ConnectOptions as _;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPromptMode;
use wire::task::TaskSpec;
use wire::task::TaskStatus;

use super::{
    TestResult, install_queue, interrupt, provider_config, stderr, terminate, write_state,
};

#[sqlx::test(migrations = false)]
async fn agent_runs_against_postgres_and_stops_on_sigterm(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    write_state(home.path(), &provider_config(home.path(), &url, ""))?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rh"))
        .arg("agent")
        .env_clear()
        .env("HOME", home.path())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let logs = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut text = String::new();
        let mut ready = Some(ready_tx);
        while let Some(line) = lines.next_line().await? {
            if line.contains("agent running")
                && let Some(sender) = ready.take()
                && sender.send(()).is_err()
            {
                return Err(std::io::Error::other("agent readiness listener dropped"));
            }
            text.push_str(&line);
            text.push('\n');
        }
        Ok(text)
    });
    if tokio::time::timeout(Duration::from_secs(30), ready_rx).await != Ok(Ok(())) {
        if child.try_wait()?.is_none()
            && let Some(pid) = child.id()
        {
            terminate(pid)?;
        }
        let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
        return Err(format!("agent did not run: {text}").into());
    }
    let pid = child.id().ok_or("agent already stopped")?;
    terminate(pid)?;
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
    let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
    assert!(status.success(), "agent exit: {status}\n{text}");
    assert!(text.contains("SIGTERM received — shutting down"), "{text}");
    assert!(text.contains("agent stopped"), "{text}");
    Ok(())
}

#[tokio::test]
async fn agent_rejects_invalid_preset_json() -> TestResult {
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(home.path(), super::DEAD_POSTGRES, ""),
    )?;
    std::fs::write(home.path().join(".rivethub").join("agents.json"), "{")?;
    let output = super::run_rh(home.path(), &["agent"]).await?;
    let text = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("task configuration JSON is invalid"),
        "{text}"
    );
    Ok(())
}

fn queued_task(id: sqlx::types::Uuid) -> TaskRecord {
    let session = format!("task-{id}");
    TaskRecord {
        id,
        goal: "hold the gateway".to_owned(),
        context_refs: Vec::new(),
        acceptance_criteria: Vec::new(),
        spec: TaskSpec {
            delegation: None,
            mesh_from: None,
            task_id: id.to_string(),
            agent_id: "main".to_owned(),
            goal: "hold the gateway".to_owned(),
            resolved_context: Vec::new(),
            acceptance_criteria: Vec::new(),
            budget: TaskBudget::default(),
            tools: None,
            working_dir: None,
            model: None,
            effort: None,
            system_prompt_append: None,
            isolation: None,
            prompt_mode: Some(TaskPromptMode::Task),
            exclude_tools: None,
            resume_message: None,
            session: session.clone(),
            permission_decisions: None,
            preset_id: None,
            preset_name: None,
            audit_only: None,
            extra: wire::Extra::default(),
        },
        executor: TaskExecutorKind::ChatLoop,
        executor_target: None,
        requested_by: None,
        agent_id: "main".to_owned(),
        origin: TaskOrigin::Chat,
        parent_task_id: None,
        chain_depth: 0,
        budget: TaskBudget::default(),
        node_affinity: None,
        usage: None,
        status: TaskStatus::Queued,
        attempt: 0,
        max_attempts: 5,
        result: None,
        error: None,
        conversation_id: None,
        session_key: Some(session),
        pending_message: None,
        claimed_by: None,
        harness_session_ids: Vec::new(),
        created_at: Utc::now(),
        started_at: None,
        last_heartbeat_at: None,
        completed_at: None,
        duration_ms: None,
        eval: None,
        eval_attempt: 0,
    }
}

#[derive(Debug, sqlx::FromRow)]
struct TaskRow {
    status: String,
    attempt: i32,
    claimed_by: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct JobRow {
    attempts: i64,
    locked: bool,
    last_error: Option<String>,
}

async fn task_row(pool: &PgPool, id: sqlx::types::Uuid) -> Result<TaskRow, sqlx::Error> {
    sqlx::query_as("SELECT status, attempt, claimed_by FROM rh_tasks WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
}

async fn job_row(pool: &PgPool, id: sqlx::types::Uuid) -> Result<JobRow, sqlx::Error> {
    sqlx::query_as(
        "SELECT j.attempts::bigint AS attempts, \
                (j.locked_at IS NOT NULL) AS locked, \
                j.last_error \
           FROM graphile_worker._private_jobs j \
          WHERE j.payload->>'taskId' = $1",
    )
    .bind(id.to_string())
    .fetch_one(pool)
    .await
}

async fn enqueue_blocked_task(pool: &PgPool) -> TestResult<sqlx::types::Uuid> {
    let id: sqlx::types::Uuid = sqlx::query_scalar("SELECT gen_random_uuid()")
        .fetch_one(pool)
        .await?;
    let tasks = PostgresTasks::from_pool(pool.clone());
    tasks.insert(&queued_task(id)).await?;
    tasks
        .enqueue(Job {
            name: JobName::RunTaskOn {
                node: "agent-node".to_owned(),
            },
            payload: wire::JsonObject::from_serialize(&TaskJobPayload {
                task_id: id.to_string(),
            })?,
            key: Some(format!("task:{id}")),
            key_mode: JobKeyMode::Replace,
            priority: 0,
            run_at: None,
            max_attempts: Some(5),
        })
        .await?;
    Ok(id)
}

struct LoggedChild {
    child: tokio::process::Child,
    logs: tokio::task::JoinHandle<std::io::Result<String>>,
}

async fn start_agent(home: &std::path::Path) -> TestResult<LoggedChild> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rh"))
        .arg("agent")
        .env_clear()
        .env("HOME", home)
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let logs = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut text = String::new();
        let mut ready = Some(ready_tx);
        while let Some(line) = lines.next_line().await? {
            if line.contains("agent running")
                && let Some(sender) = ready.take()
                && sender.send(()).is_err()
            {
                return Err(std::io::Error::other("agent readiness listener dropped"));
            }
            text.push_str(&line);
            text.push('\n');
        }
        Ok(text)
    });
    if tokio::time::timeout(Duration::from_secs(30), ready_rx).await != Ok(Ok(())) {
        if let Some(pid) = child.id() {
            interrupt(pid)?;
        }
        let text = tokio::time::timeout(Duration::from_secs(5), logs).await???;
        return Err(format!("agent did not run: {text}").into());
    }
    Ok(LoggedChild { child, logs })
}

async fn await_claim(
    pool: &PgPool,
    child: &mut tokio::process::Child,
    id: sqlx::types::Uuid,
) -> Result<(), String> {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!(
                        "agent exited before the task was running: {status}"
                    ));
                }
                Ok(None) => {}
                Err(error) => return Err(error.to_string()),
            }
            match (task_row(pool, id).await, job_row(pool, id).await) {
                (Ok(task), Ok(job))
                    if task.status == "running" && job.locked && job.attempts == 1 =>
                {
                    return Ok(());
                }
                (Ok(_), Ok(_)) => tokio::task::yield_now().await,
                (Err(error), _) | (_, Err(error)) => return Err(error.to_string()),
            }
        }
    })
    .await;
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err("task was not claimed".to_owned()),
    }
}

#[sqlx::test(migrations = false)]
async fn agent_blocked_on_a_gateway_releases_the_task_on_sigint(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    install_queue(&pool).await?;
    let hole = super::blackhole().await?;
    let id = enqueue_blocked_task(&pool).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    write_state(
        home.path(),
        &provider_config(
            home.path(),
            &url,
            &format!(
                "mesh:\n  node_name: agent-node\ngateway:\n  host: 127.0.0.1\n  port: {}\n",
                hole.address.port()
            ),
        ),
    )?;
    let LoggedChild { mut child, logs } = start_agent(home.path()).await?;
    if let Err(error) = await_claim(&pool, &mut child, id).await {
        if let Some(pid) = child.id() {
            interrupt(pid)?;
        }
        let text = logs.await??;
        return Err(format!("{error}\n{text}").into());
    }
    let pid = child.id().ok_or("agent already stopped")?;
    let started = tokio::time::Instant::now();
    interrupt(pid)?;
    let waited = tokio::time::timeout(kernel::shutdown::SHUTDOWN_GRACE, child.wait()).await;
    let elapsed = started.elapsed();
    let Ok(status) = waited else {
        child.start_kill()?;
        let text = logs.await??;
        return Err(format!("agent still running after {elapsed:?}\n{text}").into());
    };
    let status = status?;
    let text = tokio::time::timeout(Duration::from_secs(5), logs).await???;
    assert!(
        elapsed < kernel::shutdown::SHUTDOWN_GRACE,
        "{elapsed:?}\n{text}"
    );
    assert!(status.success(), "agent exit: {status}\n{text}");
    assert!(text.contains("SIGINT received — shutting down"), "{text}");
    assert!(text.contains("agent stopped"), "{text}");
    let task = task_row(&pool, id).await?;
    assert_eq!(task.status, "queued");
    assert_eq!(task.attempt, 0);
    assert_eq!(task.claimed_by, None);
    let job = job_row(&pool, id).await?;
    assert_eq!(job.attempts, 0, "{job:?}");
    assert!(!job.locked, "{job:?}");
    assert_eq!(job.last_error, None);
    Ok(())
}
