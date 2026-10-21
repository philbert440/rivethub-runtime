use std::future::Future;

use kernel::shutdown::Activities;
use kernel::shutdown::SHUTDOWN_GRACE;
use tokio_util::sync::CancellationToken;

use crate::Exit;

pub(crate) struct AbortOnDrop {
    handle: tokio::task::AbortHandle,
}

impl AbortOnDrop {
    pub(crate) fn arm(task: &tokio::task::JoinHandle<impl Sized>) -> Self {
        Self {
            handle: task.abort_handle(),
        }
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub(crate) async fn bounded<T>(
    shutdown: &CancellationToken,
    activities: &Activities,
    work: impl Future<Output = T>,
) -> Result<T, String> {
    tokio::pin!(work);
    tokio::select! {
        biased;
        value = &mut work => Ok(value),
        () = shutdown.cancelled() => {
            tokio::select! {
                biased;
                value = &mut work => Ok(value),
                () = tokio::time::sleep(SHUTDOWN_GRACE) => {
                    let running = activities.list();
                    tracing::error!("shutdown grace of 10s elapsed; still running: {running}");
                    Err(running)
                }
            }
        }
    }
}

pub(crate) async fn signals_then<T, E>(
    work: impl AsyncFnOnce(CancellationToken, Activities) -> Result<T, E>,
    finish: impl FnOnce(Result<T, E>) -> Exit,
) -> Exit {
    let signals = match crate::signals::install() {
        Ok(signals) => signals,
        Err(exit) => return exit,
    };
    let activities = Activities::new();
    let token = signals.token();
    let result = bounded(&token, &activities, work(token.clone(), activities.clone())).await;
    if let Err(exit) = signals.finish().await {
        return exit;
    }
    match result {
        Ok(inner) => finish(inner),
        Err(running) => {
            tracing::debug!("bounded shutdown: {running}");
            Exit::Failure
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use kernel::shutdown::Activities;
    use kernel::shutdown::SHUTDOWN_GRACE;
    use tokio_util::sync::CancellationToken;

    use super::bounded;

    #[tokio::test]
    async fn work_that_finishes_after_cancel_returns_before_the_grace() {
        let shutdown = CancellationToken::new();
        let activities = Activities::new();
        shutdown.cancel();
        let started = tokio::time::Instant::now();
        let result = bounded(&shutdown, &activities, async {
            tokio::task::yield_now().await;
            7
        })
        .await;
        assert_eq!(result.expect("finished work"), 7);
        assert!(started.elapsed() < SHUTDOWN_GRACE);
    }

    #[tokio::test(start_paused = true)]
    async fn grace_names_work_that_is_still_entered() {
        let shutdown = CancellationToken::new();
        let activities = Activities::new();
        let _guard = activities.enter("provider chat");
        shutdown.cancel();
        let started = tokio::time::Instant::now();
        let result = bounded(&shutdown, &activities, std::future::pending::<()>()).await;
        let elapsed = started.elapsed();
        assert_eq!(result.expect_err("grace"), "provider chat");
        assert!(elapsed >= SHUTDOWN_GRACE);
        assert!(elapsed < SHUTDOWN_GRACE + Duration::from_secs(2));
    }
}
