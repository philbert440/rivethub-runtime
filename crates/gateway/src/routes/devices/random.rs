use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::DeviceError;

struct EnrollmentToken([u8; 24]);
struct DeviceIdBytes([u8; 16]);
struct PassphraseBytes([u8; 18]);

pub(super) fn token() -> Result<String, DeviceError> {
    Ok(URL_SAFE_NO_PAD.encode(EnrollmentToken::generate()?.0))
}

pub(super) fn device_id() -> Result<String, DeviceError> {
    Ok(format!("d{}", hex::encode(DeviceIdBytes::generate()?.0)))
}

pub(super) fn passphrase() -> Result<String, DeviceError> {
    Ok(URL_SAFE_NO_PAD.encode(PassphraseBytes::generate()?.0))
}

fn draw<const N: usize>() -> Result<[u8; N], DeviceError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|error| {
        tracing::warn!(%error, "device secret generation failed");
        DeviceError::Internal
    })?;
    Ok(bytes)
}

impl EnrollmentToken {
    fn generate() -> Result<Self, DeviceError> {
        Ok(Self(draw()?))
    }
}

impl DeviceIdBytes {
    fn generate() -> Result<Self, DeviceError> {
        Ok(Self(draw()?))
    }
}

impl PassphraseBytes {
    fn generate() -> Result<Self, DeviceError> {
        Ok(Self(draw()?))
    }
}
