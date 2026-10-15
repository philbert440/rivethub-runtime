use std::str::FromStr;

use kernel::ca::Principal;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

use super::PeerCertificate;

#[derive(Debug, thiserror::Error)]
enum IdentityError {
    #[error("invalid certificate identity: {0}")]
    Invalid(String),
}

impl PeerCertificate {
    pub(crate) fn from_verified_leaf(der: &[u8]) -> Self {
        match identity(der) {
            Ok(identity) => identity,
            Err(error) => {
                tracing::warn!("verified certificate refused: {error}");
                Self::Unrecognized
            }
        }
    }
}

fn identity(der: &[u8]) -> Result<PeerCertificate, IdentityError> {
    let (remaining, leaf) = X509Certificate::from_der(der)
        .map_err(|error| IdentityError::Invalid(error.to_string()))?;
    if !remaining.is_empty() {
        return Err(IdentityError::Invalid("trailing certificate data".into()));
    }
    let san = leaf
        .subject_alternative_name()
        .map_err(|error| IdentityError::Invalid(error.to_string()))?;
    if let Some(san) = san {
        let uris: Vec<_> = san
            .value
            .general_names
            .iter()
            .filter_map(|name| {
                if let GeneralName::URI(uri) = name {
                    Some(*uri)
                } else {
                    None
                }
            })
            .collect();
        if !uris.is_empty() {
            return match uris.as_slice() {
                [uri] => spiffe(uri),
                _ => Err(IdentityError::Invalid("ambiguous URI identity".into())),
            };
        }
    }
    let common_names: Vec<_> = leaf.subject().iter_common_name().collect();
    let [common_name] = common_names.as_slice() else {
        return Err(IdentityError::Invalid(
            "expected one device common name".into(),
        ));
    };
    let name = common_name
        .as_str()
        .map_err(|error| IdentityError::Invalid(error.to_string()))?;
    let device = if let Some(device) = name.strip_prefix("device:") {
        device
    } else {
        let units: Vec<_> = leaf.subject().iter_organizational_unit().collect();
        let [unit] = units.as_slice() else {
            return Err(IdentityError::Invalid("not a legacy device".into()));
        };
        if unit
            .as_str()
            .map_err(|error| IdentityError::Invalid(error.to_string()))?
            != "client"
        {
            return Err(IdentityError::Invalid("not a legacy device".into()));
        }
        name
    };
    if device.is_empty() || device.chars().any(char::is_control) {
        return Err(IdentityError::Invalid("invalid legacy device name".into()));
    }
    tracing::warn!(
        device,
        "deprecated legacy device certificate; enroll a SPIFFE device identity"
    );
    Ok(PeerCertificate::Device {
        user: None,
        device: device.to_owned(),
    })
}

fn spiffe(uri: &str) -> Result<PeerCertificate, IdentityError> {
    let principal =
        Principal::from_str(uri).map_err(|error| IdentityError::Invalid(error.to_string()))?;
    Ok(match principal {
        Principal::Node { node } => PeerCertificate::Node { node },
        Principal::Agent { node, agent } => PeerCertificate::Agent { node, agent },
        Principal::Device { user, device } => PeerCertificate::Device {
            user: Some(user),
            device,
        },
    })
}
