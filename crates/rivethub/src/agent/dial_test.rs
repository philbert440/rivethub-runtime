use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use config::Config;
use config::Environment;
use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::ExtendedKeyUsagePurpose;
use rcgen::IsCa;
use rcgen::Issuer;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use serde::Deserialize;

use super::super::assemble::AgentError;
use super::super::assemble::Plan;
use super::choose;

struct Leaf {
    cert: String,
    key: String,
    ca: String,
}

#[derive(Deserialize)]
struct Recording {
    cases: Vec<RecordedCase>,
}

#[derive(Deserialize)]
struct RecordedCase {
    id: String,
    response: RecordedResponse,
}

#[derive(Deserialize)]
struct RecordedResponse {
    json: RecordedBody,
}

#[derive(Deserialize)]
struct RecordedBody {
    #[serde(default)]
    crt: Option<String>,
}

#[derive(Deserialize)]
struct SkipFile {
    skipped: Vec<Skip>,
}

#[derive(Deserialize)]
struct Skip {
    name: String,
    reason: String,
}

fn mint(names: &[&str]) -> Leaf {
    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca_params =
        CertificateParams::new(vec!["ca.example.test".to_owned()]).expect("ca name");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca = ca_params.self_signed(&ca_key).expect("ca");
    let key = KeyPair::generate().expect("leaf key");
    let mut params =
        CertificateParams::new(names.iter().copied().map(str::to_owned).collect::<Vec<_>>())
            .expect("leaf");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let issuer = Issuer::from_params(&ca_params, &ca_key);
    let cert = params.signed_by(&key, &issuer).expect("signed");
    Leaf {
        cert: cert.pem(),
        key: key.serialize_pem(),
        ca: ca.pem(),
    }
}

fn recorded_crt() -> String {
    let text = std::fs::read_to_string(testkit::recording("stepca/api.json")).expect("recording");
    let recording: Recording = serde_json::from_str(&text).expect("recording json");
    recording
        .cases
        .into_iter()
        .find(|case| case.id == "sign-with-token")
        .expect("sign-with-token")
        .response
        .json
        .crt
        .expect("recorded certificate")
}

fn store(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("pem");
    path
}

fn document(gateway: &str, mesh: &str) -> String {
    [
        "runtime:",
        "  workspace: /tmp",
        "  default_agent: main",
        "agents:",
        "  main:",
        "    provider: p",
        "providers:",
        "  p:",
        "    type: openai-compatible",
        "    base_url: http://127.0.0.1:9/v1",
        "    model: m",
        "memory:",
        "  backend: postgres",
        "  postgres:",
        "    connection_string: postgres://u@127.0.0.1:9/db",
        gateway,
        mesh,
    ]
    .join("\n")
}

fn gateway_block(host: &str, cert: &Path, key: &Path, ca: Option<&Path>) -> String {
    let ca_line = match ca {
        Some(path) => format!("  tls_ca: {}\n", path.display()),
        None => String::new(),
    };
    format!(
        "den:\n  host: {host}\n  port: 8443\n  tls_cert: {}\n  tls_key: {}\n{ca_line}",
        cert.display(),
        key.display()
    )
}

fn mesh_true(advertise: &str) -> String {
    format!(
        "mesh:\n  enabled: true\n  node_name: test-node\n  advertise_host: {advertise}\n  tls: true\n"
    )
}

fn mesh_paths(advertise: &str, cert: &Path, key: &Path, ca: &Path) -> String {
    format!(
        "mesh:\n  enabled: true\n  node_name: test-node\n  advertise_host: {advertise}\n  tls:\n    cert_path: {}\n    key_path: {}\n    ca_path: {}\n",
        cert.display(),
        key.display(),
        ca.display()
    )
}

async fn open_plan(text: &str) -> Result<Plan, AgentError> {
    let home = tempfile::tempdir().expect("home");
    let home_text = home.path().to_str().expect("home path");
    let env = Environment::from_vars([("HOME", home_text)]);
    let loaded = Config::from_str(&env, text).expect("config");
    let planned = super::super::assemble::plan(&loaded.config, &env).await;
    drop(home);
    planned
}

