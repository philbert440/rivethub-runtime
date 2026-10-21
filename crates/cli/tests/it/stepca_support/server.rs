use super::Result;
use axum::Router;
use axum::extract::Extension;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::DistinguishedName;
use rcgen::DnType;
use rcgen::ExtendedKeyUsagePurpose;
use rcgen::IsCa;
use rcgen::Issuer;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use rcgen::SanType;
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Behavior {
    Sign,
    Redirect,
    ForeignRedirect,
    WrongKey,
    WrongPrincipal,
    ExtraName,
    RevokedRenew,
    Wait,
}

#[derive(Debug)]
pub(crate) struct Exchange {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) client_certificate: bool,
    pub(crate) empty_body: bool,
    pub(crate) csr: Option<String>,
}

struct Shared {
    behavior: parking_lot::Mutex<Behavior>,
    origin: String,
    events: mpsc::Sender<Exchange>,
    material: parking_lot::Mutex<Material>,
    connection: parking_lot::Mutex<Option<String>>,
}

struct Material {
    intermediate_pem: String,
    intermediate_key_pem: String,
    intermediate_params: CertificateParams,
    principal: String,
    tls: stepca::TlsOptions,
    reused: FailureDocument,
    revoked: FailureDocument,
    saved_csr: Option<String>,
    signed: bool,
    redirected: bool,
    serial: u64,
}

struct FailureDocument {
    status: u16,
    body: Vec<u8>,
}

struct Prepared {
    root_pem: String,
    acceptor: TlsAcceptor,
    material: Material,
}

pub(crate) struct Server {
    pub(crate) url: url::Url,
    pub(crate) root_pem: String,
    pub(crate) events: mpsc::Receiver<Exchange>,
    shared: Arc<Shared>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result>>,
}

impl Server {
    pub(crate) async fn start(behavior: Behavior, principal: &str) -> Result<Self> {
        let prepared = prepare(principal)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let origin = format!("https://{address}");
        let url = format!("{origin}/").parse()?;
        let (events, receiver) = mpsc::channel(16);
        let shared = Arc::new(Shared {
            behavior: parking_lot::Mutex::new(behavior),
            origin,
            events,
            material: parking_lot::Mutex::new(prepared.material),
            connection: parking_lot::Mutex::new(None),
        });
        let cancel = CancellationToken::new();
        let task = spawn_accept(
            listener,
            prepared.acceptor,
            Arc::clone(&shared),
            cancel.clone(),
        );
        Ok(Self {
            url,
            root_pem: prepared.root_pem,
            events: receiver,
            shared,
            cancel,
            task: Some(task),
        })
    }

    pub(crate) fn set_behavior(&self, behavior: Behavior) {
        *self.shared.behavior.lock() = behavior;
    }

    pub(crate) fn note(&self) -> String {
        self.shared.connection.lock().clone().unwrap_or_default()
    }

