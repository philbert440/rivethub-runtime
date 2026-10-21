use std::future::{Future as _, poll_fn};
use std::task::Poll;
use std::time::Duration;

use kernel::memory::MemoryStore;
use memory_sqlite::SqliteStore;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn close_drains_a_blocking_write_even_when_its_caller_is_cancelled() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("memory.sqlite");
    migrate::sqlite(&path)?;
    let store = SqliteStore::open(&path)?;
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let writer = store.clone();
    let task = tokio::spawn(async move {
        writer
            .with_conn(move |conn| {
                started.send(()).expect("started listener");
                blocked
                    .recv_timeout(Duration::from_secs(10))
                    .expect("write released");
                conn.execute_batch(
                    "CREATE TABLE close_proof (value INTEGER); INSERT INTO close_proof VALUES (7)",
                )
                .expect("write");
                Ok(())
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), ready).await??;
    task.abort();
    assert!(task.await.expect_err("cancelled caller").is_cancelled());
    let closing = store.close();
    tokio::pin!(closing);
    poll_fn(|cx| {
        assert!(closing.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    release.send(())?;
    tokio::time::timeout(Duration::from_secs(10), closing).await??;
    let conn = rusqlite::Connection::open(&path)?;
    let value: i64 = conn.query_row("SELECT value FROM close_proof", [], |row| row.get(0))?;
    assert_eq!(value, 7);
    store.close().await?;
    Ok(())
}
