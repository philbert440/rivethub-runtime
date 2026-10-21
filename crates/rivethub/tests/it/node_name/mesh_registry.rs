use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{
    CONFIGURED, Started, TestResult, command, finish, free_port, logged_host, provider_config,
    stop_early,
};

fn write_certs(directory: &Path) -> TestResult {
    let root_key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(vec!["root.example.test".into()])?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let root = params.self_signed(&root_key)?;
    let issuer = rcgen::Issuer::from_params(&params, &root_key);
    let node_key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])?;
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let node = params.signed_by(&node_key, &issuer)?;
    std::fs::write(directory.join("node.pem"), node.pem())?;
    std::fs::write(directory.join("node.key"), node_key.serialize_pem())?;
    std::fs::write(directory.join("ca.pem"), root.pem())?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn mesh_registry_records_the_configured_node(pool: PgPool) -> TestResult {
    migrate::postgres_pool(&pool).await?;
    let url = pool.connect_options().to_url_lossy();
    let home = tempfile::tempdir()?;
    let state = home.path().join(".rivethub");
    std::fs::create_dir_all(&state)?;
    write_certs(&state)?;
    let shared = home.path().join("shared");
    std::fs::create_dir_all(&shared)?;
    let port = free_port()?;
    let channel = free_port()?;
    let body = provider_config(
        home.path(),
        &url,
        &format!(
            "gateway:\n  host: 127.0.0.1\n  port: {port}\n  tls_cert: {}\n  tls_key: {}\n  tls_ca: {}\nmesh:\n  enabled: true\n  node_name: \" {CONFIGURED} \"\n  tls: true\n  advertise_host: 127.0.0.1\n  agent_channel_port: {channel}\n",
            state.join("node.pem").display(),
            state.join("node.key").display(),
            state.join("ca.pem").display(),
        ),
    );
    std::fs::write(state.join("config.yaml"), body)?;
    let mut child = command(home.path(), &["gateway"]);
    child
        .env("RIVETHUB_SHARED_DIR", &shared)
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = child.spawn()?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let logs = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut text = String::new();
        while let Some(line) = lines.next_line().await? {
            text.push_str(&line);
            text.push('\n');
        }
        Ok::<String, std::io::Error>(text)
    });
    let registry = shared.join("mesh.json");
    let recorded = wait_for_registry(&registry, &mut child).await;
    let text = match recorded {
        Ok(file) => {
            let started = Started { child, logs };
            let text = finish(started).await?;
            assert!(file.nodes.contains_key(CONFIGURED), "{file:?}");
            assert_eq!(
                file.nodes.get(CONFIGURED).map(|node| node.name.as_str()),
                Some(CONFIGURED)
            );
            text
        }
        Err(error) => {
            stop_early(&mut child)?;
            let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
            return Err(format!("{error}\n{text}").into());
        }
    };
    logged_host(&text, "gateway")?;
    Ok(())
}

async fn wait_for_registry(
    path: &Path,
    child: &mut tokio::process::Child,
) -> Result<wire::mesh::MeshFile, String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("mesh registry {} has no directory", path.display()))?;
    let watched = mesh_watch(directory)?;
    if let Some(file) = read_registry(path).await? {
        return Ok(file);
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            mesh_event(&watched).await?;
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!("gateway exited before mesh.json: {status}"));
                }
                Ok(None) => {}
                Err(error) => return Err(error.to_string()),
            }
            if let Some(file) = read_registry(path).await? {
                return Ok(file);
            }
        }
    })
    .await
    .map_err(|_| "mesh registry did not record the configured node".to_owned())?
}

#[derive(Debug)]
struct MeshWatch {
    inotify: nix::sys::inotify::Inotify,
}

impl std::os::fd::AsRawFd for MeshWatch {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        std::os::fd::AsRawFd::as_raw_fd(&std::os::fd::AsFd::as_fd(&self.inotify))
    }
}

fn mesh_watch(directory: &Path) -> Result<tokio::io::unix::AsyncFd<MeshWatch>, String> {
    let inotify = nix::sys::inotify::Inotify::init(
        nix::sys::inotify::InitFlags::IN_CLOEXEC | nix::sys::inotify::InitFlags::IN_NONBLOCK,
    )
    .map_err(|error| error.to_string())?;
    inotify
        .add_watch(
            directory,
            nix::sys::inotify::AddWatchFlags::IN_CLOSE_WRITE
                | nix::sys::inotify::AddWatchFlags::IN_CREATE
                | nix::sys::inotify::AddWatchFlags::IN_MOVED_TO,
        )
        .map_err(|error| error.to_string())?;
    tokio::io::unix::AsyncFd::new(MeshWatch { inotify }).map_err(|error| error.to_string())
}

async fn mesh_event(watched: &tokio::io::unix::AsyncFd<MeshWatch>) -> Result<(), String> {
    loop {
        let mut ready = watched
            .readable()
            .await
            .map_err(|error| error.to_string())?;
        match ready.try_io(|inner| read_inotify(&inner.get_ref().inotify)) {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => return Err(error.to_string()),
            Err(_blocked) => {}
        }
    }
}

fn read_inotify(instance: &nix::sys::inotify::Inotify) -> std::io::Result<()> {
    match instance.read_events() {
        Ok(_) => Ok(()),
        Err(error)
            if error == nix::errno::Errno::EAGAIN || error == nix::errno::Errno::EWOULDBLOCK =>
        {
            Err(std::io::Error::from(std::io::ErrorKind::WouldBlock))
        }
        Err(error) => Err(std::io::Error::other(error)),
    }
}

async fn read_registry(path: &Path) -> Result<Option<wire::mesh::MeshFile>, String> {
    match tokio::fs::read(path).await {
        Ok(bytes) => match serde_json::from_slice::<wire::mesh::MeshFile>(&bytes) {
            Ok(file) if file.nodes.contains_key(CONFIGURED) => Ok(Some(file)),
            Ok(_) | Err(_) => Ok(None),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