    pub(crate) async fn stop(mut self) -> Result {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task).await???;
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn prepare(principal: &str) -> Result<Prepared> {
    let root_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut root_params = CertificateParams::new(vec!["stepca.example.test".to_owned()])?;
    ca_window(&mut root_params);
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let mut root_name = DistinguishedName::new();
    root_name.push(DnType::CommonName, "stepca test root");
    root_params.distinguished_name = root_name;
    let root = root_params.self_signed(&root_key)?;
    let intermediate_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut intermediate_params = CertificateParams::default();
    ca_window(&mut intermediate_params);
    intermediate_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    intermediate_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let mut intermediate_name = DistinguishedName::new();
    intermediate_name.push(DnType::CommonName, "stepca test intermediate");
    intermediate_params.distinguished_name = intermediate_name;
    let intermediate = intermediate_params.signed_by(
        &intermediate_key,
        &Issuer::from_params(&root_params, &root_key),
    )?;
    let server_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut server_params = CertificateParams::new(vec!["127.0.0.1".to_owned()])?;
    ca_window(&mut server_params);
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let server_certificate =
        server_params.signed_by(&server_key, &Issuer::from_params(&root_params, &root_key))?;
    let acceptor = acceptor(&root, &server_certificate, &server_key)?;
    let recorded = super::recording()?;
    let tls = recorded
        .case("sign-with-token")?
        .tls
        .clone()
        .ok_or("recorded sign response has no tls options")?;
    Ok(Prepared {
        root_pem: root.pem(),
        acceptor,
        material: Material {
            intermediate_pem: intermediate.pem(),
            intermediate_key_pem: intermediate_key.serialize_pem(),
            intermediate_params,
            principal: principal.to_owned(),
            tls,
            reused: failure_document(&recorded, "sign-token-reused")?,
            revoked: failure_document(&recorded, "renew-revoked")?,
            saved_csr: None,
            signed: false,
            redirected: false,
            serial: 0,
        },
    })
}

fn failure_document(recorded: &super::Recording, id: &str) -> Result<FailureDocument> {
    let case = recorded.case(id)?;
    let message = case
        .message
        .clone()
        .ok_or_else(|| format!("recorded case {id} has no message"))?;
    let body = serde_json::to_vec(&FailureBody {
        status: case.status,
        message,
    })?;
    Ok(FailureDocument {
        status: case.status,
        body,
    })
}

fn ca_window(params: &mut CertificateParams) {
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2027, 1, 1);
}

fn acceptor(
    root: &rcgen::Certificate,
    server_certificate: &rcgen::Certificate,
    server_key: &KeyPair,
) -> Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.der().clone())?;
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::clone(&provider),
    )
    .allow_unauthenticated()
    .build()?;
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server_certificate.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn spawn_accept(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<Result> {
    tokio::spawn(async move {
        let mut connections: JoinSet<Result> = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let acceptor = acceptor.clone();
                    let shared = Arc::clone(&shared);
                    let stop = cancel.clone();
                    connections.spawn(async move {
                        tokio::select! {
                            biased;
                            () = stop.cancelled() => Ok(()),
                            result = serve_one(acceptor, stream, Arc::clone(&shared)) => {
                                if let Err(error) = result {
                                    *shared.connection.lock() = Some(error.to_string());
                                }
                                Ok(())
                            }
                        }
                    });
                }
                joined = connections.join_next(), if !connections.is_empty() => {
                    joined.ok_or("missing connection")???;
                }
            }
        }
        while let Some(joined) = connections.join_next().await {
            joined??;
        }
        Ok(())
    })
}

async fn serve_one(
    acceptor: TlsAcceptor,
    stream: tokio::net::TcpStream,
    shared: Arc<Shared>,
) -> Result {
    let tls = acceptor.accept(stream).await?;
    let peer = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| cert.as_ref().to_vec());
    let router = Router::new()
        .fallback(handle)
        .layer(Extension(peer))
        .with_state(shared);
    hyper::server::conn::http1::Builder::new()
        .serve_connection(
            hyper_util::rt::TokioIo::new(tls),
            hyper_util::service::TowerToHyperService::new(router),
        )
        .await?;
    Ok(())
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    Extension(peer): Extension<Option<Vec<u8>>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(error) => return text(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let behavior = *shared.behavior.lock();
    let outcome = {
        let mut material = shared.material.lock();
        decide(
            behavior,
            &shared.origin,
            &mut material,
            &parts,
            &bytes,
            peer.as_deref(),
        )
    };
    if shared.events.send(outcome.exchange).await.is_err() {
        return text(
            StatusCode::SERVICE_UNAVAILABLE,
            "test listener dropped".to_owned(),
        );
    }
    if outcome.wait {
        std::future::pending::<()>().await;
    }
    outcome.response
}

struct Outcome {
    exchange: Exchange,
    wait: bool,
    response: Response,
}

