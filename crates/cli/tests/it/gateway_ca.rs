#[path = "gateway_ca_support/mod.rs"]
mod gateway_ca_support;

use gateway_ca_support::Lab;
use gateway_ca_support::Material;
use gateway_ca_support::crl_serials;
use gateway_ca_support::csr;
use gateway_ca_support::identity_pem;
use gateway_ca_support::serial;
use gateway_ca_support::uri_san;
use kernel::ca::CertificateAuthority;
use kernel::ca::CsrPem;
use kernel::ca::EnrollmentToken;
use kernel::ca::Principal;
use kernel::ca::SignAuthority;
use wire::ca::SignResponse;
use wire::gateway::GatewayError;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_sign_needs_no_client_certificate_and_publishes_chain_and_crl() -> Result {
    let lab = Material::open()?.serve().await?;
    let principal = Principal::Node {
        node: "node-a".to_owned(),
    };
    let uri = principal.spiffe_uri()?;
    let (csr_pem, _key) = csr(&uri)?;
    let token = lab.pki.issue_token(principal).await?;
    let signed = lab.post_sign(&csr_pem, Some(&token.0), None).await?;
    assert_eq!(signed.status, reqwest::StatusCode::OK);
    assert_eq!(signed.content_type.as_deref(), Some("application/json"));
    let body: SignResponse = serde_json::from_str(&signed.body)?;
    assert_eq!(uri_san(&body.leaf)?, uri);
    let chain = lab.get("/ca/v1/chain", None).await?;
    assert_eq!(chain.status, reqwest::StatusCode::OK);
    assert_eq!(
        chain.content_type.as_deref(),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(chain.body, body.chain.join(""));
    assert_eq!(chain.body, lab.pki.chain().await?.join(""));
    let crl = lab.get("/ca/v1/crl", None).await?;
    assert_eq!(crl.status, reqwest::StatusCode::OK);
    assert_eq!(crl_serials(&crl.body)?, Vec::<Vec<u8>>::new());
    let missing = lab.post_sign(&csr_pem, None, None).await?;
    assert_eq!(missing.status, reqwest::StatusCode::UNAUTHORIZED);
    let error: GatewayError = serde_json::from_str(&missing.body)?;
    assert_eq!(error.error, "unauthorized");
    lab.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renewal_uses_the_verified_principal_and_ignores_a_token() -> Result {
    let lab = Material::open()?.serve().await?;
    let node = Principal::Node {
        node: "node-a".to_owned(),
    };
    let other = Principal::Agent {
        node: "node-a".to_owned(),
        agent: "worker".to_owned(),
    };
    let Issued {
        leaf,
        identity,
        next_csr,
        intermediate,
        next_key,
    } = enroll(&lab, &node).await?;
    let renewed = lab.post_sign(&next_csr, None, Some(identity)).await?;
    assert_eq!(renewed.status, reqwest::StatusCode::OK);
    let body: SignResponse = serde_json::from_str(&renewed.body)?;
    assert_eq!(uri_san(&body.leaf)?, node.spiffe_uri()?);
    assert_ne!(serial(&body.leaf)?, serial(&leaf)?);
    let (foreign_csr, _) = csr(&other.spiffe_uri()?)?;
    let refused = lab
        .post_sign(
            &foreign_csr,
            None,
            Some(identity_pem(&body.leaf, &intermediate, &next_key)?),
        )
        .await?;
    assert_eq!(refused.status, reqwest::StatusCode::FORBIDDEN);
    let spared = lab.pki.issue_token(other.clone()).await?;
    let (same_csr, _) = csr(&node.spiffe_uri()?)?;
    let with_token = lab
        .post_sign(
            &same_csr,
            Some(&spared.0),
            Some(identity_pem(&body.leaf, &intermediate, &next_key)?),
        )
        .await?;
    assert_eq!(with_token.status, reqwest::StatusCode::OK);
    let (other_csr, _) = csr(&other.spiffe_uri()?)?;
    let still_valid = lab.post_sign(&other_csr, Some(&spared.0), None).await?;
    assert_eq!(still_valid.status, reqwest::StatusCode::OK);
    let other_body: SignResponse = serde_json::from_str(&still_valid.body)?;
    assert_eq!(uri_san(&other_body.leaf)?, other.spiffe_uri()?);
    lab.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_and_untrusted_certificates_cannot_renew() -> Result {
    let material = Material::open()?;
    let legacy_plain = material.legacy_identity()?;
    let legacy_with_token = material.legacy_identity()?;
    let lab = material.serve().await?;
    let node = Principal::Node {
        node: "node-a".to_owned(),
    };
    let (node_csr, _) = csr(&node.spiffe_uri()?)?;
    let refused = lab.post_sign(&node_csr, None, Some(legacy_plain)).await?;
    assert_eq!(refused.status, reqwest::StatusCode::UNAUTHORIZED);
    let token = lab.pki.issue_token(node.clone()).await?;
    let signed = lab
        .post_sign(&node_csr, Some(&token.0), Some(legacy_with_token))
        .await?;
    assert_eq!(signed.status, reqwest::StatusCode::OK);
    let body: SignResponse = serde_json::from_str(&signed.body)?;
    assert_eq!(uri_san(&body.leaf)?, node.spiffe_uri()?);
    let foreign = foreign_identity()?;
    let untrusted = lab
        .post_sign(&node_csr, Some(&token.0), Some(foreign))
        .await;
    assert!(untrusted.is_err());
    lab.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_principal_fails_tls_and_a_token_can_still_sign() -> Result {
    let mut material = Material::open()?;
    let revoked = Principal::Device {
        user: "alice".to_owned(),
        device: "laptop".to_owned(),
    };
    let uri = revoked.spiffe_uri()?;
    let (csr_pem, key) = csr(&uri)?;
    let token = material.pki.issue_token(revoked.clone()).await?;
    let signed = material
        .pki
        .sign(
            CsrPem(csr_pem),
            SignAuthority::Token(EnrollmentToken(token.0)),
        )
        .await?;
    material.pki.revoke(revoked).await?;
    material.enable_crl()?;
    let identity = identity_pem(&signed.leaf, &material.intermediate, &key)?;
    let leaf_serial = serial(&signed.leaf)?;
    let (renew_csr, _) = csr(&uri)?;
    let lab = material.serve().await?;
    let failed = lab.post_sign(&renew_csr, None, Some(identity)).await;
    assert!(failed.is_err());
    let crl = lab.get("/ca/v1/crl", None).await?;
    assert_eq!(crl.status, reqwest::StatusCode::OK);
    assert!(crl_serials(&crl.body)?.contains(&leaf_serial));
    let fresh = Principal::Node {
        node: "node-b".to_owned(),
    };
    let enrolled = enroll(&lab, &fresh).await?;
    assert_eq!(uri_san(&enrolled.leaf)?, fresh.spiffe_uri()?);
    lab.stop().await
}

struct Issued {
    leaf: String,
    identity: reqwest::Identity,
    next_csr: String,
    intermediate: String,
    next_key: String,
}

async fn enroll(lab: &Lab, principal: &Principal) -> Result<Issued> {
    let uri = principal.spiffe_uri()?;
    let (first_csr, first_key) = csr(&uri)?;
    let (next_csr, next_key) = csr(&uri)?;
    let token = lab.pki.issue_token(principal.clone()).await?;
    let reply = lab.post_sign(&first_csr, Some(&token.0), None).await?;
    assert_eq!(reply.status, reqwest::StatusCode::OK);
    let signed: SignResponse = serde_json::from_str(&reply.body)?;
    let identity = identity_pem(&signed.leaf, &lab.intermediate, &first_key)?;
    Ok(Issued {
        leaf: signed.leaf,
        identity,
        next_csr,
        intermediate: lab.intermediate.clone(),
        next_key,
    })
}

fn foreign_identity() -> Result<reqwest::Identity> {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let params = rcgen::CertificateParams::new(vec!["foreign.example.test".to_owned()])?;
    let certificate = params.self_signed(&key)?;
    let pem = format!("{}{}", certificate.pem(), key.serialize_pem());
    Ok(reqwest::Identity::from_pem(pem.as_bytes())?)
}
