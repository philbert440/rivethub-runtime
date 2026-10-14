#[path = "../common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use gateway::Gateway;
use gateway::routes::devices::DeviceAccount;
use gateway::routes::devices::DeviceRuntime;
use gateway::routes::devices::Enrollment;
use kernel::ca::CaError;
use kernel::ca::CertChainPem;
use kernel::ca::CertificateAuthority;
use kernel::ca::CrlPem;
use kernel::ca::CsrPem;
use kernel::ca::EnrollmentToken;
use kernel::ca::Principal;
use kernel::ca::SignAuthority;
use kernel::memory::MemoryStore;
use testkit::Request;
use testkit::Response;
use wire::gateway::GatewayError;
use wire::gateway::devices::DeviceEnrollConfig;
use wire::gateway::devices::DeviceEnrollRequest;

pub const WG_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
pub const WG_KEY_TWO: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
pub const POOL: &str = "192.0.2.0/29";
pub const GATEWAY_URL: &str = "https://hub.example.test";
pub const OWNER: &str = "owner";
pub const CSR: &str =
    "-----BEGIN CERTIFICATE REQUEST-----\nMIIB\n-----END CERTIFICATE REQUEST-----\n";
pub const CHAIN_INTERMEDIATE: &str = "intermediate";
pub const CHAIN_ROOT: &str = "root";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeMode {
    Ready,
    Unavailable,
    RejectsCsr,
    SplitChain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSnapshot {
    pub principals: Vec<Principal>,
    pub signed_tokens: Vec<String>,
    pub csrs: Vec<String>,
    pub revoked: Vec<Principal>,
}

#[derive(Debug)]
struct FakeState {
    next: u64,
    principals: Vec<Principal>,
    signed_tokens: Vec<String>,
    csrs: Vec<String>,
    revoked: Vec<Principal>,
    sabotage: Option<PathBuf>,
}

#[derive(Debug)]
pub struct FakeCa {
    mode: FakeMode,
    token_override: Option<String>,
    state: tokio::sync::Mutex<FakeState>,
}

impl FakeCa {
    pub fn new(mode: FakeMode) -> Self {
        Self {
            mode,
            token_override: None,
            state: tokio::sync::Mutex::new(FakeState {
                next: 0,
                principals: Vec::new(),
                signed_tokens: Vec::new(),
                csrs: Vec::new(),
                revoked: Vec::new(),
                sabotage: None,
            }),
        }
    }

    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token_override = Some(token.into());
        self
    }

    pub async fn sabotage(&self, path: PathBuf) {
        self.state.lock().await.sabotage = Some(path);
    }

    pub async fn snapshot(&self) -> FakeSnapshot {
        let state = self.state.lock().await;
        FakeSnapshot {
            principals: state.principals.clone(),
            signed_tokens: state.signed_tokens.clone(),
            csrs: state.csrs.clone(),
            revoked: state.revoked.clone(),
        }
    }

    fn down() -> CaError {
        CaError::Unavailable(Box::new(std::io::Error::other("ca down")))
    }
}

#[async_trait]
impl CertificateAuthority for FakeCa {
    async fn issue_token(&self, principal: Principal) -> Result<EnrollmentToken, CaError> {
        if self.mode == FakeMode::Unavailable {
            return Err(Self::down());
        }
        let override_token = self.token_override.clone();
        let mut state = self.state.lock().await;
        state.principals.push(principal);
        let token = if let Some(token) = override_token {
            drop(state);
            token
        } else {
            let issued = state.next;
            let Some(next) = issued.checked_add(1) else {
                drop(state);
                return Err(Self::down());
            };
            state.next = next;
            drop(state);
            format!("issued-{issued}")
        };
        Ok(EnrollmentToken(token))
    }

    async fn sign(&self, csr: CsrPem, authority: SignAuthority) -> Result<CertChainPem, CaError> {
        if self.mode == FakeMode::Unavailable {
            return Err(Self::down());
        }
        let token = match authority {
            SignAuthority::Token(token) => token.0,
            SignAuthority::Renewal(_) => {
                return Err(CaError::Refused(
                    "renewal is not device enrollment".to_owned(),
                ));
            }
        };
        let sabotage = {
            let mut state = self.state.lock().await;
            state.csrs.push(csr.0.clone());
            state.signed_tokens.push(token);
            state.sabotage.clone()
        };
        if self.mode == FakeMode::RejectsCsr {
            return Err(CaError::InvalidCsr("test csr rejected".to_owned()));
        }
        if let Some(path) = sabotage {
            std::fs::remove_file(path).map_err(|error| CaError::Unavailable(Box::new(error)))?;
        }
        let chain = if self.mode == FakeMode::SplitChain {
            vec!["other-chain".to_owned()]
        } else {
            published_chain()
        };
        Ok(CertChainPem {
            leaf: format!("leaf:{}", csr.0),
            chain,
        })
    }

