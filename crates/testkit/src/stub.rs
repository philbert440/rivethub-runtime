use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, DropGuard};

#[derive(Debug, thiserror::Error)]
pub enum CloseError {
    #[error("HTTP stub failed: {0}")]
    Serve(#[from] std::io::Error),
    #[error("HTTP stub task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

#[derive(Debug)]
pub struct Stub {
    pub addr: SocketAddr,
    shutdown: CancellationToken,
    _guard: DropGuard,
    task: JoinHandle<std::io::Result<()>>,
}

pub async fn stub_server(routes: Router) -> std::io::Result<Stub> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let shutdown = CancellationToken::new();
    let cancelled = shutdown.clone().cancelled_owned();
    let task = tokio::spawn(async move {
        axum::serve(listener, routes)
            .with_graceful_shutdown(cancelled)
            .await
    });
    Ok(Stub {
        addr,
        _guard: shutdown.clone().drop_guard(),
        shutdown,
        task,
    })
}

impl Stub {
    pub async fn close(self) -> Result<(), CloseError> {
        self.shutdown.cancel();
        self.task.await??;
        Ok(())
    }
}
