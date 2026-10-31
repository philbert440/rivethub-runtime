use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use wire::gateway::HealthzResponse;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Node {
    home: tempfile::TempDir,
}

impl Node {
    fn new() -> TestResult<Self> {
        Ok(Self {
            home: tempfile::tempdir()?,
        })
    }

    fn path(&self) -> &Path {
        self.home.path()
    }

    fn pid_file(&self) -> PathBuf {
        self.path().join(".rivethub/rivethub.pid")
    }

    fn log_file(&self) -> PathBuf {
        self.path().join(".rivethub/logs/local.log")
    }

    async fn rh(&self, args: &[&str]) -> TestResult<Output> {
        let output = Command::new(env!("CARGO_BIN_EXE_rh"))
            .args(args)
            .env_clear()
            .env("HOME", self.path())
            .env("TOKIO_WORKER_THREADS", "2")
            .current_dir(self.path())
            .kill_on_drop(true)
            .output();
        Ok(tokio::time::timeout(Duration::from_secs(60), output).await??)
    }

    async fn ensure(&self, port: u16, wait: &str) -> TestResult<Output> {
        let port = port.to_string();
        self.rh(&[
            "local", "--ensure", "--port", &port, "--no-lan", "--wait", wait,
        ])
        .await
    }

    fn pid(&self) -> TestResult<i32> {
        Ok(std::fs::read_to_string(self.pid_file())?.trim().parse()?)
    }

    async fn stop(&self) -> TestResult {
        let stopped = self.rh(&["stop"]).await?;
        assert!(stopped.status.success(), "{stopped:?}");
        let mut ticks = tokio::time::interval(Duration::from_millis(50));
        let gone = async {
            while self.pid_file().exists() {
                ticks.tick().await;
            }
        };
        tokio::time::timeout(Duration::from_secs(30), gone).await?;
        Ok(())
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if let Ok(pid) = self.pid() {
            match nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            ) {
                Ok(()) | Err(_) => {}
            }
        }
    }
}

fn text(bytes: &[u8]) -> TestResult<String> {
    Ok(String::from_utf8(bytes.to_vec())?)
}

async fn healthz(port: u16) -> TestResult<HealthzResponse> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .build()?
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .send()
        .await?
        .json()
        .await?)
}

#[tokio::test]
async fn ensure_starts_one_detached_node_and_reuses_it() -> TestResult {
    let node = Node::new()?;
    let claim = testkit::ports::claim()?;
    let port = claim.number();
    let first = node.ensure(port, "30").await?;
    assert!(first.status.success(), "{first:?}");
    assert_eq!(
        text(&first.stdout)?,
        format!("Started RivetHub local node at http://127.0.0.1:{port}\n")
    );
    assert!(healthz(port).await?.ok);
    let pid = node.pid()?;
    let second = node.ensure(port, "30").await?;
    assert!(second.status.success(), "{second:?}");
    assert_eq!(
        text(&second.stdout)?,
        format!("RivetHub local node is running at http://127.0.0.1:{port}\n")
    );
    assert_eq!(node.pid()?, pid);
    assert!(std::fs::read_to_string(node.log_file())?.contains("127.0.0.1"));
    node.stop().await
}

#[tokio::test]
async fn concurrent_ensures_start_a_single_node() -> TestResult {
    let node = Node::new()?;
    let claim = testkit::ports::claim()?;
    let port = claim.number();
    let (left, right) = tokio::join!(node.ensure(port, "30"), node.ensure(port, "30"));
    let outputs = [left?, right?];
    let mut started = 0;
    for output in &outputs {
        assert!(output.status.success(), "{output:?}");
        if text(&output.stdout)?.starts_with("Started ") {
            started += 1;
        }
    }
    assert_eq!(started, 1, "{outputs:?}");
    assert!(healthz(port).await?.ok);
    node.stop().await
}

#[tokio::test]
async fn ensure_leaves_an_answering_port_alone() -> TestResult {
    let node = Node::new()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut request = [0_u8; 1024];
        let read = stream.read(&mut request).await?;
        stream
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await?;
        Ok::<_, std::io::Error>(read)
    });
    let output = node.ensure(port, "2").await?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        text(&output.stdout)?,
        format!("RivetHub local node is running at http://127.0.0.1:{port}\n")
    );
    assert!(server.await?? > 0);
    assert!(!node.pid_file().exists());
    assert!(!node.log_file().exists());
    Ok(())
}

#[tokio::test]
async fn ensure_fails_with_the_log_path_when_the_node_cannot_serve() -> TestResult {
    let node = Node::new()?;
    let silent = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = silent.local_addr()?.port();
    let output = node.ensure(port, "2").await?;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = text(&output.stderr)?;
    assert!(
        stderr.contains(&format!(
            "did not answer at http://127.0.0.1:{port} within 2s; its output is in {}",
            node.log_file().display()
        )),
        "{stderr}"
    );
    assert!(node.log_file().is_file());
    drop(silent);
    Ok(())
}

#[tokio::test]
async fn ensure_adopts_legacy_state_before_creating_its_own() -> TestResult {
    let node = Node::new()?;
    std::fs::create_dir(node.path().join(".rivetos"))?;
    let claim = testkit::ports::claim()?;
    let port = claim.number();
    let output = node.ensure(port, "30").await?;
    assert!(output.status.success(), "{output:?}");
    assert!(
        text(&output.stderr)?.contains("moved legacy state"),
        "{output:?}"
    );
    assert!(!node.path().join(".rivetos").exists());
    assert!(healthz(port).await?.ok);
    node.stop().await
}
