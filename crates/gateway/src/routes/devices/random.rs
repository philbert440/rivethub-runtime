use super::DeviceError;

struct DeviceIdBytes([u8; 16]);

pub(super) fn device_id() -> Result<String, DeviceError> {
    Ok(format!("d{}", hex::encode(DeviceIdBytes::generate()?.0)))
}

fn draw<const N: usize>() -> Result<[u8; N], DeviceError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|error| {
        tracing::warn!(%error, "device secret generation failed");
        DeviceError::Internal
    })?;
    Ok(bytes)
}

impl DeviceIdBytes {
    fn generate() -> Result<Self, DeviceError> {
        Ok(Self(draw()?))
    }
}