#[test]
fn recorded_certificate_sans_select_the_dial_host() {
    let pem = recorded_crt();
    assert_eq!(
        choose(&pem, "0.0.0.0", Some("rec-node.example.test")).expect("dns"),
        "rec-node.example.test"
    );
    assert_eq!(
        choose(&pem, "0.0.0.0", Some("REC-NODE.EXAMPLE.TEST")).expect("case"),
        "rec-node.example.test"
    );
    assert_eq!(
        choose(&pem, "0.0.0.0", Some("192.0.2.15")).expect("ip"),
        "192.0.2.15"
    );
    assert!(matches!(
        choose(&pem, "0.0.0.0", Some("127.0.0.1")),
        Err(AgentError::AdvertiseHost)
    ));
    assert_eq!(
        choose(&pem, "0.0.0.0", None).expect("sole dns"),
        "rec-node.example.test"
    );
    assert_eq!(
        choose(&pem, "192.0.2.15", None).expect("bind ip"),
        "192.0.2.15"
    );
}

#[test]
fn out_of_slice_scenarios_are_named_with_a_reason() {
    let file: SkipFile =
        serde_json::from_str(include_str!("../../tests/fixtures/gateway-tls/skips.json"))
            .expect("skips");
    let mut names = BTreeSet::new();
    for entry in &file.skipped {
        assert!(!entry.name.is_empty(), "skip name");
        assert!(!entry.reason.is_empty(), "skip reason");
        assert!(names.insert(entry.name.as_str()), "duplicate skip");
    }
}

#[test]
fn generated_names_follow_advertise_bind_and_a_single_san() {
    let both = mint(&["only.example.test", "192.0.2.20"]);
    assert_eq!(
        choose(&both.cert, "0.0.0.0", None).expect("dns"),
        "only.example.test"
    );
    assert_eq!(
        choose(&both.cert, "127.0.0.1", Some("only.example.test")).expect("advertise"),
        "only.example.test"
    );
    let loopback = mint(&["node.example.test", "127.0.0.1"]);
    assert_eq!(
        choose(&loopback.cert, "127.0.0.1", None).expect("loopback"),
        "127.0.0.1"
    );
    let ip = mint(&["2001:db8::15"]);
    assert_eq!(choose(&ip.cert, "::", None).expect("v6"), "[2001:db8::15]");
    assert_eq!(
        choose(&ip.cert, "[2001:db8::15]", None).expect("bracket"),
        "[2001:db8::15]"
    );
}

#[test]
fn a_certificate_without_one_dialable_name_is_refused() {
    let wild = mint(&["*.example.test"]);
    assert!(matches!(
        choose(&wild.cert, "0.0.0.0", None),
        Err(AgentError::GatewayName)
    ));
    let many = mint(&["a.example.test", "b.example.test"]);
    assert!(matches!(
        choose(&many.cert, "0.0.0.0", None),
        Err(AgentError::GatewayName)
    ));
    assert_eq!(
        choose(&many.cert, "0.0.0.0", Some("b.example.test")).expect("one name"),
        "b.example.test"
    );
    assert!(matches!(
        choose(&many.cert, "192.0.2.8", Some("127.0.0.1")),
        Err(AgentError::AdvertiseHost)
    ));
}

#[tokio::test]
async fn an_advertise_host_on_the_certificate_becomes_the_gateway_url() {
    let dir = tempfile::tempdir().expect("dir");
    let leaf = mint(&["rec-node.example.test", "192.0.2.15"]);
    let cert = store(dir.path(), "cert.pem", &leaf.cert);
    let key = store(dir.path(), "key.pem", &leaf.key);
    let ca = store(dir.path(), "ca.pem", &leaf.ca);
    let text = document(
        &gateway_block("0.0.0.0", &cert, &key, Some(&ca)),
        &mesh_true("rec-node.example.test"),
    );
    let plan = open_plan(&text).await.expect("plan");
    assert_eq!(plan.gateway.as_str(), "https://rec-node.example.test:8443/");
    let trust = plan.gateway_trust.expect("trust");
    assert!(trust.presents_certificate());
    assert_eq!(
        trust,
        tasks::MeshIdentity::from_pem(&leaf.cert, &leaf.key, &leaf.ca).expect("identity")
    );
}

#[tokio::test]
async fn an_advertise_host_off_the_certificate_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let leaf = mint(&["rec-node.example.test"]);
    let cert = store(dir.path(), "cert.pem", &leaf.cert);
    let key = store(dir.path(), "key.pem", &leaf.key);
    let ca = store(dir.path(), "ca.pem", &leaf.ca);
    let text = document(
        &gateway_block("0.0.0.0", &cert, &key, Some(&ca)),
        &mesh_true("127.0.0.1"),
    );
    let error = open_plan(&text).await.expect_err("advertise");
    assert!(matches!(error, AgentError::AdvertiseHost));
}

