use kernel::ca::CertificateAuthority;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use wire::gateway::OkReply;
use wire::gateway::devices::DeviceCreateResponse;
use wire::gateway::devices::DeviceEnrollResponse;
use wire::gateway::devices::DeviceListResponse;
use wire::gateway::devices::PairRedeemResponse;
use wire::gateway::devices::PhonePairingAvailability;
use wire::gateway::devices::PhonePairingCode;
use wire::gateway::devices::PhonePairingStatus;

use super::CSR_REQUIRED;
use super::DeviceError;
use super::PAIR_CODE;
use super::authority;
use super::authority::SharedAuthority;
use super::pairing;
use super::pairing::PhoneIssuer;
use super::store;
use super::store::Enrollment;

#[derive(Clone)]
pub(super) struct ActorConfig {
    pub(super) enrollment: Option<Enrollment>,
    pub(super) phone: Option<PhoneIssuer>,
    pub(super) authority: Option<SharedAuthority>,
}

pub(super) enum Command {
    List(oneshot::Sender<Result<DeviceListResponse, DeviceError>>),
    Create {
        name: Option<String>,
        reply: oneshot::Sender<Result<DeviceCreateResponse, DeviceError>>,
    },
    Enroll {
        token: String,
        public_key: String,
        name: Option<String>,
        csr: Option<String>,
        reply: oneshot::Sender<Result<DeviceEnrollResponse, DeviceError>>,
    },
    Delete {
        id: String,
        reply: oneshot::Sender<Result<OkReply, DeviceError>>,
    },
    Availability(oneshot::Sender<PhonePairingAvailability>),
    Start {
        name: String,
        reply: oneshot::Sender<Result<PhonePairingCode, DeviceError>>,
    },
    State {
        name: String,
        reply: oneshot::Sender<Result<PhonePairingStatus, DeviceError>>,
    },
    Redeem {
        token: String,
        csr: Option<String>,
        reply: oneshot::Sender<Result<PairRedeemResponse, DeviceError>>,
    },
}

#[derive(Debug)]
pub(super) struct ActorHandle {
    pub(super) tx: mpsc::Sender<Command>,
    join: tokio::task::JoinHandle<()>,
}

impl ActorHandle {
    pub(super) fn abort(&self) {
        self.join.abort();
    }
}

pub(super) fn spawn(
    runtime: &tokio::runtime::Handle,
    cancel: CancellationToken,
    config: ActorConfig,
) -> ActorHandle {
    let (tx, rx) = mpsc::channel(32);
    let join = runtime.spawn(run(rx, cancel, config));
    ActorHandle { tx, join }
}

async fn run(mut rx: mpsc::Receiver<Command>, cancel: CancellationToken, config: ActorConfig) {
    loop {
        let command = tokio::select! {
            () = cancel.cancelled() => break,
            received = rx.recv() => match received {
                Some(command) => command,
                None => break,
            },
        };
        tokio::select! {
            () = cancel.cancelled() => break,
            () = dispatch(&config, command) => {}
        }
    }
}

async fn dispatch(config: &ActorConfig, command: Command) {
    match command {
        Command::List(reply) => reply_with(reply, list(config).await),
        Command::Create { name, reply } => reply_with(reply, create(config, name).await),
        Command::Enroll {
            token,
            public_key,
            name,
            csr,
            reply,
        } => reply_with(reply, enroll(config, token, public_key, name, csr).await),
        Command::Delete { id, reply } => reply_with(reply, delete(config, id).await),
        Command::Availability(reply) => reply_with(reply, availability(config).await),
        Command::Start { name, reply } => reply_with(reply, start(config, name).await),
        Command::State { name, reply } => reply_with(reply, state(config, name).await),
        Command::Redeem { token, csr, reply } => {
            reply_with(reply, redeem(config, token, csr).await);
        }
    }
}

fn reply_with<T>(sender: oneshot::Sender<T>, value: T) {
    if sender.send(value).is_err() {
        tracing::warn!("device caller dropped the reply");
    }
}

async fn blocking_result<T, F>(work: F) -> Result<T, DeviceError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DeviceError> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(%error, "device registry task stopped");
            Err(DeviceError::Registry)
        }
    }
}

fn authority_of(config: &ActorConfig) -> Result<&dyn CertificateAuthority, DeviceError> {
    if let Some(authority) = &config.authority {
        Ok(authority.get())
    } else {
        tracing::warn!("device certificate authority is not configured");
        Err(DeviceError::unavailable(super::CA_UNUSABLE))
    }
}

async fn list(config: &ActorConfig) -> Result<DeviceListResponse, DeviceError> {
    let enrollment = config.enrollment.clone().ok_or(DeviceError::Disabled)?;
    blocking_result(move || store::list(&enrollment)).await
}

