use axum::extract::ConnectInfo;
use axum::http::Request;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};

use super::{PeerCertificate, Tenancy, UserId};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn leaf(name: &str, unit: Option<&str>, uris: &[&str]) -> TestResult<PeerCertificate> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, name);
    if let Some(unit) = unit {
        params
            .distinguished_name
            .push(DnType::OrganizationalUnitName, unit);
    }
    for uri in uris {
        params
            .subject_alt_names
            .push(SanType::URI((*uri).try_into()?));
    }
    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok(PeerCertificate::from_verified_leaf(cert.der()))
}

fn resolve(tenancy: &Tenancy, peer: Option<PeerCertificate>, remote: &str) -> TestResult<String> {
    let (mut parts, ()) = Request::new(()).into_parts();
    parts
        .extensions
        .insert(ConnectInfo(remote.parse::<std::net::SocketAddr>()?));
    if let Some(peer) = peer {
        parts.extensions.insert(peer);
    }
    Ok(tenancy.resolve(&parts)?.as_str().to_owned())
}

fn registry() -> TestResult<Tenancy> {
    Ok(Tenancy::local(UserId::new("owner".into())?)
        .with_device("device:laptop", UserId::new("alice".into())?)?)
}

#[test]
fn registry_maps_legacy_names_and_spiffe_user_device_pairs() -> TestResult {
    let tenancy = registry()?;
    for peer in [
        leaf("device:laptop", None, &[])?,
        leaf("laptop", Some("client"), &[])?,
        leaf(
            "ignored",
            None,
            &["spiffe://rivet.mesh/device/alice/laptop"],
        )?,
    ] {
        assert_eq!(resolve(&tenancy, Some(peer), "127.0.0.2:1234")?, "alice");
    }
    assert_eq!(resolve(&tenancy, None, "127.0.0.1:1234")?, "owner");
    assert!(resolve(&tenancy, None, "127.0.0.2:1234").is_err());
    Ok(())
}

#[test]
fn unknown_or_mismatched_leaf_never_inherits_loopback_owner() -> TestResult {
    let tenancy = registry()?;
    for (name, unit, uris) in [
        ("device:stranger", None, vec![]),
        ("laptop", None, vec![]),
        ("laptop", Some("server"), vec![]),
        (
            "device:laptop",
            None,
            vec!["spiffe://rivet.mesh/device/bob/laptop"],
        ),
        (
            "device:laptop",
            None,
            vec!["spiffe://rivet.mesh/node/laptop"],
        ),
        (
            "device:laptop",
            None,
            vec!["spiffe://wrong.mesh/device/alice/laptop"],
        ),
        (
            "device:laptop",
            None,
            vec!["spiffe://rivet.mesh/device/alice/../laptop"],
        ),
        (
            "device:laptop",
            None,
            vec!["spiffe://rivet.mesh/device/alice/%6captop"],
        ),
        (
            "device:laptop",
            None,
            vec!["spiffe://rivet.mesh/device/alice/laptop?x=1"],
        ),
        (
            "device:laptop",
            None,
            vec![
                "spiffe://rivet.mesh/device/alice/laptop",
                "spiffe://rivet.mesh/device/bob/laptop",
            ],
        ),
    ] {
        assert!(resolve(&tenancy, Some(leaf(name, unit, &uris)?), "127.0.0.1:1234").is_err());
    }
    Ok(())
}

#[test]
fn explicit_unmapped_policy_applies_only_to_verified_device_names() -> TestResult {
    let tenancy = registry()?.with_unmapped_device_owner(UserId::new("owner".into())?);
    assert_eq!(
        resolve(
            &tenancy,
            Some(leaf("device:unknown", None, &[])?),
            "127.0.0.2:1234"
        )?,
        "owner"
    );
    assert_eq!(
        resolve(
            &tenancy,
            Some(leaf(
                "ignored",
                None,
                &["spiffe://rivet.mesh/device/owner/unknown"]
            )?),
            "127.0.0.2:1234"
        )?,
        "owner"
    );
    assert!(
        resolve(
            &tenancy,
            Some(leaf(
                "ignored",
                None,
                &["spiffe://rivet.mesh/device/bob/unknown"]
            )?),
            "127.0.0.2:1234"
        )
        .is_err()
    );
    assert!(
        resolve(
            &tenancy,
            Some(PeerCertificate::Unrecognized),
            "127.0.0.1:1234"
        )
        .is_err()
    );
    assert!(resolve(&tenancy, None, "127.0.0.2:1234").is_err());
    Ok(())
}

#[test]
fn ambiguous_and_invalid_device_mappings_are_refused() -> TestResult {
    assert!(
        registry()?
            .with_device("laptop", UserId::new("bob".into())?)
            .is_err()
    );
    assert!(
        registry()?
            .with_device("device:", UserId::new("alice".into())?)
            .is_err()
    );
    assert!(
        registry()?
            .with_device("device:bad\nname", UserId::new("alice".into())?)
            .is_err()
    );
    Ok(())
}