    async fn chain(&self) -> Result<Vec<String>, CaError> {
        if self.mode == FakeMode::Unavailable {
            Err(Self::down())
        } else {
            Ok(published_chain())
        }
    }

    async fn crl(&self) -> Result<CrlPem, CaError> {
        if self.mode == FakeMode::Unavailable {
            Err(Self::down())
        } else {
            Ok(CrlPem("crl".to_owned()))
        }
    }

    async fn revoke(&self, principal: Principal) -> Result<(), CaError> {
        if self.mode == FakeMode::Unavailable {
            return Err(Self::down());
        }
        self.state.lock().await.revoked.push(principal);
        Ok(())
    }
}

pub fn published_chain() -> Vec<String> {
    vec![CHAIN_INTERMEDIATE.to_owned(), CHAIN_ROOT.to_owned()]
}

pub fn account() -> DeviceAccount {
    DeviceAccount {
        pg_role: "rivet_device".to_owned(),
        owner: OWNER.to_owned(),
    }
}

pub fn config() -> DeviceEnrollConfig {
    DeviceEnrollConfig {
        shared_host: "files.example.test".to_owned(),
        shared_export: "/srv/share".to_owned(),
        pg_url: "postgres://device@db/rivet".to_owned(),
        embed_url: "http://127.0.0.1:8080".to_owned(),
        wg_endpoint: "wg.example.test:51820".to_owned(),
        wg_peer_public_key: WG_KEY.to_owned(),
        wg_address: "192.0.2.1/32".to_owned(),
        wg_allowed_ips: "192.0.2.0/24".to_owned(),
        home_subnet: "198.51.100.0/24".to_owned(),
    }
}

pub fn enrollment(roster: PathBuf, pool: Option<&str>, relay: Option<&str>) -> Enrollment {
    Enrollment::try_new(
        roster,
        pool,
        GATEWAY_URL.to_owned(),
        config(),
        relay.map(str::to_owned),
        account(),
    )
    .unwrap_or_else(|error| panic!("enrollment setup: {error}"))
}

pub fn router(runtime: DeviceRuntime) -> axum::Router {
    router_with(runtime, Arc::new(FakeCa::new(FakeMode::Ready))).0
}

pub fn router_with(runtime: DeviceRuntime, fake: Arc<FakeCa>) -> (axum::Router, Arc<FakeCa>) {
    let authority: Arc<dyn CertificateAuthority> = fake.clone();
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let gateway = Gateway::new(store, common::settings())
        .with_devices(runtime)
        .with_certificate_authority(authority);
    (Arc::new(gateway).router(false), fake)
}

pub fn router_without_authority(runtime: DeviceRuntime) -> axum::Router {
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let gateway = Gateway::new(store, common::settings()).with_devices(runtime);
    Arc::new(gateway).router(false)
}

pub fn request(method: &str, target: &str, body: &str, via: Option<&str>) -> Request {
    let mut headers = vec![("Host".to_owned(), "127.0.0.1:5174".to_owned())];
    if !body.is_empty() {
        headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    }
    Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body: body.as_bytes().to_vec(),
        recipe: None,
        via: via.map(str::to_owned),
    }
}

pub async fn call(app: &axum::Router, method: &str, target: &str, body: &str) -> Response {
    call_via(app, method, target, body, None).await
}

pub async fn call_via(
    app: &axum::Router,
    method: &str,
    target: &str,
    body: &str,
    via: Option<&str>,
) -> Response {
    common::answer(app.clone(), request(method, target, body, via))
        .await
        .unwrap_or_else(|error| panic!("{method} {target}: {error}"))
}

pub fn body<T: serde::de::DeserializeOwned>(response: &Response) -> T {
    serde_json::from_slice(&response.body)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&response.body)))
}

pub fn create_body(name: Option<&str>) -> String {
    serde_json::to_string(&wire::gateway::devices::DeviceCreateRequest {
        name: name.map(str::to_owned),
    })
    .expect("create body")
}

pub fn enroll_body(token: &str, public_key: &str, name: Option<&str>) -> String {
    enroll_csr(token, public_key, name, Some(CSR))
}

pub fn enroll_csr(token: &str, public_key: &str, name: Option<&str>, csr: Option<&str>) -> String {
    serde_json::to_string(&DeviceEnrollRequest {
        token: Some(token.to_owned()),
        public_key: Some(public_key.to_owned()),
        name: name.map(str::to_owned),
        csr: csr.map(str::to_owned),
    })
    .expect("enroll body")
}

pub fn pair_body(token: &str) -> String {
    pair_csr(token, Some(CSR))
}

pub fn pair_csr(token: &str, csr: Option<&str>) -> String {
    serde_json::to_string(&wire::gateway::PairRedeemRequest {
        token: token.to_owned(),
        csr: csr.map(str::to_owned),
    })
    .expect("pair body")
}

pub fn expect_error(response: &Response, status: u16, message: &str) {
    assert_eq!(
        response.status,
        status,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let parsed: GatewayError = body(response);
    assert_eq!(parsed, GatewayError::new(message));
}