async fn create(
    config: &ActorConfig,
    name: Option<String>,
) -> Result<DeviceCreateResponse, DeviceError> {
    let enrollment = config.enrollment.clone().ok_or(DeviceError::Disabled)?;
    let authority = authority_of(config)?;
    let enrollment_for_prepare = enrollment.clone();
    let draft =
        blocking_result(move || store::prepare_create(&enrollment_for_prepare, name.as_deref()))
            .await?;
    let token = authority::issue_device_token(authority, enrollment.owner(), &draft.id).await?;
    let enrollment_for_commit = enrollment.clone();
    blocking_result(move || store::commit_create(&enrollment_for_commit, draft, &token)).await
}

async fn enroll(
    config: &ActorConfig,
    token: String,
    public_key: String,
    name: Option<String>,
    csr: Option<String>,
) -> Result<DeviceEnrollResponse, DeviceError> {
    let enrollment = config.enrollment.clone().ok_or(DeviceError::Disabled)?;
    let authority = authority_of(config)?;
    let enrollment_for_prepare = enrollment.clone();
    let token_for_prepare = token.clone();
    let public_key_for_prepare = public_key.clone();
    let ready = blocking_result(move || {
        store::prepare_enroll(
            &enrollment_for_prepare,
            &token_for_prepare,
            &public_key_for_prepare,
            csr.as_deref(),
        )
    })
    .await?;
    let certificate = authority::sign_device(
        authority,
        &ready.csr,
        &token,
        enrollment.owner(),
        &ready.device_id,
    )
    .await?;
    let device_id = ready.device_id.clone();
    let owner = enrollment.owner().to_owned();
    let enrollment_for_commit = enrollment.clone();
    let token_for_commit = token.clone();
    match blocking_result(move || {
        store::commit_enroll(
            &enrollment_for_commit,
            &token_for_commit,
            &public_key,
            name.as_deref(),
            &certificate,
        )
    })
    .await
    {
        Ok(response) => Ok(response),
        Err(error) => {
            authority::revoke_device(authority, &owner, &device_id).await?;
            Err(error)
        }
    }
}

async fn delete(config: &ActorConfig, id: String) -> Result<OkReply, DeviceError> {
    let enrollment = config.enrollment.clone().ok_or(DeviceError::Disabled)?;
    blocking_result(move || store::delete(&enrollment, &id)).await
}

async fn availability(config: &ActorConfig) -> PhonePairingAvailability {
    match &config.phone {
        Some(issuer) => {
            pairing::availability(issuer, config.authority.as_ref().map(SharedAuthority::get)).await
        }
        None => PhonePairingAvailability {
            available: false,
            reason: Some(super::CA_UNUSABLE.to_owned()),
            gateway: None,
        },
    }
}

async fn start(config: &ActorConfig, name: String) -> Result<PhonePairingCode, DeviceError> {
    let issuer = config.phone.clone().ok_or(DeviceError::Internal)?;
    let authority = authority_of(config)?;
    pairing::start(&issuer, authority, &name).await
}

async fn state(config: &ActorConfig, name: String) -> Result<PhonePairingStatus, DeviceError> {
    let issuer = config.phone.clone().ok_or(DeviceError::Internal)?;
    blocking_result(move || pairing::state(&issuer, &name)).await
}

async fn redeem(
    config: &ActorConfig,
    token: String,
    csr: Option<String>,
) -> Result<PairRedeemResponse, DeviceError> {
    let issuer = config
        .phone
        .clone()
        .ok_or(DeviceError::Forbidden(PAIR_CODE))?;
    let authority = authority_of(config)?;
    let issuer_for_prepare = issuer.clone();
    let token_for_prepare = token.clone();
    let prepared =
        blocking_result(move || pairing::prepare_redeem(&issuer_for_prepare, &token_for_prepare))
            .await?;
    let Some(csr) = csr else {
        return Err(DeviceError::BadRequest(CSR_REQUIRED));
    };
    let certificate =
        authority::sign_device(authority, &csr, &token, issuer.owner(), &prepared.device_id)
            .await?;
    let device_id = prepared.device_id.clone();
    let owner = issuer.owner().to_owned();
    let issuer_for_claim = issuer.clone();
    let token_for_claim = token.clone();
    match blocking_result(move || pairing::claim(&issuer_for_claim, &token_for_claim)).await {
        Ok(()) => Ok(pairing::response(prepared.device_id, certificate)),
        Err(error) => {
            authority::revoke_device(authority, &owner, &device_id).await?;
            Err(error)
        }
    }
}
