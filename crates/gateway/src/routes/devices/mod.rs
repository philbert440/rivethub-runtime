mod actor;
mod authority;
mod names;
mod pairing;
mod pathutil;
mod random;
mod store;

use std::sync::Arc;
use std::sync::OnceLock;

use kernel::ca::CertificateAuthority;

use axum::Json;
use axum::Router;
use axum::extract::FromRequest;
use axum::extract::Request;
use axum::extract::State;
use axum::http::Method;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing;
use chrono::TimeDelta;
use serde::de::DeserializeOwned;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use wire::gateway::GatewayError;
use wire::gateway::PairRedeemRequest;
use wire::gateway::devices::DeviceCreateRequest;
use wire::gateway::devices::DeviceEnrollRequest;
use wire::gateway::devices::PhonePairingAvailability;
use wire::gateway::devices::PhonePairingRequest;
use wire::gateway::devices::PhonePairingState;
use wire::gateway::devices::PhonePairingStatus;

use crate::Gateway;
use crate::error::ApiError;
use crate::extract::Body;

pub use pairing::PhoneIssuer;
pub use store::DeviceAccount;
pub use store::Enrollment;

use authority::SharedAuthority;

const CLI_MISSING: &str = "the rivetos CLI is not found on this node";
pub(super) const CA_UNUSABLE: &str = "device certificate authority is not usable";
pub(super) const POOL_EXHAUSTED: &str =
    "address pool exhausted (or RIVETOS_DEN_DEVICES_POOL unset)";