fn decide(
    behavior: Behavior,
    origin: &str,
    material: &mut Material,
    parts: &axum::http::request::Parts,
    bytes: &[u8],
    peer: Option<&[u8]>,
) -> Outcome {
    let path = parts.uri.path().to_owned();
    let client_certificate = peer.is_some();
    let empty_body = bytes.is_empty();
    let form = if empty_body {
        None
    } else {
        match serde_json::from_slice::<SignForm>(bytes) {
            Ok(form) => Some(form),
            Err(error) => {
                return Outcome {
                    exchange: Exchange {
                        method: parts.method.to_string(),
                        path,
                        client_certificate,
                        empty_body,
                        csr: None,
                    },
                    wait: false,
                    response: text(StatusCode::BAD_REQUEST, error.to_string()),
                };
            }
        }
    };
    let exchange = Exchange {
        method: parts.method.to_string(),
        path: path.clone(),
        client_certificate,
        empty_body,
        csr: form.as_ref().map(|form| form.csr.clone()),
    };
    let response = route(behavior, origin, material, &path, form.as_ref());
    Outcome {
        exchange,
        wait: behavior == Behavior::Wait,
        response,
    }
}

fn route(
    behavior: Behavior,
    origin: &str,
    material: &mut Material,
    path: &str,
    form: Option<&SignForm>,
) -> Response {
    if behavior == Behavior::Wait {
        return text(StatusCode::OK, "waiting".to_owned());
    }
    if behavior == Behavior::Redirect && path == "/1.0/sign" && !material.redirected {
        material.redirected = true;
        return redirect(format!("{origin}/1.0/issue"));
    }
    if behavior == Behavior::ForeignRedirect && path == "/1.0/sign" {
        return redirect("https://127.0.0.1:1/1.0/sign".to_owned());
    }
    if path == "/1.0/sign" || path == "/1.0/issue" {
        return sign(behavior, material, form);
    }
    if path == "/1.0/renew" {
        return renew(behavior, material);
    }
    text(StatusCode::NOT_FOUND, format!("unexpected path {path}"))
}

