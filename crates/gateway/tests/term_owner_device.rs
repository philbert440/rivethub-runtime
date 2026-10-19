#![cfg(test)]
#[expect(
    dead_code,
    reason = "integration binaries share fixtures and use different subsets"
)]
mod session_support;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use futures::{SinkExt as _, StreamExt as _};
use gateway::routes::devices::{DeviceRuntime, PhoneIssuer};
use gateway::server::BindSource;
use gateway::tenancy::Tenancy;
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};
use session_support::{Fixture, WAIT, user};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::Message;
use wire::gateway::devices::PairingRecord;
use wire::gateway::term::{DeviceOwner, TermServerFrame, TermSpawnRequest};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>;

const PHONE: &str = "d00112233445566778899aabbccddeeff";
const DESK: &str = "desk-hub";

struct Mesh {
    root: CertificateDer<'static>,
    root_params: CertificateParams,
    root_key: KeyPair,
}

impl Mesh {
    fn new(directory: &Path) -> TestResult<Self> {
        let root_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let mut root_params = CertificateParams::new(vec!["Test Mesh Root".to_owned()])?;
        root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let root = root_params.self_signed(&root_key)?;
        std::fs::write(directory.join("ca.pem"), root.pem())?;
        let server_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let mut server = CertificateParams::new(vec!["127.0.0.1".to_owned()])?;
        server.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let issuer = Issuer::from_params(&root_params, &root_key);
        let server = server.signed_by(&server_key, &issuer)?;
        std::fs::write(directory.join("server.pem"), server.pem())?;
        std::fs::write(directory.join("server.key"), server_key.serialize_pem())?;
        Ok(Self {
            root: root.der().clone(),
            root_params,
            root_key,
        })
    }

    fn connector(&self, device: &str) -> TestResult<TlsConnector> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let mut params = CertificateParams::default();
        params.subject_alt_names = vec![SanType::URI(
            format!("spiffe://rivet.mesh/device/owner/{device}").try_into()?,
        )];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let issuer = Issuer::from_params(&self.root_params, &self.root_key);
        let leaf = params.signed_by(&key, &issuer)?;
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.root.clone())?;
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_client_auth_cert(
            vec![leaf.der().clone()],
            PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )?;
        Ok(TlsConnector::from(Arc::new(config)))
    }
}

fn spawn(session: &str) -> TermSpawnRequest {
    TermSpawnRequest {
        session: Some(session.into()),
        command: Some("echo".into()),
        model: None,
        effort: None,
        resume: None,
        agent_id: None,
        force: None,
        cols: None,
        rows: None,
    }
}

fn paired_phone(pairing: &Path) -> TestResult<DeviceRuntime> {
    let claimed = pairing.join("claimed");
    std::fs::create_dir_all(&claimed)?;
    let record = PairingRecord {
        v: 1,
        device_id: PHONE.to_owned(),
        name: "pixel-9".to_owned(),
        expires_at: chrono::Utc::now(),
    };
    std::fs::write(claimed.join("spent.json"), serde_json::to_vec(&record)?)?;
    let issuer = PhoneIssuer::try_new(
        pairing.to_path_buf(),
        "https://127.0.0.1:5174".to_owned(),
        "owner".to_owned(),
    )?;
    Ok(DeviceRuntime::unconfigured().enable_phone_issuer(issuer))
}

async fn connect(
    address: SocketAddr,
    connector: TlsConnector,
) -> TestResult<(Socket, TermServerFrame)> {
    let tcp = tokio::net::TcpStream::connect(address).await?;
    let tls = connector
        .connect(ServerName::try_from("127.0.0.1")?, tcp)
        .await?;
    let url = format!("wss://127.0.0.1:{}/term?id=pty-1", address.port());
    let (mut socket, _) = tokio_tungstenite::client_async(url, tls).await?;
    let hello = next(&mut socket).await?;
    let replay = tokio::time::timeout(WAIT, socket.next())
        .await?
        .ok_or("socket closed")??;
    if !replay.is_binary() {
        return Err(format!("expected the replay, got {replay:?}").into());
    }
    Ok((socket, hello))
}

async fn next(socket: &mut Socket) -> TestResult<TermServerFrame> {
    let message = tokio::time::timeout(WAIT, socket.next())
        .await?
        .ok_or("socket closed")??;
    Ok(serde_json::from_str(message.to_text()?)?)
}

#[tokio::test(flavor = "current_thread")]
async fn owner_frames_name_the_device_that_holds_the_terminal() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mesh = Mesh::new(directory.path())?;
    let fixture = Fixture::new().await;
    fixture
        .service
        .spawn(&user("owner"), spawn("owned"))
        .await?;
    let gateway = Arc::try_unwrap(fixture.gateway)
        .map_err(|_| "gateway is shared")?
        .with_tenancy(Tenancy::local(user("owner")).with_unmapped_device_owner(user("owner")))
        .with_devices(paired_phone(&directory.path().join("pairing"))?);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let tls = gateway::tls::resolve_tls(&BindSource {
        host: "127.0.0.1".to_owned(),
        port: address.port(),
        tls_cert: Some(directory.path().join("server.pem")),
        tls_key: Some(directory.path().join("server.key")),
        tls_ca: Some(directory.path().join("ca.pem")),
        tls_crl: None,
        require_client: true,
        notices: Vec::new(),
    })?;
    let stop = tokio_util::sync::CancellationToken::new();
    let server = tokio::spawn(gateway::server::serve(
        listener,
        Arc::new(gateway),
        tls,
        stop.clone(),
    ));
    let (mut phone, _) = connect(address, mesh.connector(PHONE)?).await?;
    phone
        .send(Message::text(r#"{"type":"resize","cols":90,"rows":30}"#))
        .await?;
    let claimed = next(&mut phone).await?;
    assert!(
        matches!(&claimed, TermServerFrame::Owner { device: Some(device), self_owner: true, .. } if device == "pixel-9"),
        "{claimed:?}"
    );
    let (mut desk, hello) = connect(address, mesh.connector(DESK)?).await?;
    assert!(
        matches!(&hello, TermServerFrame::Hello { owner: Some(DeviceOwner { device: Some(device), self_owner: false }), .. } if device == "pixel-9"),
        "{hello:?}"
    );
    desk.send(Message::text(r#"{"type":"claim"}"#)).await?;
    let moved = next(&mut phone).await?;
    assert!(
        matches!(&moved, TermServerFrame::Owner { device: Some(device), self_owner: false, .. } if device == DESK),
        "{moved:?}"
    );
    drop((phone, desk));
    stop.cancel();
    tokio::time::timeout(WAIT, server).await???;
    Ok(())
}
