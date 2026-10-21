use crate::common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use kernel::ca::CaError;
use kernel::ca::CertificateAuthority;
use kernel::ca::CrlPem;
use kernel::ca::CsrPem;
use kernel::ca::EnrollmentToken;
use kernel::ca::Principal;
use kernel::ca::SignAuthority;
use parking_lot::Mutex;
use serde::Deserialize;
use tower::ServiceExt;
use wire::gateway::GatewayError;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Deserialize)]
struct Skipped {
    name: String,
    reason: String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sign_rate_limit_refuses_without_calling_the_authority_again() -> Result {
    let fake = Arc::new(CountingCa::default());
    let authority: Arc<dyn CertificateAuthority> = fake.clone();
    let store: Arc<dyn kernel::memory::MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let gateway = Arc::new(
        gateway::Gateway::new(store, common::settings()).with_certificate_authority(authority),
    );
    let app = gateway::routes::ca::router().with_state(gateway);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..65 {
        let app = app.clone();
        let request = Request::builder()
            .method("POST")
            .uri("/ca/v1/sign")
            .header("content-type", "application/json")
            .header("authorization", "Bearer token")
            .body(Body::from(r#"{"csr":"request"}"#))?;
        tasks.spawn(async move { app.oneshot(request).await });
    }
    let mut admitted = 0_u64;
    let mut limited = 0_u64;
    while let Some(joined) = tasks.join_next().await {
        let response = joined??;
        match response.status() {
            reqwest::StatusCode::OK => admitted += 1,
            reqwest::StatusCode::TOO_MANY_REQUESTS => {
                let body = axum::body::to_bytes(response.into_body(), 4096).await?;
                let error: GatewayError = serde_json::from_slice(&body)?;
                assert_eq!(error.error, "too many certificate requests");
                limited += 1;
            }
            status => return Err(format!("unexpected sign status {status}").into()),
        }
    }
    assert_eq!((admitted, limited, fake.signs()), (60, 5, 60));
    let chain = app
        .oneshot(Request::builder().uri("/ca/v1/chain").body(Body::empty())?)
        .await?;
    assert_eq!(chain.status(), reqwest::StatusCode::OK);
    let absent = gateway::routes::ca::router().with_state(Arc::new(gateway::Gateway::new(
        Arc::new(testkit::FakeMemoryStore::default()),
        common::settings(),
    )));
    let unavailable = absent
        .oneshot(Request::builder().uri("/ca/v1/crl").body(Body::empty())?)
        .await?;
    assert_eq!(
        unavailable.status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
    Ok(())
}

#[test]
fn out_of_slice_scenarios_are_named_and_explained() -> Result {
    let entries: Vec<Skipped> = serde_json::from_str(include_str!("../fixtures/ca/skipped.json"))?;
    let expected = [
        "crl_reload_without_listener_restart",
        "mesh_channel_optional_client_certificates",
    ];
    assert_eq!(entries.len(), expected.len());
    for (entry, name) in entries.iter().zip(expected) {
        assert_eq!(entry.name, name);
        assert!(entry.reason.len() >= 60);
    }
    Ok(())
}

#[derive(Default)]
struct CountingCa {
    signs: Mutex<u64>,
}

impl CountingCa {
    fn signs(&self) -> u64 {
        *self.signs.lock()
    }
}

#[async_trait]
impl CertificateAuthority for CountingCa {
    async fn issue_token(
        &self,
        _principal: Principal,
    ) -> std::result::Result<EnrollmentToken, CaError> {
        Ok(EnrollmentToken("token".to_owned()))
    }

    async fn sign(
        &self,
        _csr: CsrPem,
        _authority: SignAuthority,
    ) -> std::result::Result<wire::ca::SignResponse, CaError> {
        *self.signs.lock() += 1;
        Ok(wire::ca::SignResponse {
            leaf: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
            chain: vec![
                "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
            ],
        })
    }

    async fn chain(&self) -> std::result::Result<Vec<String>, CaError> {
        Ok(vec![
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".to_owned(),
        ])
    }

    async fn crl(&self) -> std::result::Result<CrlPem, CaError> {
        Ok(CrlPem(
            "-----BEGIN X509 CRL-----\nMIIB\n-----END X509 CRL-----\n".to_owned(),
        ))
    }

    async fn revoke(&self, _principal: Principal) -> std::result::Result<(), CaError> {
        Ok(())
    }
}
