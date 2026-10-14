#[expect(
    dead_code,
    reason = "shared CA fixture exposes helpers used by the other integration targets"
)]
mod support;

use base64::Engine;
use kernel::ca::{
    CaError, CertificateAuthority, CsrPem, EnrollmentToken, Principal, SignAuthority,
};
use rcgen::{CertificateParams, KeyPair, SanType};
use serde::Deserialize;
use support::{Fixture, Result, certificate, csr, node};

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    principal: Principal,
    sans: Vec<String>,
    accepted: bool,
    days: Option<i64>,
}

#[tokio::test]
async fn replay_san_policy_and_lifetimes() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    let path = testkit::golden_path(env!("CARGO_MANIFEST_DIR"), "tests/fixtures/san-policy.json");
    let scenarios: Vec<Scenario> = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    assert_eq!(scenarios.len(), 15);
    for scenario in scenarios {
        let token = ca.issue_token(scenario.principal).await?;
        let names = scenario.sans.iter().map(String::as_str).collect::<Vec<_>>();
        let result = ca.sign(csr(&names)?, SignAuthority::Token(token)).await;
        assert_eq!(
            result.is_ok(),
            scenario.accepted,
            "{}: {result:?}",
            scenario.name
        );
        if let Ok(chain) = result {
            let der = certificate(&chain.leaf)?;
            let (_, leaf) = x509_parser::parse_x509_certificate(&der)?;
            assert_eq!(
                Some(
                    (leaf.validity().not_after.timestamp()
                        - leaf.validity().not_before.timestamp())
                        / 86400
                ),
                scenario.days
            );
            let san = leaf.subject_alternative_name()?.ok_or("missing SAN")?;
            assert_eq!(
                san.value.general_names,
                vec![x509_parser::extensions::GeneralName::URI(
                    names.first().ok_or("accepted CSR has no SAN")?
                )]
            );
            assert_eq!(leaf.subject().iter().count(), 0);
        }
    }
    Ok(())
}

#[tokio::test]
async fn renewal_requires_exact_verified_principal() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    let token = ca.issue_token(node()).await?;
    let original = ca
        .sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Token(token),
        )
        .await?;
    let renewed = ca
        .sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Renewal(node()),
        )
        .await?;
    assert_ne!(certificate(&original.leaf)?, certificate(&renewed.leaf)?);
    let error = ca
        .sign(
            csr(&["spiffe://rivet.mesh/node/node-b"])?,
            SignAuthority::Renewal(node()),
        )
        .await;
    assert!(matches!(error, Err(CaError::Refused(_))));
    Ok(())
}

#[tokio::test]
async fn malformed_csr_and_privilege_extensions_fail_closed() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    let key = KeyPair::generate()?;
    let uri = SanType::URI("spiffe://rivet.mesh/node/node-a".try_into()?);
    let mut params = CertificateParams::default();
    params.subject_alt_names = vec![uri.clone(), SanType::DnsName("example.test".try_into()?)];
    assert!(
        ca.sign(
            CsrPem(params.serialize_request(&key)?.pem()?),
            SignAuthority::Renewal(node())
        )
        .await
        .is_err()
    );
    params.subject_alt_names = vec![uri];
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    assert!(
        ca.sign(
            CsrPem(params.serialize_request(&key)?.pem()?),
            SignAuthority::Renewal(node())
        )
        .await
        .is_err()
    );
    params.is_ca = rcgen::IsCa::NoCa;
    params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
    assert!(
        ca.sign(
            CsrPem(params.serialize_request(&key)?.pem()?),
            SignAuthority::Renewal(node())
        )
        .await
        .is_err()
    );
    let wrong_curve = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384)?;
    params.key_usages.clear();
    assert!(
        ca.sign(
            CsrPem(params.serialize_request(&wrong_curve)?.pem()?),
            SignAuthority::Renewal(node())
        )
        .await
        .is_err()
    );
    let mut damaged = params.serialize_request(&key)?.der().to_vec();
    *damaged.last_mut().ok_or("empty CSR")? ^= 1;
    let encoded = format!(
        "-----BEGIN CERTIFICATE REQUEST-----\n{}\n-----END CERTIFICATE REQUEST-----\n",
        base64::engine::general_purpose::STANDARD.encode(damaged)
    );
    assert!(
        ca.sign(CsrPem(encoded), SignAuthority::Renewal(node()))
            .await
            .is_err()
    );
    let good = csr(&["spiffe://rivet.mesh/node/node-a"])?;
    for bad in [
        "not a CSR".to_owned(),
        format!("{}trailing", good.0),
        good.0.repeat(2),
        "x".repeat(16_385),
    ] {
        assert!(matches!(
            ca.sign(CsrPem(bad), SignAuthority::Renewal(node())).await,
            Err(CaError::InvalidCsr(_))
        ));
    }
    Ok(())
}

#[tokio::test]
async fn principal_segments_refuse_ambiguous_paths() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    for invalid in [
        "", ".", "..", "node/a", "a%2fb", "a?b", "a#b", "a b", "a\\b", "é",
    ] {
        assert!(matches!(
            ca.issue_token(Principal::Node {
                node: invalid.to_owned()
            })
            .await,
            Err(CaError::Refused(_))
        ));
    }
    for valid in ["node.example", "node-a", "node_a", "node~a", "a"] {
        ca.issue_token(Principal::Node {
            node: valid.to_owned(),
        })
        .await?;
    }
    assert_eq!(
        format!("{:?}", EnrollmentToken("secret".to_owned())),
        "EnrollmentToken([redacted])"
    );
    Ok(())
}
