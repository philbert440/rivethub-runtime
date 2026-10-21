use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gateway::tenancy::{Tenancy, UserId};
use gateway::{Gateway, Settings};
use kernel::tasks::TaskStore;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;
use wire::gateway::notifications::TaskDoneNotification;
use wire::task::TaskStatus;

use super::Setup;
use super::finished;
use super::record;
use super::stamp;

const WAIT: Duration = Duration::from_secs(5);

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn settings() -> Settings {
    let scratch = PathBuf::from("/tmp/rr-gateway-unused-agents");
    Settings {
        hostname: "gold-host".to_owned(),
        node: "golden-node".to_owned(),
        allowed_origins: Vec::new(),
        allowed_hosts: Vec::new(),
        files_root: None,
        static_dir: None,
        root_redirect: None,
        state_dir: scratch.join("state"),
        directory_root: scratch.join("agents"),
        mesh_file: Some(PathBuf::from("/no-such-gateway-mesh/mesh.json")),
        shared_dir: scratch.join("shared"),
        legacy_mesh_dir: PathBuf::from("/no-such-gateway-legacy"),
        allowed_harnesses: None,
        devices_enabled: false,
        probe_timeout: Duration::from_millis(200),
        mesh_cache_ttl: Duration::ZERO,
        mesh_ca: None,
    }
}

struct Served {
    address: std::net::SocketAddr,
    stop: tokio_util::sync::CancellationToken,
}

async fn serve(setup: Option<&Setup>) -> Served {
    let owner = UserId::new("owner".into()).expect("user");
    let gateway = Gateway::new(Arc::new(testkit::FakeMemoryStore::default()), settings())
        .with_tenancy(Tenancy::local(owner));
    let gateway = Arc::new(match setup {
        Some(setup) => gateway.with_tasks(setup.api()),
        None => gateway,
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let stop = tokio_util::sync::CancellationToken::new();
    tokio::spawn(gateway::server::serve(
        listener,
        gateway,
        None,
        stop.clone(),
    ));
    Served { address, stop }
}

async fn listen(served: &Served) -> Socket {
    tokio::time::timeout(
        WAIT,
        tokio_tungstenite::connect_async(format!("ws://{}/api/notifications/ws", served.address)),
    )
    .await
    .expect("connect timeout")
    .expect("connect")
    .0
}

async fn notification(socket: &mut Socket) -> TaskDoneNotification {
    let frame = tokio::time::timeout(WAIT, socket.next())
        .await
        .expect("frame timeout")
        .expect("open stream")
        .expect("frame");
    let Message::Text(text) = frame else {
        panic!("text frame expected, got {frame:?}");
    };
    assert!(text.contains(r#""kind":"task.done""#), "{text}");
    serde_json::from_str(&text).expect("task.done frame")
}

async fn queued(setup: &Setup, id: u128) -> Uuid {
    let id = Uuid::from_u128(id);
    setup
        .memory
        .insert(&record(id, "alpha", stamp(15)))
        .await
        .expect("insert");
    id
}

#[tokio::test]
async fn a_finished_task_is_announced_on_the_notifications_socket() {
    let setup = Setup::new();
    let served = serve(Some(&setup)).await;
    let mut socket = listen(&served).await;
    let done = queued(&setup, 7).await;
    setup
        .memory
        .finish(done, &finished(TaskStatus::Completed))
        .await
        .expect("finish");
    let frame = notification(&mut socket).await;
    assert_eq!(
        (frame.task_id, frame.status),
        (done.to_string(), TaskStatus::Completed)
    );
    let before = chrono::Utc::now();
    let killed = queued(&setup, 8).await;
    setup.memory.kill(killed, stamp(16)).await.expect("kill");
    let frame = notification(&mut socket).await;
    assert_eq!(
        (frame.task_id, frame.status),
        (killed.to_string(), TaskStatus::Killed)
    );
    assert!(frame.ts >= before - chrono::Duration::seconds(1));
    served.stop.cancel();
}

#[tokio::test]
async fn a_task_that_does_not_finish_is_not_announced() {
    let setup = Setup::new();
    let served = serve(Some(&setup)).await;
    let mut socket = listen(&served).await;
    let running = queued(&setup, 9).await;
    setup
        .memory
        .set_pending(running, "keep going")
        .await
        .expect("pending");
    let done = queued(&setup, 10).await;
    setup
        .memory
        .finish(done, &finished(TaskStatus::Failed))
        .await
        .expect("finish");
    let frame = notification(&mut socket).await;
    assert_eq!(
        (frame.task_id, frame.status),
        (done.to_string(), TaskStatus::Failed)
    );
    served.stop.cancel();
}

#[tokio::test]
async fn the_socket_opens_on_a_node_without_tasks() {
    let served = serve(None).await;
    let socket = listen(&served).await;
    drop(socket);
    served.stop.cancel();
}
