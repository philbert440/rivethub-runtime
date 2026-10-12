use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Debug)]
pub struct Stub {
    pub addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

pub async fn stub_server(routes: Router) -> std::io::Result<Stub> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let (shutdown, rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, routes)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });
    Ok(Stub {
        addr,
        shutdown,
        task,
    })
}

impl Stub {
    pub async fn close(self) {
        let _ = self.shutdown.send(());
        let _ = self.task.await;
    }
}
