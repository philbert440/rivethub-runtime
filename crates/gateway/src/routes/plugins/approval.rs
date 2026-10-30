use crate::Gateway;
use crate::plugins::{HostError, PairedDevice};
use crate::tenancy::PeerCertificate;

pub(super) fn paired_device(
    gateway: &Gateway,
    peer: Option<&PeerCertificate>,
) -> Result<PairedDevice, HostError> {
    let Some(peer @ PeerCertificate::Device { .. }) = peer else {
        return Err(HostError::DeviceRequired);
    };
    let user = gateway
        .tenancy
        .device_user(peer)
        .map_err(|_| HostError::DeviceRequired)?;
    if gateway.tenancy.local_user() == Some(&user) {
        Ok(PairedDevice)
    } else {
        Err(HostError::DeviceRequired)
    }
}
