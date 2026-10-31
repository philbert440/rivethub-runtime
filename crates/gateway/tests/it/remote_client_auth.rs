use crate::common;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use axum::http::StatusCode;
use gateway::Gateway;
use gateway::tenancy::PeerCertificate;
use gateway::tenancy::Tenancy;
use gateway::tenancy::UserId;
use kernel::memory::MemoryStore;
use testkit::FakeMemoryStore;
use tower::ServiceExt;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const PROTECTED: [&str; 7] = [
    "/sessions",
    "/api/memory/search?q=x",
    "/files/list",
    "/api/updates/manifest",
    "/api/updates/RivetHub-1.2.3.apk",
    "/api/agents",
    "/mesh.json",
];

const OPEN: [&str; 3] = ["/healthz", "/ca/v1/chain", "/ca/v1/crl"];

fn tls_router() -> Router {
    let (gateway, _store) = common::gateway();
    gateway.router(true)
}

fn tenant_router() -> Result<Router> {
    let owner = UserId::new("owner".to_owned())?;
    let tenancy = Tenancy::local(owner.clone()).with_device("phone", owner)?;
    let store: Arc<dyn MemoryStore> = Arc::new(FakeMemoryStore::default());
    let gateway = Arc::new(Gateway::new(store, common::settings()).with_tenancy(tenancy));
    Ok(gateway.router(true))
}

async fn status(
    router: Router,
    path: &str,
    remote: &str,
    peer: Option<PeerCertificate>,
) -> Result<StatusCode> {
    let mut request = Request::builder()
        .uri(path)
        .header("host", "den.example.test")
        .body(Body::empty())?;
    request
        .extensions_mut()
        .insert(ConnectInfo(remote.parse::<SocketAddr>()?));
    if let Some(peer) = peer {
        request.extensions_mut().insert(peer);
    }
    Ok(router.oneshot(request).await?.status())
}

fn device() -> PeerCertificate {
    PeerCertificate::Device {
        user: Some("owner".to_owned()),
        device: "phone".to_owned(),
    }
}

#[tokio::test]
async fn a_remote_caller_without_a_client_certificate_is_refused_on_tls() -> Result {
    for path in PROTECTED {
        let refused = status(tls_router(), path, "192.0.2.7:41000", None).await?;
        assert_eq!(refused, StatusCode::UNAUTHORIZED, "{path} with tenancy off");
        let tenant = status(tenant_router()?, path, "192.0.2.7:41000", None).await?;
        assert_eq!(tenant, StatusCode::UNAUTHORIZED, "{path} with tenancy on");
    }
    Ok(())
}

#[tokio::test]
async fn health_and_the_public_ca_routes_need_no_client_certificate() -> Result {
    for path in OPEN {
        let answered = status(tls_router(), path, "192.0.2.7:41000", None).await?;
        assert_ne!(answered, StatusCode::UNAUTHORIZED, "{path}");
    }
    Ok(())
}

#[tokio::test]
async fn a_verified_client_certificate_or_loopback_passes_the_guard() -> Result {
    for path in PROTECTED {
        let certified = status(tenant_router()?, path, "192.0.2.7:41000", Some(device())).await?;
        assert_ne!(
            certified,
            StatusCode::UNAUTHORIZED,
            "{path} with a certificate"
        );
        let local = status(tenant_router()?, path, "127.0.0.1:41000", None).await?;
        assert_ne!(local, StatusCode::UNAUTHORIZED, "{path} from loopback");
    }
    Ok(())
}
