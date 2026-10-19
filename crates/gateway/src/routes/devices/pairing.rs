use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use kernel::ca::CertificateAuthority;
use sha2::Digest;
use sha2::Sha256;
use wire::gateway::devices::PairRedeemResponse;
use wire::gateway::devices::PairingRecord;
use wire::gateway::devices::PhonePairingAvailability;
use wire::gateway::devices::PhonePairingCode;
use wire::gateway::devices::PhonePairingState;
use wire::gateway::devices::PhonePairingStatus;
use wire::gateway::devices::SignedCertificate;

use super::CA_UNUSABLE;
use super::CODE_TTL;
use super::DeviceError;
use super::PAIR_CODE;
use super::authority;
use super::pathutil;
use super::pathutil::OpenFile;
use super::random;

const RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct PhoneIssuer {
    pairing_dir: PathBuf,
    gateway_url: String,
    owner: String,
}

impl PhoneIssuer {
    pub fn try_new(
        pairing_dir: PathBuf,
        gateway_url: String,
        owner: String,
    ) -> Result<Self, super::SetupError> {
        pathutil::absolute_without_parents(&pairing_dir)?;
        if gateway_url.is_empty() {
            return Err(super::SetupError::GatewayUrl);
        }
        if owner.is_empty() {
            return Err(super::SetupError::Owner);
        }
        Ok(Self {
            pairing_dir,
            gateway_url,
            owner,
        })
    }

    pub(super) fn owner(&self) -> &str {
        &self.owner
    }
}

pub(super) async fn availability(
    issuer: &PhoneIssuer,
    authority: Option<&dyn CertificateAuthority>,
) -> PhonePairingAvailability {
    let Some(authority) = authority else {
        return ca_unavailable();
    };
    match authority::chain_ready(authority).await {
        Ok(()) => PhonePairingAvailability {
            available: true,
            reason: None,
            gateway: Some(issuer.gateway_url.clone()),
        },
        Err(error) => {
            tracing::warn!(%error, "phone pairing certificate authority is not usable");
            ca_unavailable()
        }
    }
}

pub(super) async fn start(
    issuer: &PhoneIssuer,
    authority: &dyn CertificateAuthority,
    name: &str,
) -> Result<PhonePairingCode, DeviceError> {
    authority::chain_ready(authority).await?;
    let device_id = random::device_id()?;
    let token = authority::issue_device_token(authority, issuer.owner(), &device_id).await?;
    if !pathutil::component_ok(&token) {
        tracing::warn!("enrollment token is not a single path component");
        revoke_issued(authority, issuer, &device_id).await?;
        return Err(DeviceError::unavailable(CA_UNUSABLE));
    }
    let expires_at = Utc::now()
        .checked_add_signed(CODE_TTL)
        .ok_or(DeviceError::Internal)?;
    let record = PairingRecord {
        v: RECORD_VERSION,
        device_id: device_id.clone(),
        name: name.to_owned(),
        expires_at,
    };
    if let Err(error) = write_record(
        &issuer.pairing_dir.join("pending"),
        &record_stem(&token),
        &record,
    ) {
        revoke_issued(authority, issuer, &device_id).await?;
        return Err(error);
    }
    Ok(PhonePairingCode {
        device_id,
        gateway: issuer.gateway_url.clone(),
        expires_at,
        qr_text: token,
        reshown: 0,
        added_to_users: false,
    })
}

pub(super) fn state(issuer: &PhoneIssuer, name: &str) -> Result<PhonePairingStatus, DeviceError> {
    let now = Utc::now();
    let pending = load_dir(&issuer.pairing_dir.join("pending"))?;
    let claimed = load_dir(&issuer.pairing_dir.join("claimed"))?;
    Ok(PhonePairingStatus {
        state: state_of(name, &pending, &claimed, now),
    })
}

pub(super) fn claimed_name(
    issuer: &PhoneIssuer,
    device_id: &str,
) -> Result<Option<String>, DeviceError> {
    Ok(load_dir(&issuer.pairing_dir.join("claimed"))?
        .into_iter()
        .find(|record| record.device_id == device_id)
        .map(|record| record.name))
}

pub(super) struct PreparedRedeem {
    pub(super) device_id: String,
}

pub(super) fn prepare_redeem(
    issuer: &PhoneIssuer,
    token: &str,
) -> Result<PreparedRedeem, DeviceError> {
    let record = read_pending(issuer, token)?;
    if record.expires_at <= Utc::now() {
        return Err(DeviceError::Forbidden(PAIR_CODE));
    }
    Ok(PreparedRedeem {
        device_id: record.device_id,
    })
}

pub(super) fn claim(issuer: &PhoneIssuer, token: &str) -> Result<(), DeviceError> {
    let pending = issuer.pairing_dir.join("pending");
    let Some(stem) = pending_stem(token, &pending)? else {
        return Err(DeviceError::Forbidden(PAIR_CODE));
    };
    let path = match pathutil::open_pairing_file(&pending, &stem, "json")? {
        OpenFile::Ready(path) => path,
        OpenFile::Missing => return Err(DeviceError::Forbidden(PAIR_CODE)),
        OpenFile::Escape => {
            tracing::warn!("pairing code path leaves its directory");
            return Err(DeviceError::Forbidden(PAIR_CODE));
        }
    };
    let record = read_record(&path)?;
    if record.expires_at <= Utc::now() {
        return Err(DeviceError::Forbidden(PAIR_CODE));
    }
    let claimed = pathutil::ensure_dir(&issuer.pairing_dir.join("claimed"))?;
    let Some(dest_name) = pathutil::pairing_file(&stem, "json") else {
        return Err(DeviceError::Forbidden(PAIR_CODE));
    };
    std::fs::rename(&path, claimed.join(dest_name)).map_err(|error| pathutil::io_registry(&error))
}

