use super::Result;
use axum::{
    Json, Router,
    extract::{Extension, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use kernel::ca::{CertificateAuthority, CsrPem, EnrollmentToken, Principal, SignAuthority};
use pki::Pki;
use rcgen::{CertificateParams, Issuer, KeyPair};
use std::path::Path;
use std::sync::Arc;
use tokio::{net::TcpListener, sync::mpsc, task::JoinSet};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use wire::ca::SignRequest;
use x509_parser::prelude::FromDer;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Behavior {
    Sign,
    Redirect,
    ForeignRedirect,
    WrongKey,
    WrongPrincipal,
    Refuse,
    Wait,
}

#[derive(Clone)]
struct Service {
    authority: Pki,
    events: mpsc::Sender<bool>,
    behavior: Behavior,
}

pub(crate) struct Server {
    pub(crate) url: url::Url,
    pub(crate) events: mpsc::Receiver<bool>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result>,
}

impl Server {
    pub(crate) async fn start(authority: Pki, state: &Path, behavior: Behavior) -> Result<Self> {
        let acceptor = acceptor(state)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("https://{}/", listener.local_addr()?).parse()?;
        let (events, receiver) = mpsc::channel(16);
        let service = Service {
            authority,
            events,
            behavior,
        };
        let cancel = CancellationToken::new();
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => break,
                    result = connections.join_next(), if !connections.is_empty() => { result.ok_or("missing connection")???; }
                    accepted = listener.accept() => {
                        let (stream, _) = accepted?;
                        let acceptor = acceptor.clone();
                        let service = service.clone();
                        let stop = shutdown.clone();
                        connections.spawn(async move {
                            tokio::select! {
                                () = stop.cancelled() => Ok::<_, Box<dyn std::error::Error + Send + Sync>>(()),
                                result = async {
                                    let tls = acceptor.accept(stream).await?;
                                    let principal = match tls.get_ref().1.peer_certificates().and_then(|certs| certs.first()) {
                                        Some(der) => {
                                            let (_, certificate) = x509_parser::certificate::X509Certificate::from_der(der)?;
                                            let san = certificate.subject_alternative_name()?.ok_or("missing peer SAN")?;
                                            let [x509_parser::extensions::GeneralName::URI(uri)] = san.value.general_names.as_slice() else { return Err("wrong peer SAN".into()); };
                                            Some(uri.parse::<Principal>()?)
                                        }
                                        None => None,
                                    };
                                    let waiting = matches!(service.behavior, Behavior::Wait);
                                    let router = Router::new().route("/ca/v1/sign", post(sign)).route("/signed", post(issue))
                                        .layer(Extension(principal)).with_state(service);
                                    let result = hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(tls), hyper_util::service::TowerToHyperService::new(router)).await;
                                    match result {
                                        Err(error) if waiting && error.is_incomplete_message() => Ok(()),
                                        result => Ok(result?),
                                    }
                                } => result,
                            }
                        });
                    }
                }
            }
            while let Some(result) = connections.join_next().await {
                result??;
            }
            Ok(())
        });
        Ok(Self {
            url,
            events: receiver,
            cancel,
            task,
        })
    }

    pub(crate) async fn stop(self) -> Result {
        self.cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), self.task).await???;
        Ok(())
    }
}

async fn sign(
    state: State<Service>,
    principal: Extension<Option<Principal>>,
    headers: HeaderMap,
    request: Json<SignRequest>,
) -> Response {
    match state.behavior {
        Behavior::Redirect => {
            (StatusCode::TEMPORARY_REDIRECT, [("location", "/signed")]).into_response()
        }
        Behavior::ForeignRedirect => (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "https://127.0.0.1:1/stolen")],
        )
            .into_response(),
        Behavior::Sign
        | Behavior::WrongKey
        | Behavior::WrongPrincipal
        | Behavior::Refuse
        | Behavior::Wait => issue(state, principal, headers, request).await,
    }
}

async fn issue(
    State(state): State<Service>,
    Extension(principal): Extension<Option<Principal>>,
    headers: HeaderMap,
    Json(mut request): Json<SignRequest>,
) -> Response {
    let result = async {
        state.events.send(principal.is_some()).await?;
        if matches!(state.behavior, Behavior::Wait) {
            return std::future::pending().await;
        }
        if matches!(state.behavior, Behavior::Refuse) {
            return Err("refused".into());
        }
        let authority = if let Some(principal) = principal {
            SignAuthority::Renewal(principal)
        } else {
            let token = headers
                .get("authorization")
                .ok_or("missing token")?
                .to_str()?
                .strip_prefix("Bearer ")
                .ok_or("wrong authorization")?;
            SignAuthority::Token(EnrollmentToken(token.to_owned()))
        };
        if matches!(
            state.behavior,
            Behavior::WrongKey | Behavior::WrongPrincipal
        ) {
            let key = KeyPair::generate()?;
            let mut params = CertificateParams::default();
            let uri = if matches!(state.behavior, Behavior::WrongPrincipal) {
                "spiffe://rivet.mesh/node/other"
            } else {
                "spiffe://rivet.mesh/node/node-a"
            };
            params.subject_alt_names = vec![rcgen::SanType::URI(uri.try_into()?)];
            request.csr = params.serialize_request(&key)?.pem()?;
            if matches!(state.behavior, Behavior::WrongPrincipal) {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                    state
                        .authority
                        .sign(
                            CsrPem(request.csr),
                            SignAuthority::Token(state.authority.issue_token(uri.parse()?).await?),
                        )
                        .await?,
                );
            }
        }
        Ok(state.authority.sign(CsrPem(request.csr), authority).await?)
    }
    .await;
    match result {
        Ok(response) => Json(response).into_response(),
        Err(_) => StatusCode::FORBIDDEN.into_response(),
    }
}

fn acceptor(state: &Path) -> Result<TlsAcceptor> {
    let root = pki::certificate(&std::fs::read_to_string(state.join("root.cert.pem"))?)?;
    let intermediate = std::fs::read_to_string(state.join("intermediate.cert.pem"))?;
    let issuer = Issuer::from_ca_cert_pem(
        &intermediate,
        KeyPair::from_pem(&std::fs::read_to_string(
            state.join("intermediate.key.pem"),
        )?)?,
    )?;
    let key = KeyPair::generate()?;
    let params = CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])?;
    let cert = params.signed_by(&key, &issuer)?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.into())?;
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .allow_unauthenticated()
    .build()?;
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![cert.der().clone(), pki::certificate(&intermediate)?.into()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}
