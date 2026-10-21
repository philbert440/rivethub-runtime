use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use sqlx::ConnectOptions;

use super::Error;
use super::certificates::{self, Certificates};
use super::recording::RunKind;

pub(super) struct Fixture {
    pub(super) home: tempfile::TempDir,
    pub(super) certificates: Certificates,
    pub(super) remote: IpAddr,
}

pub(super) async fn prepare(pool: &sqlx::PgPool, kind: RunKind) -> Result<Fixture, Error> {
    let socket = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).await?;
    let remote = socket.local_addr()?.ip();
    if remote.is_loopback() || remote.is_unspecified() {
        return Err(Error::Fixture(
            "a routable non-loopback source is required".into(),
        ));
    }
    let url = pool.connect_options().to_url_lossy().to_string();
    tokio::task::spawn_blocking(move || prepare_files(remote, &url, kind)).await?
}

fn prepare_files(remote: IpAddr, url: &str, kind: RunKind) -> Result<Fixture, Error> {
    let home = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let state = home.path().join(".rivethub");
    std::fs::create_dir(&state)?;
    let certificates = certificates::generate(&state, remote)?;
    let static_dir = home.path().join("static");
    std::fs::create_dir(&static_dir)?;
    std::fs::write(
        static_dir.join("index.html"),
        include_str!("../../fixtures/tls-replay/index.txt"),
    )?;
    let reservation = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let port = reservation.local_addr()?.port();
    let mut config = format!(
        "runtime:\n  workspace: {home}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:9/v1\n    model: test\n    probe_models: false\nmemory:\n  backend: postgres\n  postgres:\n    connection_string: {url}\ngateway:\n  host: 0.0.0.0\n  port: {port}\n  tls_cert: {state}/server.pem\n  tls_key: {state}/server.key\n  static_dir: {static_dir}\n",
        home = home.path().display(),
        state = state.display(),
        static_dir = static_dir.display(),
    );
    {
        use std::fmt::Write;

        writeln!(config, "  tls_ca: {}/ca.pem", state.display())?;
        if kind == RunKind::Optional {
            writeln!(config, "  tls_require_client: false")?;
        }
    }
    if kind == RunKind::Tenancy {
        use std::fmt::Write;

        let mut users: wire::gateway::users::UsersRegistry = serde_json::from_str(include_str!(
            "../../../../wire/tests/golden/gateway/users-tenancy.json"
        ))?;
        for user in users.users.values_mut() {
            if user.pg_url.is_some() {
                user.pg_url = Some(url.to_owned());
            }
        }
        std::fs::write(state.join("users.json"), serde_json::to_vec(&users)?)?;
        let owners = std::collections::BTreeMap::from([("alice-session", "alice")]);
        let owners_file = config::paths::session_owners_file(&state);
        if let Some(parent) = owners_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(owners_file, serde_json::to_vec(&owners)?)?;
        writeln!(config, "  users_file: {}/users.json", state.display())?;
    }
    std::fs::write(state.join("config.yaml"), config)?;
    Ok(Fixture {
        home,
        certificates,
        remote,
    })
}

impl Fixture {
    pub(super) fn client(
        &self,
        certificate: Option<super::recording::ClientCertificate>,
        source: IpAddr,
    ) -> Result<reqwest::Client, Error> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .https_only(true)
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .local_address(source)
            .add_root_certificate(self.certificates.root.clone())
            .timeout(Duration::from_secs(10));
        if let Some(certificate) = certificate {
            let identity =
                self.certificates.clients.get(&certificate).ok_or_else(|| {
                    Error::Fixture(format!("missing certificate: {certificate:?}"))
                })?;
            builder = builder.identity(identity.clone());
        }
        Ok(builder.build()?)
    }

    pub(super) fn address(&self, source: super::recording::Source, port: u16) -> SocketAddr {
        let ip = match source {
            super::recording::Source::Loopback => IpAddr::V4(Ipv4Addr::LOCALHOST),
            super::recording::Source::Remote => self.remote,
        };
        SocketAddr::new(ip, port)
    }
}