pub(super) fn response(device_id: String, certificate: SignedCertificate) -> PairRedeemResponse {
    PairRedeemResponse {
        device_id,
        certificate,
    }
}

fn ca_unavailable() -> PhonePairingAvailability {
    PhonePairingAvailability {
        available: false,
        reason: Some(CA_UNUSABLE.to_owned()),
        gateway: None,
    }
}

async fn revoke_issued(
    authority: &dyn CertificateAuthority,
    issuer: &PhoneIssuer,
    device_id: &str,
) -> Result<(), DeviceError> {
    authority::revoke_device(authority, issuer.owner(), device_id).await
}

fn read_pending(issuer: &PhoneIssuer, token: &str) -> Result<PairingRecord, DeviceError> {
    let pending = issuer.pairing_dir.join("pending");
    let Some(stem) = pending_stem(token, &pending)? else {
        return Err(DeviceError::Forbidden(PAIR_CODE));
    };
    match pathutil::open_pairing_file(&pending, &stem, "json")? {
        OpenFile::Ready(path) => read_record(&path),
        OpenFile::Missing => Err(DeviceError::Forbidden(PAIR_CODE)),
        OpenFile::Escape => {
            tracing::warn!("pairing code path leaves its directory");
            Err(DeviceError::Forbidden(PAIR_CODE))
        }
    }
}

fn pending_stem(token: &str, pending: &Path) -> Result<Option<String>, DeviceError> {
    let names = list_stems(pending)?;
    Ok(select_token(&record_stem(token), &names))
}

fn record_stem(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn state_of(
    key: &str,
    pending: &[PairingRecord],
    claimed: &[PairingRecord],
    now: DateTime<Utc>,
) -> PhonePairingState {
    let live = |record: &PairingRecord| {
        (record.name == key || record.device_id == key) && record.expires_at > now
    };
    if claimed.iter().any(live) {
        PhonePairingState::Paired
    } else if pending.iter().any(live) {
        PhonePairingState::Pending
    } else {
        PhonePairingState::Expired
    }
}

fn load_dir(dir: &Path) -> Result<Vec<PairingRecord>, DeviceError> {
    let mut records = Vec::new();
    for stem in list_stems(dir)? {
        match pathutil::open_pairing_file(dir, &stem, "json")? {
            OpenFile::Ready(path) => records.push(read_record(&path)?),
            OpenFile::Missing => {
                tracing::warn!("pairing record disappeared while it was listed");
                return Err(DeviceError::Registry);
            }
            OpenFile::Escape => {
                tracing::warn!("pairing record path leaves its directory");
                return Err(DeviceError::Registry);
            }
        }
    }
    Ok(records)
}

fn list_stems(dir: &Path) -> Result<Vec<String>, DeviceError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(pathutil::io_registry(&error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| pathutil::io_registry(&error))?;
        let file_name = entry.file_name();
        let Some(text) = file_name.to_str() else {
            continue;
        };
        let Some(stem) = text.strip_suffix(".json") else {
            continue;
        };
        if pathutil::component_ok(stem) {
            names.push(stem.to_owned());
        }
    }
    Ok(names)
}

fn select_token(token: &str, names: &[String]) -> Option<String> {
    let mut found = None;
    for name in names {
        if pathutil::tokens_equal(token.as_bytes(), name.as_bytes()) && found.is_none() {
            found = Some(name.clone());
        }
    }
    found
}

fn write_record(dir: &Path, stem: &str, record: &PairingRecord) -> Result<(), DeviceError> {
    let root = pathutil::ensure_dir(dir)?;
    let Some(name) = pathutil::pairing_file(stem, "json") else {
        return Err(DeviceError::Internal);
    };
    let bytes = serde_json::to_vec(record).map_err(|error| {
        tracing::warn!(%error, "pairing record did not serialize");
        DeviceError::Internal
    })?;
    pathutil::write_atomic(&root.join(name), &bytes)
}

fn read_record(path: &Path) -> Result<PairingRecord, DeviceError> {
    let bytes = std::fs::read(path).map_err(|error| pathutil::io_registry(&error))?;
    let record = match serde_json::from_slice::<PairingRecord>(&bytes) {
        Ok(record) => record,
        Err(error) => {
            tracing::warn!(%error, "pairing record is corrupt");
            return Err(DeviceError::Registry);
        }
    };
    if record.v != RECORD_VERSION {
        tracing::warn!(
            version = record.v,
            "pairing record version is not supported"
        );
        return Err(DeviceError::Registry);
    }
    if !pathutil::component_ok(&record.device_id) {
        tracing::warn!("pairing record device id is not a single path component");
        return Err(DeviceError::Registry);
    }
    Ok(record)
}
