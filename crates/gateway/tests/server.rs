use std::path::PathBuf;

use gateway::server::BindError;
use gateway::server::BindSource;
use gateway::server::is_bind_loopback;
use gateway::tls::resolve_tls;
use serde::Deserialize;
use tempfile::tempdir;

fn masks() -> testkit::Masks {
    testkit::Masks {
        port: "5174".to_string(),
        hostname: Some("gold-host".to_string()),
        tmp: Some("/tmp/golden".to_string()),
    }
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
    let path = testkit::recording("gateway/startup.json");
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
            tls_crl: None,
        };
        let outcome = resolve_tls(&source);
        let expected = testkit::substitute(
            &testkit::translate_recorded_product_names(&run.error),
            &masks(),
        );
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
        tls_crl: None,
    };
    let paths = resolve_tls(&source)
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
        tls_crl: None,
    };
    let error = resolve_tls(&source).expect_err("missing cert refuses");
    assert!(matches!(error, BindError::Io(_)));
}

#[tokio::test]
async fn bind_loopback_hosts_and_the_recorded_refusal_hosts() {
    assert!(is_bind_loopback("127.0.0.1"));
    assert!(is_bind_loopback("localhost"));
    assert!(is_bind_loopback("::1"));
    assert!(is_bind_loopback("::ffff:127.0.0.1"));
    assert!(!is_bind_loopback("0.0.0.0"));
    assert!(!is_bind_loopback(""));
    assert!(!is_bind_loopback("gateway.example.test"));

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
        tls_crl: None,
    };
    assert!(gateway::server::bind(&source).await.is_ok());
    let source = BindSource {
        host: "::1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        tls_ca: None,
        tls_crl: None,
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
        tls_crl: None,
    };
    let error = gateway::server::bind(&source)
        .await
        .expect_err("port in use");
    assert!(matches!(error, BindError::Io(io) if io.kind() == std::io::ErrorKind::AddrInUse));
    drop(occupied);
}

#[test]
fn settings_come_from_the_config_gateway_section() {
    let document = "den:\n  host: 0.0.0.0\n  port: 6001\n  allowed_origins:\n    - https://hub.example.test\n  allowed_hosts:\n    - gateway.example.test\n";
    let config: config::Config = serde_yaml_ng::from_str(document).expect("config");
    let bind = BindSource::from_config(&config);
    assert_eq!(bind.host, "0.0.0.0");
    assert_eq!(bind.port, 6001);
    let home = tempdir().expect("home");
    let env = config::Environment::from_vars([("HOME", home.path().to_str().expect("utf-8 home"))]);
    let settings = gateway::Settings::from_config_in(
        &config,
        &env,
        "gold-host".to_string(),
        "golden-node".to_string(),
    );
    assert_eq!(settings.allowed_origins, vec!["https://hub.example.test"]);
    assert_eq!(settings.allowed_hosts, vec!["gateway.example.test"]);

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
        "[rivethub-gateway] refusing to bind 0.0.0.0: gateway TLS is required off-loopback (set RIVETHUB_TLS_CERT + RIVETHUB_TLS_KEY to the node cert from rivet-ca issue-node; enroll clients with rivet-ca issue-client). Or bind the gateway host to 127.0.0.1."
    );
    let error = BindError::CaMissing {
        path: std::path::PathBuf::from("/tmp/golden/chain.pem"),
    };
    assert_eq!(
        error.to_string(),
        "[rivethub-gateway] TLS CA chain missing at /tmp/golden/chain.pem — cannot verify device client certs"
    );
}