pub(super) const ENROLL_TOKEN: &str = "invalid or expired enrollment token";
pub(super) const ENROLL_FIELDS: &str = "token and a valid WireGuard publicKey are required";
pub(super) const ALREADY_ENROLLED: &str = "this device is already enrolled";
pub(super) const PAIR_CODE: &str = "invalid or expired pairing code";
pub(super) const CSR_REQUIRED: &str = "a certificate signing request is required";
pub(super) const CSR_REJECTED: &str = "certificate signing request was rejected";
pub(super) const CA_REFUSED: &str = "certificate authority refused the request";
const PHONE_NAME: &str = "name may only use letters, digits, \".\", \"_\" and \"-\"";
const PHONE_HOST: &str = "host must be an IP address or a host name";
pub(super) const RELAY_FAILED: &str = "relay registration failed: ssh relay is not implemented";
pub(super) const CODE_TTL: TimeDelta = TimeDelta::minutes(10);
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SetupError {
    #[error("device pool must be an IPv4 network")]
    Pool,
    #[error("device gateway url is empty")]
    GatewayUrl,
    #[error("device enrollment config is incomplete")]
    Config,
    #[error("device path must be absolute")]
    Path,
    #[error("postgres role is empty")]
    Role,
    #[error("device owner is empty")]
    Owner,
    #[error("ssh relay target is empty")]
    Relay,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum DeviceError {
    #[error("device enrollment disabled on this node")]
    Disabled,
    #[error("phone pairing is off on this node")]
    PhoneOff,
    #[error("only the owner can pair a phone")]
    NotOwner,
    #[error("{message}")]
    Unavailable {
        message: &'static str,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    #[error("{0}")]
    BadRequest(&'static str),
    #[error("malformed device name")]
    MalformedName,
    #[error("not found")]
    NotFound,
    #[error("unknown device")]
    UnknownDevice,
    #[error("POST only")]
    PostOnly,
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    BadGateway(String),
    #[error("device registry unavailable")]
    Registry,
    #[error("internal error")]
    Internal,
}

impl DeviceError {
    fn unavailable(message: &'static str) -> Self {
        Self::Unavailable {
            message,
            source: None,
        }
    }
}

impl IntoResponse for DeviceError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Disabled | Self::PhoneOff | Self::Unavailable { .. } | Self::Registry => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::NotOwner | Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::BadRequest(_) | Self::MalformedName => StatusCode::BAD_REQUEST,
            Self::NotFound | Self::UnknownDevice => StatusCode::NOT_FOUND,
            Self::PostOnly | Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::BadGateway(_) => StatusCode::BAD_GATEWAY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(GatewayError::new(self.to_string()))).into_response()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerGate {
    Admitted,
    Refused,
}

#[derive(Debug, Clone)]
enum PhoneService {
    CliMissing,
    Off,
    Issuer(PhoneIssuer),
}

enum PhoneGate {
    NotOwner,
    Off,
    CliMissing,
    Issuer,
}

#[derive(Debug)]
pub struct DeviceRuntime {
    enrollment: Option<Enrollment>,
    owner: OwnerGate,
    phone: PhoneService,
    authority: Option<SharedAuthority>,
    cancel: CancellationToken,
    slot: OnceLock<actor::ActorHandle>,
}

impl DeviceRuntime {
    #[must_use]
    pub fn unconfigured() -> Self {
        Self {
            enrollment: None,
            owner: OwnerGate::Admitted,
            phone: PhoneService::CliMissing,
            authority: None,
            cancel: CancellationToken::new(),
            slot: OnceLock::new(),
        }
    }

    #[must_use]
    pub(crate) fn with_authority(mut self, authority: Arc<dyn CertificateAuthority>) -> Self {
        self.authority = Some(SharedAuthority::new(authority));
        self
    }

    #[must_use]
    pub fn enable_enrollment(mut self, enrollment: Enrollment) -> Self {
        self.enrollment = Some(enrollment);
        self
    }

    #[must_use]
    pub fn enable_phone_issuer(mut self, issuer: PhoneIssuer) -> Self {
        self.phone = PhoneService::Issuer(issuer);
        self
    }

    #[must_use]
    pub fn phone_off(mut self) -> Self {
        self.phone = PhoneService::Off;
        self
    }

    #[must_use]
    pub fn phone_not_owner(mut self) -> Self {
        self.owner = OwnerGate::Refused;
        self
    }

    pub(crate) fn enrollment_on(&self) -> bool {
        self.enrollment.is_some()
    }

    fn phone_gate(&self) -> PhoneGate {
        if self.owner == OwnerGate::Refused {
            return PhoneGate::NotOwner;
        }
        match &self.phone {
            PhoneService::Off => PhoneGate::Off,
            PhoneService::CliMissing => PhoneGate::CliMissing,
            PhoneService::Issuer(_) => PhoneGate::Issuer,
        }
    }

    fn sender(&self) -> Result<tokio::sync::mpsc::Sender<actor::Command>, DeviceError> {
        if let Some(existing) = self.slot.get() {
            return Ok(existing.tx.clone());
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            tracing::warn!(%error, "device registry has no async runtime");
            DeviceError::Internal
        })?;
        let config = self.actor_config()?;
        let cancel = self.cancel.clone();
        let handle = self
            .slot
            .get_or_init(|| actor::spawn(&runtime, cancel, config));
        Ok(handle.tx.clone())
    }

    fn actor_config(&self) -> Result<actor::ActorConfig, DeviceError> {
        let phone = match &self.phone {
            PhoneService::Issuer(issuer) => Some(issuer.clone()),
            PhoneService::CliMissing | PhoneService::Off => None,
        };
        if self.enrollment.is_none() && phone.is_none() {
            return Err(DeviceError::Internal);
        }
        Ok(actor::ActorConfig {
            enrollment: self.enrollment.clone(),
            phone,
            authority: self.authority.clone(),
        })
    }
}

impl Drop for DeviceRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.slot.get() {
            handle.abort();
        }
    }
}

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/api/devices/pair", routing::post(pair).fallback(post_only))
        .route(
            "/api/devices/enroll",
            routing::post(enroll).fallback(devices_fallback),
        )
        .route(
            "/api/devices",
            routing::get(list).post(create).fallback(devices_fallback),
        )
        .route(
            "/api/devices/{id}",
            routing::delete(remove).fallback(devices_fallback),
        )
        .route(
            "/api/phone-pairing",
            routing::get(phone_availability)
                .post(phone_start)
                .fallback(method_not_allowed),
        )
        .route("/api/phone-pairing/{device}", routing::get(phone_status))
        .layer(axum::extract::DefaultBodyLimit::max(super::BODY_LIMIT))
}

