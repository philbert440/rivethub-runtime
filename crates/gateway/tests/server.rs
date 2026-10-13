use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gateway::Gateway;
use gateway::Settings;
use gateway::server::BindError;
use gateway::server::BindSource;
use gateway::server::is_bind_loopback;
use kernel::memory::MemoryStore;
use serde::Deserialize;
use tempfile::tempdir;
use testkit::FakeMemoryStore;
use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncWriteExt as _;
use tokio_util::sync::CancellationToken;

fn golden(name: &str) -> PathBuf {
    testkit::golden_path(env!("CARGO_MANIFEST_DIR"), "tests/golden").join(name)
}

fn masks() -> testkit::Masks {
    testkit::Masks {
        port: "5174".to_string(),
        hostname: Some("gold-host".to_string()),
        tmp: Some("/tmp/golden".to_string()),
    }
}

fn settings() -> Settings {
    Settings {
        hostname: "gold-host".to_string(),
        node: "golden-node".to_string(),
        allowed_origins: vec!["https://hub.example.test".to_string()],
        allowed_hosts: vec!["den.example.test".to_string()],
    }
}

fn gateway() -> Arc<Gateway> {
    let store = Arc::new(FakeMemoryStore::default());
    let handle: Arc<dyn MemoryStore> = store;
    Arc::new(Gateway::new(handle, settings()))
}

#[derive(Deserialize)]
struct Refusals {
    runs: std::collections::BTreeMap<String, RefusalRun>,
}

#[derive(Deserialize)]
struct RefusalRun {
    env: RefusalEnv,
    error: String,
}

#[derive(Deserialize)]
struct RefusalEnv {
    host: String,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    tls_ca: Option<String>,
}

#[test]
fn recorded_startup_refusals_reproduce_the_error_lines() {
    let path = golden("gateway/startup-refusals.json");
    let recording: Refusals =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("golden readable"))
            .expect("refusal golden parses");
    let tmp = tempdir().expect("temp root");
    let pki = tmp.path().join("pki");
    std::fs::create_dir_all(&pki).expect("pki dir");
    let cert = pki.join("node.crt");
    let key = pki.join("node.key");
    std::fs::write(&cert, b"cert").expect("cert fixture");
    std::fs::write(&key, b"key").expect("key fixture");
    for (name, run) in recording.runs {
        let tmp_text = tmp.path().to_str().expect("tmp");
        let material = |raw: &Option<String>| {
            raw.as_deref()
                .map(|path| testkit::substitute(path, &masks()))
                .map(|path| path.replacen("/tmp/golden", tmp_text, 1))
                .map(PathBuf::from)
        };
        let source = BindSource {
            host: run.env.host.clone(),
            port: 5174,
            tls_cert: material(&run.env.tls_cert),
            tls_key: material(&run.env.tls_key),
            tls_ca: material(&run.env.tls_ca),
        };
        let outcome = gateway::server::resolve_tls(&source);
        let expected = testkit::substitute(&run.error, &masks());
        let expected = expected.replacen("/tmp/golden", tmp_text, 1);
        match outcome {
            Err(error) => {
                assert_eq!(error.to_string(), expected, "run {name}");
            }
            Ok(_) => panic!("expected refusal for {name}"),
        }
    }
}

#[test]
fn tls_paths_resolve_when_the_files_exist() {
    let tmp = tempdir().expect("temp root");
    let cert = tmp.path().join("node.crt");
    let key = tmp.path().join("node.key");
    std::fs::write(&cert, b"cert").expect("cert fixture");
    std::fs::write(&key, b"key").expect("key fixture");
    let source = BindSource {
        host: "0.0.0.0".to_string(),
        port: 5174,
        tls_cert: Some(cert),
        tls_key: Some(key),
        tls_ca: None,
    };
    let paths = gateway::server::resolve_tls(&source)
        .expect("tls resolves")
        .expect("tls paths present");
    assert!(paths.ca.is_none());
}
#[test]
fn missing_cert_file_refuses_before_listen() {
    let source = BindSource {
        host: "0.0.0.0".to_string(),
        port: 5174,
        tls_cert: Some(std::path::PathBuf::from("/nonexistent/no-such-cert.pem")),
        tls_key: Some(std::path::PathBuf::from("/nonexistent/no-such-key.pem")),
        tls_ca: None,
    };
    let error = gateway::server::resolve_tls(&source).expect_err("missing cert refuses");
    assert!(matches!(error, BindError::Io(_)));
}

