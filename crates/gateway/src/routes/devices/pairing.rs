use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use wire::gateway::devices::PairRedeemResponse;
use wire::gateway::devices::PairingRecord;
use wire::gateway::devices::PhonePairingAvailability;
use wire::gateway::devices::PhonePairingCode;
use wire::gateway::devices::PhonePairingState;
use wire::gateway::devices::PhonePairingStatus;

use super::CA_UNUSABLE;
use super::CODE_TTL;
use super::DeviceError;
use super::PAIR_CODE;
use super::cert;
use super::pathutil;
use super::pathutil::OpenFile;
use super::random;

const RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct PhoneIssuer {
    pairing_dir: PathBuf,
    ca_dir: PathBuf,
    gateway_url: String,
}

impl PhoneIssuer {
    pub fn try_new(
        pairing_dir: PathBuf,
        ca_dir: PathBuf,
        gateway_url: String,
    ) -> Result<Self, super::SetupError> {
        pathutil::absolute_without_parents(&pairing_dir)?;
        pathutil::absolute_without_parents(&ca_dir)?;
        if gateway_url.is_empty() {
            return Err(super::SetupError::GatewayUrl);
        }
        Ok(Self {
            pairing_dir,
            ca_dir,
            gateway_url,
        })
    }
}

pub(super) fn availability(issuer: &PhoneIssuer) -> PhonePairingAvailability {
    if cert::authority_usable(&issuer.ca_dir) {
        PhonePairingAvailability {
            available: true,
            reason: None,
            gateway: Some(issuer.gateway_url.clone()),
        }
    } else {
        PhonePairingAvailability {
            available: false,
            reason: Some(CA_UNUSABLE.to_owned()),
            gateway: None,
        }
    }
}

pub(super) fn start(issuer: &PhoneIssuer, name: &str) -> Result<PhonePairingCode, DeviceError> {
    if !cert::authority_usable(&issuer.ca_dir) {
        return Err(DeviceError::unavailable(CA_UNUSABLE));
    }
    let device_id = random::device_id()?;
    let client = cert::issue_client(&issuer.ca_dir, &device_id)?;
    let token = random::token()?;
    let now = Utc::now();
    let expires_at = now
        .checked_add_signed(CODE_TTL)
        .ok_or(DeviceError::Internal)?;
    let record = PairingRecord {
        v: RECORD_VERSION,
        device_id: device_id.clone(),
        name: name.to_owned(),
        passphrase: client.passphrase.clone(),
        p12: cert::encode_p12(&client.p12_der),
        expires_at,
    };
    let pending = issuer.pairing_dir.join("pending");
    let certs = issuer.pairing_dir.join("certs");
    pathutil::ensure_dir(&pending)?;
    pathutil::ensure_dir(&certs)?;
    write_record(&pending, &token, &record)?;
    write_cert(&certs, &device_id, &client.p12_der)?;
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

pub(super) fn redeem(issuer: &PhoneIssuer, token: &str) -> Result<PairRedeemResponse, DeviceError> {
    let pending = issuer.pairing_dir.join("pending");
    let names = list_stems(&pending)?;
    let Some(stem) = select_token(token, &names) else {
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
    let dest = claimed.join(dest_name);
    std::fs::rename(&path, &dest).map_err(|error| pathutil::io_registry(&error))?;
    read_cert(issuer, &record)
}

fn read_cert(
    issuer: &PhoneIssuer,
    record: &PairingRecord,
) -> Result<PairRedeemResponse, DeviceError> {
    let certs = issuer.pairing_dir.join("certs");
    match pathutil::open_pairing_file(&certs, &record.device_id, "p12")? {
        OpenFile::Missing => Err(DeviceError::CertGone),
        OpenFile::Escape => {
            tracing::warn!("device certificate path leaves its directory");
            Err(DeviceError::Forbidden(PAIR_CODE))
        }
        OpenFile::Ready(path) => {
            let bytes = std::fs::read(&path).map_err(|error| pathutil::io_registry(&error))?;
            Ok(PairRedeemResponse {
                device_id: record.device_id.clone(),
                p12: cert::encode_p12(&bytes),
                passphrase: record.passphrase.clone(),
            })
        }
    }
}

fn state_of(
    name: &str,
    pending: &[PairingRecord],
    claimed: &[PairingRecord],
    now: DateTime<Utc>,
) -> PhonePairingState {
    if claimed
        .iter()
        .any(|record| record.name == name && record.expires_at > now)
    {
        PhonePairingState::Paired
    } else if pending
        .iter()
        .any(|record| record.name == name && record.expires_at > now)
    {
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

fn write_cert(dir: &Path, device_id: &str, bytes: &[u8]) -> Result<(), DeviceError> {
    let root = pathutil::ensure_dir(dir)?;
    let Some(name) = pathutil::pairing_file(device_id, "p12") else {
        return Err(DeviceError::Internal);
    };
    pathutil::write_atomic(&root.join(name), bytes)
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