pub(crate) fn admits_remote_without_client_auth(
    method: &Method,
    path: &str,
    enrollment_on: bool,
) -> bool {
    path == "/api/devices/pair"
        || (enrollment_on && *method == Method::POST && path == "/api/devices/enroll")
}

async fn post_only() -> DeviceError {
    DeviceError::PostOnly
}

async fn method_not_allowed() -> DeviceError {
    DeviceError::MethodNotAllowed
}

async fn devices_fallback(State(gateway): State<Arc<Gateway>>) -> DeviceError {
    if gateway.devices().enrollment_on() {
        DeviceError::MethodNotAllowed
    } else {
        DeviceError::Disabled
    }
}

async fn list(State(gateway): State<Arc<Gateway>>) -> Response {
    if !gateway.devices().enrollment_on() {
        return DeviceError::Disabled.into_response();
    }
    match ask_result(&gateway, actor::Command::List).await {
        Ok(body) => Json(body).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn create(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    if !gateway.devices().enrollment_on() {
        return DeviceError::Disabled.into_response();
    }
    let body = match read_json::<DeviceCreateRequest>(request).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    match ask_result(&gateway, |reply| actor::Command::Create {
        name: body.name,
        reply,
    })
    .await
    {
        Ok(created) => Json(created).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn enroll(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    if !gateway.devices().enrollment_on() {
        return DeviceError::Disabled.into_response();
    }
    let body = match read_json::<DeviceEnrollRequest>(request).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let Some(token) = present(body.token) else {
        return DeviceError::BadRequest(ENROLL_FIELDS).into_response();
    };
    let Some(public_key) = present(body.public_key) else {
        return DeviceError::BadRequest(ENROLL_FIELDS).into_response();
    };
    if !store::valid_wg_key(&public_key) {
        return DeviceError::BadRequest(ENROLL_FIELDS).into_response();
    }
    match ask_result(&gateway, |reply| actor::Command::Enroll {
        token,
        public_key,
        name: body.name,
        csr: present(body.csr),
        reply,
    })
    .await
    {
        Ok(enrolled) => Json(enrolled).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn remove(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    if !gateway.devices().enrollment_on() {
        return DeviceError::Disabled.into_response();
    }
    let Some(id) = device_segment(request.uri().path()) else {
        return DeviceError::NotFound.into_response();
    };
    if !pathutil::component_ok(id) {
        return DeviceError::NotFound.into_response();
    }
    match ask_result(&gateway, |reply| actor::Command::Delete {
        id: id.to_owned(),
        reply,
    })
    .await
    {
        Ok(body) => Json(body).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn pair(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    let body = match read_json::<PairRedeemRequest>(request).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    if !matches!(gateway.devices().phone, PhoneService::Issuer(_)) {
        return DeviceError::Forbidden(PAIR_CODE).into_response();
    }
    match ask_result(&gateway, |reply| actor::Command::Redeem {
        token: body.token,
        csr: present(body.csr),
        reply,
    })
    .await
    {
        Ok(redeemed) => Json(redeemed).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn phone_availability(State(gateway): State<Arc<Gateway>>) -> Response {
    match gateway.devices().phone_gate() {
        PhoneGate::NotOwner => DeviceError::NotOwner.into_response(),
        PhoneGate::Off => DeviceError::PhoneOff.into_response(),
        PhoneGate::CliMissing => Json(cli_missing_availability()).into_response(),
        PhoneGate::Issuer => match ask(&gateway, actor::Command::Availability).await {
            Ok(body) => Json(body).into_response(),
            Err(error) => error.into_response(),
        },
    }
}

async fn phone_start(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    match gateway.devices().phone_gate() {
        PhoneGate::NotOwner => return DeviceError::NotOwner.into_response(),
        PhoneGate::Off => return DeviceError::PhoneOff.into_response(),
        PhoneGate::CliMissing => return DeviceError::unavailable(CLI_MISSING).into_response(),
        PhoneGate::Issuer => {}
    }
    let body = match read_json::<PhonePairingRequest>(request).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    if !names::phone_name_ok(&body.name) {
        return DeviceError::BadRequest(PHONE_NAME).into_response();
    }
    if body
        .host
        .as_ref()
        .is_some_and(|host| !names::phone_host_ok(host))
    {
        return DeviceError::BadRequest(PHONE_HOST).into_response();
    }
    match ask_result(&gateway, |reply| actor::Command::Start {
        name: body.name,
        reply,
    })
    .await
    {
        Ok(code) => Json(code).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn phone_status(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    match gateway.devices().phone_gate() {
        PhoneGate::NotOwner => return DeviceError::NotOwner.into_response(),
        PhoneGate::Off => return DeviceError::PhoneOff.into_response(),
        PhoneGate::CliMissing | PhoneGate::Issuer => {}
    }
    let Some(raw) = phone_segment(request.uri().path()) else {
        return DeviceError::NotFound.into_response();
    };
    let name = match names::strict_percent_decode(raw) {
        Ok(name) => name,
        Err(names::PercentDecode::BadEscape) => return DeviceError::MalformedName.into_response(),
    };
    if !names::phone_name_ok(&name) {
        return DeviceError::NotFound.into_response();
    }
    if matches!(gateway.devices().phone, PhoneService::CliMissing) {
        return Json(PhonePairingStatus {
            state: PhonePairingState::Expired,
        })
        .into_response();
    }
    match ask_result(&gateway, |reply| actor::Command::State { name, reply }).await {
        Ok(status) => Json(status).into_response(),
        Err(error) => error.into_response(),
    }
}

fn cli_missing_availability() -> PhonePairingAvailability {
    PhonePairingAvailability {
        available: false,
        reason: Some(CLI_MISSING.to_owned()),
        gateway: None,
    }
}

fn present(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

fn device_segment(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/devices/")?;
    single_segment(rest)
}

fn phone_segment(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/phone-pairing/")?;
    single_segment(rest)
}

fn single_segment(rest: &str) -> Option<&str> {
    if rest.is_empty() || rest.contains('/') {
        None
    } else {
        Some(rest)
    }
}

async fn ask_result<T, F>(gateway: &Gateway, command: F) -> Result<T, DeviceError>
where
    F: FnOnce(oneshot::Sender<Result<T, DeviceError>>) -> actor::Command,
{
    ask(gateway, command).await?
}

async fn read_json<T: DeserializeOwned>(request: Request) -> Result<T, ApiError> {
    Ok(Body::<T>::from_request(request, &()).await?.0)
}

async fn ask<T, F>(gateway: &Gateway, command: F) -> Result<T, DeviceError>
where
    F: FnOnce(oneshot::Sender<T>) -> actor::Command,
{
    let (tx, rx) = oneshot::channel();
    let sender = gateway.devices().sender()?;
    match tokio::time::timeout(CALL_TIMEOUT, sender.send(command(tx))).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "device registry is not accepting work");
            return Err(DeviceError::Registry);
        }
        Err(error) => {
            tracing::warn!(%error, "device registry queue timed out");
            return Err(DeviceError::Registry);
        }
    }
    match tokio::time::timeout(CALL_TIMEOUT, rx).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            tracing::warn!(%error, "device registry dropped the reply");
            Err(DeviceError::Registry)
        }
        Err(error) => {
            tracing::warn!(%error, "device registry timed out");
            Err(DeviceError::Registry)
        }
    }
}
