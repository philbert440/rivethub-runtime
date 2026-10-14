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

use super::DeviceError;
use super::pairing;
use super::pairing::PhoneIssuer;
use super::store;
use super::store::Enrollment;

#[derive(Clone)]
pub(super) struct ActorConfig {
    pub(super) enrollment: Option<Enrollment>,
    pub(super) phone: Option<PhoneIssuer>,
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
        let config = config.clone();
        let joined = tokio::task::spawn_blocking(move || execute(&config, command)).await;
        if let Err(error) = joined {
            tracing::warn!(%error, "device registry task stopped");
        }
    }
}

fn execute(config: &ActorConfig, command: Command) {
    match command {
        Command::List(reply) => reply_with(reply, list(config)),
        Command::Create { name, reply } => {
            reply_with(reply, create(config, name.as_deref()));
        }
        Command::Enroll {
            token,
            public_key,
            name,
            reply,
        } => reply_with(reply, enroll(config, &token, &public_key, name.as_deref())),
        Command::Delete { id, reply } => reply_with(reply, delete(config, &id)),
        Command::Availability(reply) => reply_with(reply, availability(config)),
        Command::Start { name, reply } => reply_with(reply, start(config, &name)),
        Command::State { name, reply } => reply_with(reply, state(config, &name)),
        Command::Redeem { token, reply } => reply_with(reply, redeem(config, &token)),
    }
}

fn reply_with<T>(sender: oneshot::Sender<T>, value: T) {
    if sender.send(value).is_err() {
        tracing::warn!("device caller dropped the reply");
    }
}

fn list(config: &ActorConfig) -> Result<DeviceListResponse, DeviceError> {
    let enrollment = config.enrollment.as_ref().ok_or(DeviceError::Disabled)?;
    store::list(enrollment)
}

fn create(config: &ActorConfig, name: Option<&str>) -> Result<DeviceCreateResponse, DeviceError> {
    let enrollment = config.enrollment.as_ref().ok_or(DeviceError::Disabled)?;
    store::create(enrollment, name)
}

fn enroll(
    config: &ActorConfig,
    token: &str,
    public_key: &str,
    name: Option<&str>,
) -> Result<DeviceEnrollResponse, DeviceError> {
    let enrollment = config.enrollment.as_ref().ok_or(DeviceError::Disabled)?;
    store::enroll(enrollment, token, public_key, name)
}

fn delete(config: &ActorConfig, id: &str) -> Result<OkReply, DeviceError> {
    let enrollment = config.enrollment.as_ref().ok_or(DeviceError::Disabled)?;
    store::delete(enrollment, id)
}

fn availability(config: &ActorConfig) -> PhonePairingAvailability {
    match &config.phone {
        Some(issuer) => pairing::availability(issuer),
        None => PhonePairingAvailability {
            available: false,
            reason: Some(super::CA_UNUSABLE.to_owned()),
            gateway: None,
        },
    }
}

fn start(config: &ActorConfig, name: &str) -> Result<PhonePairingCode, DeviceError> {
    let issuer = config.phone.as_ref().ok_or(DeviceError::Internal)?;
    pairing::start(issuer, name)
}

fn state(config: &ActorConfig, name: &str) -> Result<PhonePairingStatus, DeviceError> {
    let issuer = config.phone.as_ref().ok_or(DeviceError::Internal)?;
    pairing::state(issuer, name)
}

fn redeem(config: &ActorConfig, token: &str) -> Result<PairRedeemResponse, DeviceError> {
    let issuer = config
        .phone
        .as_ref()
        .ok_or(DeviceError::Forbidden(super::PAIR_CODE))?;
    pairing::redeem(issuer, token)
}
