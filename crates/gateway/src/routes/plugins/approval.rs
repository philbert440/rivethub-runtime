use crate::Gateway;
use crate::tenancy::PeerCertificate;

use super::PluginRouteError;

pub(super) fn paired_device(
    gateway: &Gateway,
    peer: Option<&PeerCertificate>,
) -> Result<(), PluginRouteError> {
    let Some(peer @ PeerCertificate::Device { .. }) = peer else {
        return Err(PluginRouteError::DeviceRequired);
    };
    let user = gateway
        .tenancy
        .device_user(peer)
        .map_err(|_| PluginRouteError::DeviceRequired)?;
    if gateway.tenancy.local_user() == Some(&user) {
        Ok(())
    } else {
        Err(PluginRouteError::DeviceRequired)
    }
}
