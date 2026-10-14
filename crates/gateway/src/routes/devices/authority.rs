use std::sync::Arc;

use kernel::ca::CaError;
use kernel::ca::CertificateAuthority;
use kernel::ca::CsrPem;
use kernel::ca::EnrollmentToken;
use kernel::ca::Principal;
use kernel::ca::SignAuthority;
use wire::gateway::devices::SignedCertificate;

use super::CA_REFUSED;
use super::CA_UNUSABLE;
use super::CSR_REJECTED;
use super::DeviceError;

pub(super) struct SharedAuthority(Arc<dyn CertificateAuthority>);

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "the authority is a trait object, and Debug must not print it"
    )
)]
impl std::fmt::Debug for SharedAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SharedAuthority")
    }
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "derive(Clone) would require the trait object to be Clone; Arc is cloned instead"
    )
)]
impl Clone for SharedAuthority {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl SharedAuthority {
    pub(super) fn new(authority: Arc<dyn CertificateAuthority>) -> Self {
        Self(authority)
    }

    pub(super) fn get(&self) -> &dyn CertificateAuthority {
        self.0.as_ref()
    }
}

pub(super) async fn issue_device_token(
    authority: &dyn CertificateAuthority,
    user: &str,
    device: &str,
) -> Result<String, DeviceError> {
    let issued = authority
        .issue_token(Principal::Device {
            user: user.to_owned(),
            device: device.to_owned(),
        })
        .await
        .map_err(map_ca)?;
    if issued.0.is_empty() {
        tracing::warn!("certificate authority issued an empty enrollment token");
        return Err(DeviceError::unavailable(CA_UNUSABLE));
    }
    Ok(issued.0)
}

pub(super) async fn sign_device(
    authority: &dyn CertificateAuthority,
    csr: &str,
    token: &str,
    user: &str,
    device: &str,
) -> Result<SignedCertificate, DeviceError> {
    let signed = authority
        .sign(
            CsrPem(csr.to_owned()),
            SignAuthority::Token(EnrollmentToken(token.to_owned())),
        )
        .await
        .map_err(map_ca)?;
    let published = match authority.chain().await {
        Ok(chain) => chain,
        Err(error) => {
            let mapped = map_ca(error);
            revoke_device(authority, user, device).await?;
            return Err(mapped);
        }
    };
    if signed.leaf.is_empty() || !chain_usable(&published) || signed.chain != published {
        tracing::warn!(
            "certificate authority returned a chain that does not match the signed leaf"
        );
        revoke_device(authority, user, device).await?;
        return Err(DeviceError::unavailable(CA_UNUSABLE));
    }
    Ok(SignedCertificate {
        leaf: signed.leaf,
        chain: published,
    })
}

pub(super) async fn chain_ready(authority: &dyn CertificateAuthority) -> Result<(), DeviceError> {
    let published = authority.chain().await.map_err(map_ca)?;
    if chain_usable(&published) {
        Ok(())
    } else {
        tracing::warn!("certificate authority published an empty chain");
        Err(DeviceError::unavailable(CA_UNUSABLE))
    }
}

pub(super) async fn revoke_device(
    authority: &dyn CertificateAuthority,
    user: &str,
    device: &str,
) -> Result<(), DeviceError> {
    authority
        .revoke(Principal::Device {
            user: user.to_owned(),
            device: device.to_owned(),
        })
        .await
        .map_err(map_ca)
}

fn chain_usable(chain: &[String]) -> bool {
    !chain.is_empty() && chain.iter().all(|pem| !pem.is_empty())
}

fn map_ca(error: CaError) -> DeviceError {
    match error {
        CaError::Refused(reason) => {
            tracing::warn!(%reason, "certificate authority refused a device request");
            DeviceError::Forbidden(CA_REFUSED)
        }
        CaError::InvalidCsr(reason) => {
            tracing::warn!(%reason, "certificate authority rejected a device csr");
            DeviceError::BadRequest(CSR_REJECTED)
        }
        CaError::Unavailable(source) => {
            tracing::warn!(%source, "certificate authority is unavailable");
            DeviceError::Unavailable {
                message: CA_UNUSABLE,
                source: Some(source),
            }
        }
    }
}