fn sign(behavior: Behavior, material: &mut Material, form: Option<&SignForm>) -> Response {
    if material.signed {
        return failure(&material.reused);
    }
    let Some(form) = form else {
        return text(
            StatusCode::BAD_REQUEST,
            "missing certificate request".to_owned(),
        );
    };
    match issue(material, Some(&form.csr), mint(behavior)) {
        Ok(body) => {
            material.saved_csr = Some(form.csr.clone());
            material.signed = true;
            created(body)
        }
        Err(error) => text(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

fn renew(behavior: Behavior, material: &mut Material) -> Response {
    if behavior == Behavior::RevokedRenew {
        return failure(&material.revoked);
    }
    let saved = material.saved_csr.clone();
    match issue(material, saved.as_deref(), mint(behavior)) {
        Ok(body) => created(body),
        Err(error) => text(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

fn mint(behavior: Behavior) -> Mint {
    match behavior {
        Behavior::WrongKey => Mint::WrongKey,
        Behavior::WrongPrincipal => Mint::WrongPrincipal,
        Behavior::ExtraName => Mint::ExtraName,
        Behavior::Sign
        | Behavior::Redirect
        | Behavior::ForeignRedirect
        | Behavior::RevokedRenew
        | Behavior::Wait => Mint::Grant,
    }
}

#[derive(Clone, Copy)]
enum Mint {
    Grant,
    WrongKey,
    WrongPrincipal,
    ExtraName,
}

fn issue(material: &mut Material, csr_pem: Option<&str>, mint: Mint) -> Result<Vec<u8>> {
    let mut params = CertificateParams::default();
    let Some(pem) = csr_pem else {
        params.subject_alt_names = vec![SanType::URI(material.principal.as_str().try_into()?)];
        apply_names(&mut params, mint, &material.principal)?;
        return sign_new_key(material, &params);
    };
    let requested = rcgen::CertificateSigningRequestParams::from_pem(pem)?;
    params
        .subject_alt_names
        .clone_from(&requested.params.subject_alt_names);
    apply_names(&mut params, mint, &material.principal)?;
    stamp(&mut params, material)?;
    let key = KeyPair::from_pem(&material.intermediate_key_pem)?;
    let issuer = Issuer::from_params(&material.intermediate_params, &key);
    let certificate = match mint {
        Mint::WrongKey => {
            let foreign = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
            params.signed_by(&foreign, &issuer)?
        }
        Mint::Grant | Mint::WrongPrincipal | Mint::ExtraName => {
            requested_with(params, &requested, &issuer)?
        }
    };
    document(material, &certificate)
}

fn requested_with(
    params: CertificateParams,
    requested: &rcgen::CertificateSigningRequestParams,
    issuer: &Issuer<'_, impl rcgen::SigningKey>,
) -> Result<rcgen::Certificate> {
    let mut signed = requested.clone();
    signed.params = params;
    Ok(signed.signed_by(issuer)?)
}

fn sign_new_key(material: &mut Material, params: &CertificateParams) -> Result<Vec<u8>> {
    let mut params = params.clone();
    stamp(&mut params, material)?;
    let foreign = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let key = KeyPair::from_pem(&material.intermediate_key_pem)?;
    let issuer = Issuer::from_params(&material.intermediate_params, &key);
    let certificate = params.signed_by(&foreign, &issuer)?;
    document(material, &certificate)
}

fn apply_names(params: &mut CertificateParams, mint: Mint, principal: &str) -> Result {
    match mint {
        Mint::Grant | Mint::WrongKey => ensure_principal(&mut params.subject_alt_names, principal),
        Mint::WrongPrincipal => {
            params
                .subject_alt_names
                .retain(|name| !matches!(name, SanType::URI(_)));
            params
                .subject_alt_names
                .push(SanType::URI("spiffe://rivet.mesh/node/other".try_into()?));
            Ok(())
        }
        Mint::ExtraName => {
            ensure_principal(&mut params.subject_alt_names, principal)?;
            params
                .subject_alt_names
                .push(SanType::DnsName("evil.example.test".try_into()?));
            Ok(())
        }
    }
}

fn ensure_principal(names: &mut Vec<SanType>, principal: &str) -> Result {
    let uri = SanType::URI(principal.try_into()?);
    if !names.iter().any(|name| name == &uri) {
        names.push(uri);
    }
    Ok(())
}

fn stamp(params: &mut CertificateParams, material: &mut Material) -> Result {
    let not_before = leaf_not_before()?;
    params.not_before = not_before;
    params.not_after = not_before + time::Duration::seconds(7 * 86_400);
    material.serial += 1;
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial_bytes(
        material.serial,
    )));
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ClientAuth,
        ExtendedKeyUsagePurpose::ServerAuth,
    ];
    params.distinguished_name = DistinguishedName::new();
    Ok(())
}

fn leaf_not_before() -> Result<time::OffsetDateTime> {
    let now = time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
    now.checked_sub(time::Duration::seconds(60))
        .ok_or_else(|| "certificate window underflow".into())
}

fn serial_bytes(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    match bytes.iter().position(|byte| *byte != 0) {
        Some(start) => bytes.iter().skip(start).copied().collect(),
        None => vec![1],
    }
}

fn document(material: &Material, certificate: &rcgen::Certificate) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&SignResponse {
        crt: certificate.pem(),
        ca: material.intermediate_pem.clone(),
        cert_chain: vec![certificate.pem(), material.intermediate_pem.clone()],
        tls_options: material.tls.clone(),
    })?)
}

fn created(body: Vec<u8>) -> Response {
    (
        StatusCode::CREATED,
        [("content-type", "application/json")],
        body,
    )
        .into_response()
}

fn failure(document: &FailureDocument) -> Response {
    (
        status_code(document.status),
        [("content-type", "application/json")],
        document.body.clone(),
    )
        .into_response()
}

fn redirect(location: String) -> Response {
    (StatusCode::TEMPORARY_REDIRECT, [("location", location)]).into_response()
}

fn text(status: StatusCode, body: String) -> Response {
    (status, body).into_response()
}

fn status_code(code: u16) -> StatusCode {
    match code {
        201 => StatusCode::CREATED,
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        _ => StatusCode::BAD_REQUEST,
    }
}

#[derive(Deserialize)]
struct SignForm {
    csr: String,
}

#[derive(Serialize)]
struct FailureBody {
    status: u16,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignResponse {
    crt: String,
    ca: String,
    cert_chain: Vec<String>,
    tls_options: stepca::TlsOptions,
}
