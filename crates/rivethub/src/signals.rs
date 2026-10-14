use std::io;

use tokio::signal::unix::SignalKind;
use tokio_util::sync::CancellationToken;

use crate::Exit;

#[derive(Debug)]
pub(crate) struct Installed {
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), io::Error>>,
}

impl Installed {
    pub(crate) fn token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    pub(crate) async fn finish(self) -> Result<(), Exit> {
        self.shutdown.cancel();
        self.task.abort();
        match self.task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                tracing::error!("signal handler failed: {error}");
                Err(Exit::Runtime)
            }
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => {
                tracing::error!("signal task failed: {error}");
                Err(Exit::Runtime)
            }
        }
    }
}

pub(crate) fn install() -> Result<Installed, Exit> {
    let shutdown = CancellationToken::new();
    let sigterm = match tokio::signal::unix::signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            tracing::error!("installing the SIGTERM handler failed: {error}");
            return Err(Exit::Runtime);
        }
    };
    let token = shutdown.clone();
    let task = tokio::spawn(watch(sigterm, token));
    Ok(Installed { shutdown, task })
}

async fn watch(
    mut sigterm: tokio::signal::unix::Signal,
    token: CancellationToken,
) -> Result<(), io::Error> {
    let signal = tokio::select! {
        result = tokio::signal::ctrl_c() => result.map(|()| "SIGINT"),
        signal = sigterm.recv() => signal
            .map(|()| "SIGTERM")
            .ok_or_else(|| io::Error::other("SIGTERM handler closed")),
    };
    let signal = signal?;
    tracing::info!("{signal} received — shutting down");
    token.cancel();
    Ok(())
}