#[tokio::test]
async fn a_gateway_without_a_ca_keeps_the_san_and_carries_no_trust() {
    let dir = tempfile::tempdir().expect("dir");
    let leaf = mint(&["rec-node.example.test"]);
    let cert = store(dir.path(), "cert.pem", &leaf.cert);
    let key = store(dir.path(), "key.pem", &leaf.key);
    let text = document(&gateway_block("0.0.0.0", &cert, &key, None), "");
    let plan = open_plan(&text).await.expect("plan");
    assert_eq!(plan.gateway.as_str(), "https://rec-node.example.test:8443/");
    assert!(plan.gateway_trust.is_none());
}

#[tokio::test]
async fn mesh_client_material_without_a_gateway_ca_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let leaf = mint(&["rec-node.example.test"]);
    let cert = store(dir.path(), "cert.pem", &leaf.cert);
    let key = store(dir.path(), "key.pem", &leaf.key);
    let text = document(
        &gateway_block("0.0.0.0", &cert, &key, None),
        &mesh_true("rec-node.example.test"),
    );
    let error = open_plan(&text).await.expect_err("ca");
    assert!(matches!(error, AgentError::GatewayCa));
}

#[tokio::test]
async fn an_invalid_gateway_certificate_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let leaf = mint(&["rec-node.example.test"]);
    let cert = store(dir.path(), "cert.pem", "not a certificate\n");
    let key = store(dir.path(), "key.pem", &leaf.key);
    let ca = store(dir.path(), "ca.pem", &leaf.ca);
    let text = document(&gateway_block("127.0.0.1", &cert, &key, Some(&ca)), "");
    let error = open_plan(&text).await.expect_err("pem");
    assert!(matches!(error, AgentError::GatewayCertificate));
}

#[tokio::test]
async fn http_keeps_the_bind_host_and_carries_no_trust() {
    let plan = open_plan(&document("den:\n  host: 127.0.0.1\n  port: 5174\n", "")).await;
    let plan = plan.expect("plan");
    assert_eq!(plan.gateway.as_str(), "http://127.0.0.1:5174/");
    assert!(plan.gateway_trust.is_none());
}

#[tokio::test]
async fn one_ipv6_san_is_dialled_with_brackets_and_ca_only_trust() {
    let dir = tempfile::tempdir().expect("dir");
    let leaf = mint(&["2001:db8::15"]);
    let cert = store(dir.path(), "cert.pem", &leaf.cert);
    let key = store(dir.path(), "key.pem", &leaf.key);
    let ca = store(dir.path(), "ca.pem", &leaf.ca);
    let text = document(&gateway_block("0.0.0.0", &cert, &key, Some(&ca)), "");
    let plan = open_plan(&text).await.expect("plan");
    assert_eq!(plan.gateway.as_str(), "https://[2001:db8::15]:8443/");
    let trust = plan.gateway_trust.expect("trust");
    assert!(!trust.presents_certificate());
    assert_eq!(trust, tasks::MeshIdentity::from_ca(&leaf.ca).expect("ca"));
}

#[tokio::test]
async fn mesh_client_material_trusts_the_gateway_ca() {
    let dir = tempfile::tempdir().expect("dir");
    let server = mint(&["rec-node.example.test"]);
    let client = mint(&["client.example.test"]);
    let foreign = mint(&["other.example.test"]);
    let server_cert = store(dir.path(), "server.pem", &server.cert);
    let server_key = store(dir.path(), "server.key", &server.key);
    let server_ca = store(dir.path(), "server-ca.pem", &server.ca);
    let client_cert = store(dir.path(), "client.pem", &client.cert);
    let client_key = store(dir.path(), "client.key", &client.key);
    let foreign_ca = store(dir.path(), "foreign-ca.pem", &foreign.ca);
    let text = document(
        &gateway_block("0.0.0.0", &server_cert, &server_key, Some(&server_ca)),
        &mesh_paths(
            "rec-node.example.test",
            &client_cert,
            &client_key,
            &foreign_ca,
        ),
    );
    let plan = open_plan(&text).await.expect("plan");
    let trust = plan.gateway_trust.expect("trust");
    assert!(trust.presents_certificate());
    assert_eq!(
        trust,
        tasks::MeshIdentity::from_pem(&client.cert, &client.key, &server.ca).expect("gateway ca")
    );
    assert_ne!(
        trust,
        tasks::MeshIdentity::from_pem(&client.cert, &client.key, &foreign.ca).expect("other ca")
    );
}