#[test]
fn bind_loopback_hosts_and_the_recorded_refusal_hosts() {
    assert!(is_bind_loopback("127.0.0.1"));
    assert!(is_bind_loopback("localhost"));
    assert!(is_bind_loopback("::1"));
    assert!(is_bind_loopback("::ffff:127.0.0.1"));
    assert!(!is_bind_loopback("0.0.0.0"));
    assert!(!is_bind_loopback(""));
    assert!(!is_bind_loopback("den.example.test"));

    let runtime = tokio::runtime::Runtime::new().expect("dev runtime");
    runtime.block_on(async {
        let free = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("probe listener");
        let port = free.local_addr().expect("addr").port();
        drop(free);
        let source = BindSource {
            host: "localhost".to_string(),
            port,
            tls_cert: None,
            tls_key: None,
            tls_ca: None,
        };
        assert!(gateway::server::bind(&source).await.is_ok());
        let source = BindSource {
            host: "::1".to_string(),
            port,
            tls_cert: None,
            tls_key: None,
            tls_ca: None,
        };
        assert!(gateway::server::bind(&source).await.is_ok());

        let occupied = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("occupied listener");
        let port = occupied.local_addr().expect("addr").port();
        let source = BindSource {
            host: "127.0.0.1".to_string(),
            port,
            tls_cert: None,
            tls_key: None,
            tls_ca: None,
        };
        let error = gateway::server::bind(&source)
            .await
            .expect_err("port in use");
        assert!(matches!(error, BindError::Io(io) if io.kind() == std::io::ErrorKind::AddrInUse));
        drop(occupied);
    });
}

#[test]
fn serve_answers_healthz_and_shuts_down_on_the_token() {
    let runtime = tokio::runtime::Runtime::new().expect("dev runtime");
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let token = CancellationToken::new();
        let server_token = token.clone();
        let server = tokio::spawn(gateway::server::serve(
            listener,
            gateway(),
            None,
            server_token,
        ));
        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .await
            .expect("request written");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.expect("response read");
        let text = String::from_utf8_lossy(&raw);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("\"ok\":true"), "{text}");
        assert!(text.contains("\"node\":\"golden-node\""), "{text}");

        token.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("serve finishes after cancellation")
            .expect("serve task");
        assert!(outcome.is_ok());
    });
}

#[test]
fn tls_serving_answers_healthz_over_rustls() {
    let runtime = tokio::runtime::Runtime::new().expect("dev runtime");
    runtime.block_on(async {
        let key_pair = rcgen::KeyPair::generate().expect("test key");
        let params =
            rcgen::CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()])
                .expect("cert params");
        let certificate = params.self_signed(&key_pair).expect("certificate");
        let tmp = tempdir().expect("temp root");
        let cert_path = tmp.path().join("node.crt");
        let key_path = tmp.path().join("node.key");
        std::fs::write(&cert_path, certificate.pem()).expect("cert written");
        std::fs::write(&key_path, key_pair.serialize_pem()).expect("key written");

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let token = CancellationToken::new();
        let server_token = token.clone();
        let tls = gateway::server::resolve_tls(&BindSource {
            host: "127.0.0.1".to_string(),
            port: addr.port(),
            tls_cert: Some(cert_path),
            tls_key: Some(key_path),
            tls_ca: None,
        })
        .expect("tls resolves");
        let server = tokio::spawn(gateway::server::serve(
            listener,
            gateway(),
            tls,
            server_token,
        ));

        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(certificate.der().clone())
            .expect("self signed root");
        let client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
        let stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let mut tls_stream = connector
            .connect("127.0.0.1".try_into().expect("server name"), stream)
            .await
            .expect("tls handshake");
        tls_stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .await
            .expect("request written");
        let mut raw = Vec::new();
        tls_stream
            .read_to_end(&mut raw)
            .await
            .expect("response read");
        let text = String::from_utf8_lossy(&raw);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");

        token.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("serve finishes after cancellation")
            .expect("serve task");
        assert!(outcome.is_ok());
    });
}

#[test]
fn settings_come_from_the_config_gateway_section() {
    let document = "den:\n  host: 0.0.0.0\n  port: 6001\n  allowed_origins:\n    - https://hub.example.test\n  allowed_hosts:\n    - den.example.test\n";
    let config: config::Config = serde_yaml_ng::from_str(document).expect("config");
    let bind = BindSource::from_config(&config);
    assert_eq!(bind.host, "0.0.0.0");
    assert_eq!(bind.port, 6001);
    let settings =
        gateway::Settings::from_config(&config, "gold-host".to_string(), "golden-node".to_string());
    assert_eq!(settings.allowed_origins, vec!["https://hub.example.test"]);
    assert_eq!(settings.allowed_hosts, vec!["den.example.test"]);

    let empty: config::Config = serde_yaml_ng::from_str("").expect("empty config");
    let bind = BindSource::from_config(&empty);
    assert_eq!(bind.host, "127.0.0.1");
    assert_eq!(bind.port, 5174);
}

#[test]
fn bind_error_lines_carry_the_recorded_text() {
    let error = BindError::TlsRequired {
        host: "0.0.0.0".to_string(),
    };
    assert_eq!(
        error.to_string(),
        "[den-server] refusing to bind 0.0.0.0: gateway TLS is required off-loopback (set RIVETOS_DEN_TLS_CERT + RIVETOS_DEN_TLS_KEY to the node cert from rivet-ca issue-node; enroll clients with rivet-ca issue-client). Or bind den.host to 127.0.0.1."
    );
    let error = BindError::CaMissing {
        path: std::path::PathBuf::from("/tmp/golden/chain.pem"),
    };
    assert_eq!(
        error.to_string(),
        "[den-server] TLS CA chain missing at /tmp/golden/chain.pem — cannot verify device client certs"
    );
}
