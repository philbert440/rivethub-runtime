use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(super) struct Proxy {
    addr: SocketAddr,
    paused: Arc<AtomicBool>,
    sessions: Arc<Mutex<CancellationToken>>,
    task: tokio::task::JoinHandle<()>,
}

impl Proxy {
    pub(super) async fn start(upstream: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let paused = Arc::new(AtomicBool::new(false));
        let sessions = Arc::new(Mutex::new(CancellationToken::new()));
        let task = tokio::spawn(accept(
            listener,
            upstream,
            Arc::clone(&paused),
            Arc::clone(&sessions),
        ));
        Ok(Self {
            addr,
            paused,
            sessions,
            task,
        })
    }

    pub(super) fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub(super) async fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        let mut sessions = self.sessions.lock().await;
        sessions.cancel();
        *sessions = CancellationToken::new();
    }

    pub(super) fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn accept(
    listener: TcpListener,
    upstream: SocketAddr,
    paused: Arc<AtomicBool>,
    sessions: Arc<Mutex<CancellationToken>>,
) {
    loop {
        let accepted = listener.accept().await;
        let (client, _) = match accepted {
            Ok(pair) => pair,
            Err(_error) => break,
        };
        if paused.load(Ordering::SeqCst) {
            drop(client);
            continue;
        }
        let token = sessions.lock().await.child_token();
        drop(tokio::spawn(bridge(client, upstream, token)));
    }
}

async fn bridge(mut client: TcpStream, upstream: SocketAddr, token: CancellationToken) {
    let connected = TcpStream::connect(upstream).await;
    let mut server = match connected {
        Ok(server) => server,
        Err(_error) => return,
    };
    tokio::select! {
        biased;
        () = token.cancelled() => {}
        copied = copy_bidirectional(&mut client, &mut server) => {
            drop(copied);
        }
    }
}

pub(super) async fn upstream(database_url: &str) -> Result<SocketAddr, String> {
    let parsed =
        url::Url::parse(database_url).map_err(|_| "postgres url is not a tcp url".to_owned())?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "postgres url has no tcp host".to_owned())?;
    let port = parsed
        .port()
        .ok_or_else(|| "postgres url has no explicit port".to_owned())?;
    let mut addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "postgres host did not resolve".to_owned())?;
    addresses
        .next()
        .ok_or_else(|| "postgres host did not resolve".to_owned())
}

pub(super) fn point_at(database_url: &str, proxy: SocketAddr) -> Result<String, String> {
    let mut parsed =
        url::Url::parse(database_url).map_err(|_| "postgres url is not a tcp url".to_owned())?;
    if parsed.host_str().is_none() {
        return Err("postgres url has no tcp host".to_owned());
    }
    if parsed.port().is_none() {
        return Err("postgres url has no explicit port".to_owned());
    }
    parsed
        .set_ip_host(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
        .map_err(|()| "proxy host was rejected".to_owned())?;
    parsed
        .set_port(Some(proxy.port()))
        .map_err(|()| "proxy port was rejected".to_owned())?;
    Ok(parsed.to_string())
}
