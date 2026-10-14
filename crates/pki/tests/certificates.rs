#[expect(
    dead_code,
    reason = "shared CA fixture exposes helpers used by the other integration targets"
)]
mod support;

use kernel::ca::{CaError, CertificateAuthority, SignAuthority};
use pki::Pki;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{
    CertificateDer, CertificateRevocationListDer, CertificateSigningRequestDer, UnixTime,
};
use support::{Fixture, Result, certificate, csr, node};
use tokio_util::sync::CancellationToken;
use x509_parser::prelude::FromDer;

#[tokio::test]
async fn imported_ca_signs_holder_public_key_and_webpki_validates_chain() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    let request = csr(&["spiffe://rivet.mesh/node/node-a"])?;
    let der = CertificateSigningRequestDer::from_pem_slice(request.0.as_bytes())?;
    let (_, parsed) = x509_parser::certification_request::X509CertificationRequest::from_der(&der)?;
    let token = ca.issue_token(node()).await?;
    let chain = ca.sign(request, SignAuthority::Token(token)).await?;
    let leaf = CertificateDer::from(certificate(&chain.leaf)?);
    let (_, parsed_leaf) = x509_parser::parse_x509_certificate(&leaf)?;
    assert_eq!(
        parsed_leaf.public_key().raw,
        parsed.certification_request_info.subject_pki.raw
    );
    assert_eq!(parsed_leaf.raw_serial(), &[0x10, 0x02]);
    assert_eq!(chain.chain, ca.chain().await?);
    let intermediate = CertificateDer::from(certificate(
        chain.chain.first().ok_or("missing intermediate")?,
    )?);
    let root = CertificateDer::from(certificate(chain.chain.get(1).ok_or("missing root")?)?);
    let anchor = webpki::anchor_from_trusted_cert(&root)?;
    webpki::EndEntityCert::try_from(&leaf)?.verify_for_usage(
        webpki::ALL_VERIFICATION_ALGS,
        &[anchor],
        &[intermediate],
        UnixTime::now(),
        webpki::KeyUsage::client_auth(),
        None,
        None,
    )?;
    drop(ca);
    std::fs::remove_dir_all(&fixture.source)?;
    let ca = Pki::open(fixture.state.clone(), CancellationToken::new()).await?;
    let renewed = ca
        .sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Renewal(node()),
        )
        .await?;
    let der = certificate(&renewed.leaf)?;
    let (_, cert) = x509_parser::parse_x509_certificate(&der)?;
    assert_eq!(cert.raw_serial(), &[0x10, 0x03]);
    Ok(())
}

#[tokio::test]
async fn crl_contains_imported_and_new_revocations_and_verifies() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    let first = ca
        .sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Renewal(node()),
        )
        .await?;
    ca.sign(
        csr(&["spiffe://rivet.mesh/node/node-a"])?,
        SignAuthority::Renewal(node()),
    )
    .await?;
    let outstanding = ca.issue_token(node()).await?;
    ca.revoke(node()).await?;
    ca.revoke_serial("1001".to_owned()).await?;
    ca.revoke(node()).await?;
    assert!(matches!(
        ca.issue_token(node()).await,
        Err(CaError::Refused(_))
    ));
    assert!(matches!(
        ca.sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Renewal(node())
        )
        .await,
        Err(CaError::Refused(_))
    ));
    assert!(matches!(
        ca.sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Token(outstanding)
        )
        .await,
        Err(CaError::Refused(_))
    ));
    let pem = ca.crl().await?;
    let crl_der = CertificateRevocationListDer::from_pem_slice(pem.0.as_bytes())?;
    let (_, crl) = x509_parser::parse_x509_crl(&crl_der)?;
    let intermediate = certificate(first.chain.first().ok_or("missing intermediate")?)?;
    let (_, issuer) = x509_parser::parse_x509_certificate(&intermediate)?;
    crl.verify_signature(issuer.public_key())?;
    assert_eq!(crl.issuer(), issuer.subject());
    let revoked = crl
        .iter_revoked_certificates()
        .map(|entry| entry.raw_serial().to_vec())
        .collect::<Vec<_>>();
    assert_eq!(
        revoked,
        vec![
            vec![0x10, 0x00],
            vec![0x10, 0x01],
            vec![0x10, 0x02],
            vec![0x10, 0x03]
        ]
    );
    rejects_revoked_leaf(&first, &crl_der)?;
    drop(ca);
    let ca = Pki::open(fixture.state.clone(), CancellationToken::new()).await?;
    assert!(
        ca.sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Renewal(node())
        )
        .await
        .is_err()
    );
    let after_restart = ca.crl().await?;
    let item = CertificateRevocationListDer::from_pem_slice(after_restart.0.as_bytes())?;
    let (_, restarted) = x509_parser::parse_x509_crl(&item)?;
    assert_eq!(restarted.iter_revoked_certificates().count(), 4);
    assert!(restarted.crl_number() > crl.crl_number());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn refuses_lifetime_beyond_issuer_validity() -> Result {
    let fixture = Fixture::new()?;
    let ca = fixture.import().await?;
    tokio::time::advance(std::time::Duration::from_hours(360 * 24)).await;
    assert!(matches!(
        ca.sign(
            csr(&["spiffe://rivet.mesh/node/node-a"])?,
            SignAuthority::Renewal(node())
        )
        .await,
        Err(CaError::Refused(_))
    ));
    Ok(())
}

fn rejects_revoked_leaf(first: &kernel::ca::CertChainPem, crl_der: &[u8]) -> Result {
    let root = CertificateDer::from(certificate(first.chain.get(1).ok_or("missing root")?)?);
    let leaf = CertificateDer::from(certificate(&first.leaf)?);
    let intermediate = CertificateDer::from(certificate(
        first.chain.first().ok_or("missing intermediate")?,
    )?);
    let anchor = webpki::anchor_from_trusted_cert(&root)?;
    let web_crl = webpki::BorrowedCertRevocationList::from_der(crl_der)?.into();
    let crls = [&web_crl];
    let options = webpki::RevocationOptionsBuilder::new(&crls)
        .map_err(|_| "CRL list is empty")?
        .with_depth(webpki::RevocationCheckDepth::EndEntity)
        .build();
    let end_entity = webpki::EndEntityCert::try_from(&leaf)?;
    let anchors = [anchor];
    let intermediates = [intermediate];
    let result = end_entity.verify_for_usage(
        webpki::ALL_VERIFICATION_ALGS,
        &anchors,
        &intermediates,
        UnixTime::now(),
        webpki::KeyUsage::client_auth(),
        Some(options),
        None,
    );
    assert!(matches!(result, Err(webpki::Error::CertRevoked)));
    Ok(())
}
