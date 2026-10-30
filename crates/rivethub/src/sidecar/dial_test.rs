use std::collections::BTreeSet;

use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::ExtendedKeyUsagePurpose;
use rcgen::IsCa;
use rcgen::Issuer;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use serde::Deserialize;

use super::DialError;
use super::choose;

struct Leaf {
    cert: String,
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
    let key = KeyPair::generate().expect("leaf key");
    let mut params =
        CertificateParams::new(names.iter().copied().map(str::to_owned).collect::<Vec<_>>())
            .expect("leaf");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let issuer = Issuer::from_params(&ca_params, &ca_key);
    let cert = params.signed_by(&key, &issuer).expect("signed");
    Leaf { cert: cert.pem() }
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

#[test]
fn a_recorded_node_certificate_without_a_loopback_name_is_refused() {
    let pem = recorded_crt();
    assert!(matches!(
        choose(&pem, "0.0.0.0"),
        Err(DialError::GatewayName)
    ));
    let missing = choose(&pem, "0.0.0.0").expect_err("san");
    let missing_text = missing.to_string();
    assert!(missing_text.contains("127.0.0.1"), "{missing_text}");
    assert!(missing_text.contains("reissue"), "{missing_text}");
    let remote = choose(&pem, "192.0.2.55").expect_err("bind");
    let remote_text = remote.to_string();
    assert!(matches!(
        remote,
        DialError::GatewayBind { host } if host == "192.0.2.55"
    ));
    assert!(remote_text.contains("loopback owner"), "{remote_text}");
    assert!(remote_text.contains("127.0.0.1"), "{remote_text}");
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
fn wildcard_and_loopback_binds_dial_a_name_on_the_certificate() {
    let both = mint(&["only.example.test", "127.0.0.1", "localhost"]);
    assert_eq!(choose(&both.cert, "0.0.0.0").expect("wild"), "127.0.0.1");
    assert_eq!(choose(&both.cert, "127.0.0.1").expect("bind"), "127.0.0.1");
    assert_eq!(
        choose(&both.cert, "::ffff:127.0.0.1").expect("mapped"),
        "127.0.0.1"
    );
    let name = mint(&["localhost", "node.example.test"]);
    assert_eq!(choose(&name.cert, "LocalHost").expect("dns"), "localhost");
    assert_eq!(
        choose(&name.cert, "0.0.0.0").expect("fallback"),
        "localhost"
    );
    let v6 = mint(&["::1", "node.example.test"]);
    assert_eq!(choose(&v6.cert, "::").expect("wild6"), "[::1]");
    assert_eq!(choose(&v6.cert, "[::1]").expect("bind6"), "[::1]");
    let local_only = mint(&["localhost"]);
    assert_eq!(
        choose(&local_only.cert, "::").expect("v6 name"),
        "localhost"
    );
    let public = mint(&["only.example.test", "192.0.2.20"]);
    assert!(matches!(
        choose(&public.cert, "0.0.0.0"),
        Err(DialError::GatewayName)
    ));
    assert!(matches!(
        choose(&public.cert, "127.0.0.1"),
        Err(DialError::GatewayName)
    ));
    assert!(matches!(
        choose(&v6.cert, "0.0.0.0"),
        Err(DialError::GatewayName)
    ));
    let remote = choose(&both.cert, "192.0.2.55").expect_err("remote");
    assert!(matches!(
        remote,
        DialError::GatewayBind { host } if host == "192.0.2.55"
    ));
}
