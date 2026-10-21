#[derive(Debug, serde::Deserialize)]
struct Catalog {
    cases: Vec<Case>,
    #[serde(rename = "crlPem")]
    crl_pem: String,
}

#[derive(Debug, serde::Deserialize)]
struct Case {
    id: String,
    response: Response,
}

#[derive(Debug, serde::Deserialize)]
struct Response {
    status: u16,
    json: Body,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(untagged)]
enum StatusField {
    Code(u16),
    Text(String),
}

#[derive(Debug, serde::Deserialize)]
struct Body {
    #[serde(default)]
    status: Option<StatusField>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default, rename = "tlsOptions")]
    tls_options: Option<stepca::TlsOptions>,
}

const EXERCISED: &[&str] = &[
    "sign-with-token",
    "sign-token-reused",
    "renew-mtls",
    "renew-revoked",
];

const SKIPPED: &[(&str, &str)] = &[
    (
        "sign-san-not-granted",
        "The holder builds its CSR only from the sans claim of the one-time token, so it never asks step-ca for a name the token did not pin.",
    ),
    (
        "renew-without-certificate",
        "Renewal always presents the stored holder certificate over mTLS, and the command refuses to call renew when that certificate is absent.",
    ),
    (
        "revoke-own-mtls",
        "Revocation is a later certificate-authority operation and rh cert renew never calls the step-ca revoke endpoint.",
    ),
    (
        "roots",
        "The holder trusts only the operator-supplied trust root and does not fetch the step-ca roots endpoint.",
    ),
    (
        "crl",
        "The certificate revocation list is consumed by the gateway slice, and rh cert renew does not download or apply it.",
    ),
];

#[test]
fn stepca_recordings_are_exercised_or_skipped_with_a_reason()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog: Catalog =
        serde_json::from_slice(&std::fs::read(testkit::recording("stepca/api.json"))?)?;
    assert_ne!(catalog.crl_pem, "");
    let mut expected = std::collections::BTreeSet::new();
    for case in &catalog.cases {
        assert!(
            expected.insert(case.id.as_str()),
            "duplicate case {}",
            case.id
        );
    }
    assert!(expected.insert("crl"));
    let mut covered = std::collections::BTreeSet::new();
    for name in EXERCISED {
        assert!(covered.insert(*name), "duplicate exercised case {name}");
    }
    for (name, reason) in SKIPPED {
        assert!(reason.len() >= 60, "{name} reason is too short");
        assert!(covered.insert(*name), "duplicate skipped case {name}");
    }
    assert_eq!(covered, expected);
    let sign = catalog
        .cases
        .iter()
        .find(|case| case.id == "sign-with-token")
        .ok_or("missing sign-with-token")?;
    let tls = sign
        .response
        .json
        .tls_options
        .clone()
        .ok_or("missing tls options")?;
    assert_eq!(
        tls.cipher_suites,
        vec![
            stepca::CipherSuite::Chacha20Poly1305,
            stepca::CipherSuite::Aes128Gcm,
        ]
    );
    assert_eq!(
        (tls.min_version, tls.max_version, tls.renegotiation),
        (stepca::TlsVersion::V12, stepca::TlsVersion::V13, false)
    );
    for id in ["sign-token-reused", "renew-revoked", "sign-san-not-granted"] {
        let case = catalog.cases.iter().find(|case| case.id == id).ok_or(id)?;
        let message = case.response.json.message.as_deref().ok_or(id)?;
        assert_ne!(message, "");
        match case.response.json.status.clone().ok_or(id)? {
            StatusField::Code(code) => assert_eq!(case.response.status, code),
            StatusField::Text(text) => {
                return Err(format!("recorded case {id} status is {text}").into());
            }
        }
    }
    Ok(())
}
