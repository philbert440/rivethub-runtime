use std::sync::Arc;

use config::{Config, Environment, Paths};
use gateway::Gateway;
use gateway::routes::devices::{DeviceRuntime, PhoneIssuer};
use gateway::tenancy::UserId;
use kernel::ca::CertificateAuthority;
use tokio_util::sync::CancellationToken;

use super::ComposeError;

pub(super) async fn attach(
    gateway: Gateway,
    config: &Config,
    env: &Environment,
    owner: &UserId,
    shutdown: &CancellationToken,
) -> Result<Gateway, ComposeError> {
    let devices = phone_devices(config, env, owner)?;
    let gateway = gateway.with_devices(devices);
    let Some(authority) = open_authority(env, shutdown).await? else {
        tracing::info!("no certificate authority on this node (rh ca init creates one)");
        return Ok(gateway);
    };
    Ok(gateway.with_certificate_authority(authority))
}

fn phone_devices(
    config: &Config,
    env: &Environment,
    owner: &UserId,
) -> Result<DeviceRuntime, ComposeError> {
    let Some(url) = gateway_url(config) else {
        return Ok(DeviceRuntime::unconfigured());
    };
    let pairing = Paths::new(env).state_dir().join("phone-pairing");
    let issuer = PhoneIssuer::try_new(pairing, url, owner.as_str().to_owned())?;
    Ok(DeviceRuntime::unconfigured().enable_phone_issuer(issuer))
}

fn gateway_url(config: &Config) -> Option<String> {
    config
        .gateway
        .as_ref()
        .and_then(|section| section.devices.gateway_url.clone())
}

async fn open_authority(
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Option<Arc<dyn CertificateAuthority>>, ComposeError> {
    let directory = Paths::new(env).ca_dir();
    if !tokio::fs::try_exists(&directory).await? {
        return Ok(None);
    }
    let opened = pki::Pki::open(directory, shutdown.child_token()).await?;
    Ok(Some(Arc::new(opened)))
}
