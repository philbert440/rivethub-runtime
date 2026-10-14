use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use uuid::Uuid;

use super::DeviceError;

const TOKEN_BYTES: usize = 24;
const ID_BYTES: usize = 16;
const PASSPHRASE_BYTES: usize = 18;

pub(super) fn token() -> Result<String, DeviceError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes(TOKEN_BYTES)?))
}

pub(super) fn device_id() -> Result<String, DeviceError> {
    Ok(format!("d{}", hex::encode(random_bytes(ID_BYTES)?)))
}

pub(super) fn passphrase() -> Result<String, DeviceError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes(PASSPHRASE_BYTES)?))
}

fn random_bytes(len: usize) -> Result<Vec<u8>, DeviceError> {
    let mut out = Vec::new();
    while out.len() < len {
        let block = Uuid::new_v4().into_bytes();
        let take = len.saturating_sub(out.len()).min(block.len());
        let Some(chunk) = block.get(..take) else {
            return Err(DeviceError::Internal);
        };
        out.extend_from_slice(chunk);
    }
    Ok(out)
}
