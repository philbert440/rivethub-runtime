use std::sync::Arc;

use config::{Config, Environment, Paths};
use gateway::Gateway;
use gateway::routes::devices::{DeviceRuntime, PhoneIssuer};
use gateway::tenancy::UserId;
use kernel::ca::CertificateAuthority;
use pki::Settings;
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
    let Some(authority) = open_authority(config, env, shutdown).await? else {
        tracing::info!("no certificate authority configured");
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
    let pairing = Paths::new(env)?.state_dir().join("phone-pairing");
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
    config: &Config,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Option<Arc<dyn CertificateAuthority>>, ComposeError> {
    let Some(section) = config
        .gateway
        .as_ref()
        .and_then(|gateway| gateway.certificate_authority.clone())
    else {
        return Ok(None);
    };
    let url = section
        .url
        .ok_or_else(|| ComposeError::Invalid("certificate authority url is required".to_owned()))?;
    let root = section.root.ok_or_else(|| {
        ComposeError::Invalid("certificate authority root is required".to_owned())
    })?;
    let password_file = section.provisioner_password_file.ok_or_else(|| {
        ComposeError::Invalid(
            "certificate authority provisioner password file is required".to_owned(),
        )
    })?;
    let url = url::Url::parse(&url).map_err(|error| ComposeError::Invalid(error.to_string()))?;
    let authority = pki::Authority::connect(
        Settings {
            url,
            root,
            password_file,
            roster: section.roster,
            environment: pki::ProcessEnv::from_pairs(
                env.iter().map(|(key, value)| (key.clone(), value.clone())),
            ),
        },
        shutdown.child_token(),
    )
    .await?;
    Ok(Some(Arc::new(authority)))
}
