use std::time::Duration;

use axum::Router;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::routing::get;
use futures::SinkExt;
use futures::StreamExt;
use gateway::server::ServeError;
use gateway::server::ServeOptions;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

type ServerHandle = tokio::task::JoinHandle<Result<(), ServeError>>;

async fn ws_upgrade(upgrade: WebSocketUpgrade) -> axum::response::Response {
    upgrade.on_upgrade(echo_frames)
}

async fn echo_frames(mut socket: WebSocket) {
    while let Some(Ok(message)) = socket.recv().await {
        if socket.send(message).await.is_err() {
            return;
        }
    }
}

fn ws_router() -> Router {
    Router::new().route("/ws", get(ws_upgrade))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_shutdown_waits_for_the_upgraded_socket() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener");
    let addr = listener.local_addr().expect("addr");
    let token = CancellationToken::new();
    let mut server: ServerHandle = tokio::spawn(gateway::server::serve_router_with(
        listener,
        ws_router(),
        None,
        token.clone(),
        ServeOptions::default(),
    ));
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("websocket upgrade");
    token.cancel();
    socket
        .send(Message::text("ping"))
        .await
        .expect("frame sent after shutdown began");
    let echo = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("echo answered after shutdown began")
        .expect("stream open")
        .expect("frame");
    assert_eq!(echo, Message::text("ping"));
    let finished = tokio::time::timeout(Duration::ZERO, &mut server).await;
    assert!(
        finished.is_err(),
        "serve returned while the upgraded socket was still open"
    );
    drop(socket);
    let outcome = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("serve returns once the upgraded socket is gone")
        .expect("serve task");
    assert!(outcome.is_ok());
}
