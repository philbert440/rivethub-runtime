use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use wire::gateway::OkReply;
use wire::gateway::devices::DeviceCreateResponse;
use wire::gateway::devices::DeviceEnrollConfig;
use wire::gateway::devices::DeviceEnrollResponse;
use wire::gateway::devices::DeviceListResponse;
use wire::gateway::devices::DeviceRosterFile;
use wire::gateway::devices::EnrolledDevice;
use wire::gateway::devices::EnrollmentQr;
use wire::gateway::devices::MeshDevice;
use wire::gateway::devices::PendingEnrollment;
use wire::gateway::devices::SignedCertificate;
use wire::gateway::devices::StoredPending;

use super::ALREADY_ENROLLED;
use super::CODE_TTL;
use super::CSR_REQUIRED;
use super::DeviceError;
use super::ENROLL_FIELDS;
use super::ENROLL_TOKEN;
use super::POOL_EXHAUSTED;
use super::RELAY_FAILED;
use super::SetupError;
use super::pathutil;
use super::pathutil::RosterBytes;
use super::random;

const NAME_LIMIT: usize = 64;
const DEFAULT_NAME: &str = "device";
const ROSTER_VERSION: u32 = 1;
const QR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AddressPool {
    base: u32,
    mask: u32,
}

impl AddressPool {
    pub(super) fn parse(text: &str) -> Result<Self, SetupError> {
        let Some((address, prefix_text)) = text.split_once('/') else {
            return Err(SetupError::Pool);
        };
        let ip = address.parse::<Ipv4Addr>().map_err(|_| SetupError::Pool)?;
        let prefix = prefix_text.parse::<u8>().map_err(|_| SetupError::Pool)?;
        if !(1..=30).contains(&prefix) {
            return Err(SetupError::Pool);
        }
        let shift = 32 - u32::from(prefix);
        let mask = u32::MAX << shift;
        let base = u32::from(ip);
        if base & !mask != 0 {
            return Err(SetupError::Pool);
        }
        Ok(Self { base, mask })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAccount {
    pub pg_role: String,
    pub owner: String,
}

#[derive(Debug, Clone)]
pub struct Enrollment {
    roster_path: PathBuf,
    pool: Option<AddressPool>,
    gateway_url: String,
    owner: String,
    config: DeviceEnrollConfig,
    relay_ssh: Option<String>,
    pg_role: String,
}

impl Enrollment {
    pub fn try_new(
        roster_path: PathBuf,
        pool: Option<&str>,
        gateway_url: String,
        config: DeviceEnrollConfig,
        relay_ssh: Option<String>,
        account: DeviceAccount,
    ) -> Result<Self, SetupError> {
        pathutil::absolute_without_parents(&roster_path)?;
        if gateway_url.is_empty() {
            return Err(SetupError::GatewayUrl);
        }
        if account.pg_role.is_empty() {
            return Err(SetupError::Role);
        }
        if account.owner.is_empty() {
            return Err(SetupError::Owner);
        }
        if !config_complete(&config) {
            return Err(SetupError::Config);
        }
        if relay_ssh.as_ref().is_some_and(String::is_empty) {
            return Err(SetupError::Relay);
        }
        let pool = match pool {
            Some(text) => Some(AddressPool::parse(text)?),
            None => None,
        };
        Ok(Self {
            roster_path,
            pool,
            gateway_url,
            owner: account.owner,
            config,
            relay_ssh,
            pg_role: account.pg_role,
        })
    }

    pub(super) fn owner(&self) -> &str {
        &self.owner
    }

    fn relay_configured(&self) -> bool {
        self.relay_ssh.is_some()
    }
}

pub(super) struct PendingDraft {
    pub(super) id: String,
    name: String,
    address: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

pub(super) struct EnrollReady {
    pub(super) device_id: String,
    pub(super) csr: String,
}

pub(super) fn list(enrollment: &Enrollment) -> Result<DeviceListResponse, DeviceError> {
    let roster = load(enrollment)?;
    Ok(DeviceListResponse {
        devices: roster.devices,
        pending: roster.pending.into_iter().map(pending_view).collect(),
        relay_configured: enrollment.relay_configured(),
    })
}

pub(super) fn roster_name(
    enrollment: &Enrollment,
    id: &str,
) -> Result<Option<String>, DeviceError> {
    Ok(load(enrollment)?
        .devices
        .into_iter()
        .find(|device| device.id == id)
        .map(|device| device.name.trim().to_owned())
        .filter(|name| !name.is_empty()))
}

pub(super) fn prepare_create(
    enrollment: &Enrollment,
    requested_name: Option<&str>,
) -> Result<PendingDraft, DeviceError> {
    let roster = load(enrollment)?;
    let now = Utc::now();
    Ok(PendingDraft {
        id: random::device_id()?,
        name: device_name(requested_name),
        address: allocate(enrollment, &roster)?,
        created_at: now,
        expires_at: expiry(now)?,
    })
}

pub(super) fn commit_create(
    enrollment: &Enrollment,
    draft: PendingDraft,
    token: &str,
) -> Result<DeviceCreateResponse, DeviceError> {
    if token.is_empty() {
        tracing::warn!("device enrollment token is empty");
        return Err(DeviceError::unavailable(super::CA_UNUSABLE));
    }
    let mut roster = load(enrollment)?;
    if address_taken(&roster, &draft.address) {
        return Err(DeviceError::Conflict(POOL_EXHAUSTED.to_owned()));
    }
    let pending = StoredPending {
        id: draft.id,
        name: draft.name,
        token: token.to_owned(),
        address: draft.address.clone(),
        created_at: draft.created_at,
        expires_at: draft.expires_at,
    };
    let view = pending_view(pending.clone());
    let qr = EnrollmentQr {
        v: QR_VERSION,
        gateway: enrollment.gateway_url.clone(),
        token: pending.token.clone(),
        config: issued_config(enrollment, &draft.address),
    };
    roster.pending.push(pending);
    save(enrollment, &roster)?;
    Ok(DeviceCreateResponse { pending: view, qr })
}

pub(super) fn prepare_enroll(
    enrollment: &Enrollment,
    token: &str,
    public_key: &str,
    csr: Option<&str>,
) -> Result<EnrollReady, DeviceError> {
    let pending = live_pending(enrollment, token, public_key)?;
    if enrollment.relay_configured() {
        return Err(DeviceError::BadGateway(RELAY_FAILED.to_owned()));
    }
    let Some(csr) = csr.filter(|text| !text.is_empty()) else {
        return Err(DeviceError::BadRequest(CSR_REQUIRED));
    };
    Ok(EnrollReady {
        device_id: pending.id,
        csr: csr.to_owned(),
    })
}

pub(super) fn commit_enroll(
    enrollment: &Enrollment,
    token: &str,
    public_key: &str,
    requested_name: Option<&str>,
    certificate: &SignedCertificate,
) -> Result<DeviceEnrollResponse, DeviceError> {
    let mut roster = load(enrollment)?;
    let Some(index) = pending_index(&roster.pending, token) else {
        return Err(DeviceError::Forbidden(ENROLL_TOKEN));
    };
    let now = Utc::now();
    let pending = roster
        .pending
        .get(index)
        .ok_or(DeviceError::Internal)?
        .clone();
    if pending.expires_at <= now {
        return Err(DeviceError::Forbidden(ENROLL_TOKEN));
    }
    if public_key_enrolled(&roster.devices, public_key) {
        return Err(DeviceError::Conflict(ALREADY_ENROLLED.to_owned()));
    }
    if enrollment.relay_configured() {
        return Err(DeviceError::BadGateway(RELAY_FAILED.to_owned()));
    }
    let name = match requested_name {
        Some(name) => clip_name(name),
        None => pending.name.clone(),
    };
    let device = MeshDevice {
        id: pending.id.clone(),
        name: name.clone(),
        public_key: public_key.to_owned(),
        address: pending.address.clone(),
        created_at: pending.created_at,
        enrolled_at: Some(now),
        last_handshake: None,
        pg_role: Some(enrollment.pg_role.clone()),
    };
    roster.pending.remove(index);
    let config = issued_config(enrollment, &device.address);
    let enrolled = EnrolledDevice {
        id: device.id.clone(),
        name,
        address: device.address.clone(),
    };
    roster.devices.push(device);
    save(enrollment, &roster)?;
    Ok(DeviceEnrollResponse {
        ok: true,
        device: enrolled,
        config,
        certificate: certificate.clone(),
    })
}

pub(super) fn delete(enrollment: &Enrollment, id: &str) -> Result<OkReply, DeviceError> {
    let mut roster = load(enrollment)?;
    let before = roster.devices.len() + roster.pending.len();
    roster.devices.retain(|device| device.id != id);
    roster.pending.retain(|pending| pending.id != id);
    if roster.devices.len() + roster.pending.len() == before {
        return Err(DeviceError::UnknownDevice);
    }
    save(enrollment, &roster)?;
    Ok(OkReply { ok: true })
}

fn load(enrollment: &Enrollment) -> Result<DeviceRosterFile, DeviceError> {
    match pathutil::read_roster_file(&enrollment.roster_path)? {
        RosterBytes::Missing => Ok(empty_roster()),
        RosterBytes::Ready(bytes) => parse_roster(&bytes),
    }
}

fn empty_roster() -> DeviceRosterFile {
    DeviceRosterFile {
        v: ROSTER_VERSION,
        devices: Vec::new(),
        pending: Vec::new(),
    }
}

fn parse_roster(bytes: &[u8]) -> Result<DeviceRosterFile, DeviceError> {
    let roster = match serde_json::from_slice::<DeviceRosterFile>(bytes) {
        Ok(roster) => roster,
        Err(error) => {
            tracing::warn!(%error, "device roster is corrupt");
            return Err(DeviceError::Registry);
        }
    };
    if roster.v != ROSTER_VERSION {
        tracing::warn!(version = roster.v, "device roster version is not supported");
        return Err(DeviceError::Registry);
    }
    for device in &roster.devices {
        host_address(&device.address)?;
    }
    for pending in &roster.pending {
        host_address(&pending.address)?;
    }
    Ok(roster)
}

fn save(enrollment: &Enrollment, roster: &DeviceRosterFile) -> Result<(), DeviceError> {
    let bytes = serde_json::to_vec(roster).map_err(|error| {
        tracing::warn!(%error, "device roster did not serialize");
        DeviceError::Internal
    })?;
    pathutil::write_atomic(&enrollment.roster_path, &bytes)
}

fn live_pending(
    enrollment: &Enrollment,
    token: &str,
    public_key: &str,
) -> Result<StoredPending, DeviceError> {
    if !valid_wg_key(public_key) || token.is_empty() {
        return Err(DeviceError::BadRequest(ENROLL_FIELDS));
    }
    let roster = load(enrollment)?;
    let Some(index) = pending_index(&roster.pending, token) else {
        return Err(DeviceError::Forbidden(ENROLL_TOKEN));
    };
    let pending = roster
        .pending
        .get(index)
        .ok_or(DeviceError::Internal)?
        .clone();
    if pending.expires_at <= Utc::now() {
        return Err(DeviceError::Forbidden(ENROLL_TOKEN));
    }
    if public_key_enrolled(&roster.devices, public_key) {
        return Err(DeviceError::Conflict(ALREADY_ENROLLED.to_owned()));
    }
    Ok(pending)
}

fn address_taken(roster: &DeviceRosterFile, address: &str) -> bool {
    roster
        .devices
        .iter()
        .any(|device| device.address == address)
        || roster
            .pending
            .iter()
            .any(|pending| pending.address == address)
}

fn pending_view(pending: StoredPending) -> PendingEnrollment {
    PendingEnrollment {
        id: pending.id,
        name: pending.name,
        address: pending.address,
        created_at: pending.created_at,
        expires_at: pending.expires_at,
    }
}

fn issued_config(enrollment: &Enrollment, address: &str) -> DeviceEnrollConfig {
    let mut config = enrollment.config.clone();
    address.clone_into(&mut config.wg_address);
    config
}

fn allocate(enrollment: &Enrollment, roster: &DeviceRosterFile) -> Result<String, DeviceError> {
    let Some(pool) = &enrollment.pool else {
        return Err(DeviceError::Conflict(POOL_EXHAUSTED.to_owned()));
    };
    let mut used = BTreeSet::new();
    for device in &roster.devices {
        used.insert(host_address(&device.address)?);
    }
    for pending in &roster.pending {
        used.insert(host_address(&pending.address)?);
    }
    let broadcast = pool.base | !pool.mask;
    let mut cursor = pool.base.checked_add(1).ok_or(DeviceError::Internal)?;
    for taken in &used {
        if *taken < cursor {
            continue;
        }
        if *taken > cursor || cursor >= broadcast {
            break;
        }
        cursor = cursor.checked_add(1).ok_or(DeviceError::Internal)?;
    }
    if cursor >= broadcast {
        return Err(DeviceError::Conflict(POOL_EXHAUSTED.to_owned()));
    }
    Ok(format!("{}/32", Ipv4Addr::from(cursor)))
}

fn host_address(address: &str) -> Result<u32, DeviceError> {
    let Some((ip, prefix)) = address.split_once('/') else {
        tracing::warn!("device roster address is not a host prefix");
        return Err(DeviceError::Registry);
    };
    if prefix != "32" {
        tracing::warn!("device roster address is not a /32");
        return Err(DeviceError::Registry);
    }
    ip.parse::<Ipv4Addr>().map(u32::from).map_err(|_| {
        tracing::warn!("device roster address is not ipv4");
        DeviceError::Registry
    })
}

fn pending_index(pending: &[StoredPending], token: &str) -> Option<usize> {
    let mut found = None;
    for (index, row) in pending.iter().enumerate() {
        if pathutil::tokens_equal(token.as_bytes(), row.token.as_bytes()) && found.is_none() {
            found = Some(index);
        }
    }
    found
}

fn public_key_enrolled(devices: &[MeshDevice], public_key: &str) -> bool {
    let mut found = false;
    for device in devices {
        if pathutil::tokens_equal(device.public_key.as_bytes(), public_key.as_bytes()) {
            found = true;
        }
    }
    found
}

fn device_name(requested: Option<&str>) -> String {
    match requested {
        None => DEFAULT_NAME.to_owned(),
        Some(name) => clip_name(name),
    }
}

fn clip_name(name: &str) -> String {
    name.chars().take(NAME_LIMIT).collect()
}

fn expiry(now: DateTime<Utc>) -> Result<DateTime<Utc>, DeviceError> {
    now.checked_add_signed(CODE_TTL)
        .ok_or(DeviceError::Internal)
}

pub(super) fn valid_wg_key(key: &str) -> bool {
    let Some((body, last)) = key.split_at_checked(key.len().saturating_sub(1)) else {
        return false;
    };
    if last != "=" || !(42..=44).contains(&body.len()) {
        return false;
    }
    body.bytes()
        .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/'))
}

fn config_complete(config: &DeviceEnrollConfig) -> bool {
    !config.shared_host.is_empty()
        && !config.shared_export.is_empty()
        && !config.pg_url.is_empty()
        && !config.embed_url.is_empty()
        && !config.wg_endpoint.is_empty()
        && !config.wg_peer_public_key.is_empty()
        && !config.wg_address.is_empty()
        && !config.wg_allowed_ips.is_empty()
        && !config.home_subnet.is_empty()
}
