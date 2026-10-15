use std::fmt;

use serde::Deserialize;
use serde::Serialize;

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub enum TlsVersion {
    V12,
    V13,
}

#[derive(Debug)]
pub struct VersionRejected;

impl fmt::Display for VersionRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unsupported TLS version")
    }
}

impl TryFrom<f64> for TlsVersion {
    type Error = VersionRejected;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if (value - 1.2).abs() < 1e-9 {
            Ok(Self::V12)
        } else if (value - 1.3).abs() < 1e-9 {
            Ok(Self::V13)
        } else {
            Err(VersionRejected)
        }
    }
}

impl From<TlsVersion> for f64 {
    fn from(value: TlsVersion) -> Self {
        match value {
            TlsVersion::V12 => 1.2,
            TlsVersion::V13 => 1.3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CipherSuite {
    #[serde(rename = "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256")]
    Chacha20Poly1305,
    #[serde(rename = "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256")]
    Aes128Gcm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TlsOptions {
    pub cipher_suites: Vec<CipherSuite>,
    pub min_version: TlsVersion,
    pub max_version: TlsVersion,
    pub renegotiation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCertificate {
    pub leaf: String,
    pub intermediate: String,
    pub chain: Vec<String>,
    pub tls: TlsOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct SignResponse {
    pub(crate) crt: String,
    pub(crate) ca: String,
    pub(crate) cert_chain: Vec<String>,
    pub(crate) tls_options: TlsOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevocationCause {
    #[serde(rename = "unspecified")]
    Unspecified,
    #[serde(rename = "keyCompromise")]
    KeyCompromise,
    #[serde(rename = "cACompromise")]
    CaCompromise,
    #[serde(rename = "affiliationChanged")]
    AffiliationChanged,
    #[serde(rename = "superseded")]
    Superseded,
    #[serde(rename = "cessationOfOperation")]
    CessationOfOperation,
    #[serde(rename = "certificateHold")]
    CertificateHold,
    #[serde(rename = "removeFromCRL")]
    RemoveFromCrl,
    #[serde(rename = "privilegeWithdrawn")]
    PrivilegeWithdrawn,
    #[serde(rename = "aACompromise")]
    AaCompromise,
}

impl RevocationCause {
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Unspecified => 0,
            Self::KeyCompromise => 1,
            Self::CaCompromise => 2,
            Self::AffiliationChanged => 3,
            Self::Superseded => 4,
            Self::CessationOfOperation => 5,
            Self::CertificateHold => 6,
            Self::RemoveFromCrl => 8,
            Self::PrivilegeWithdrawn => 9,
            Self::AaCompromise => 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Serial(String);

impl Serial {
    pub fn parse(value: &str) -> Result<Self, Error> {
        crate::verify::serial(value).map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    pub serial: Serial,
    pub reason: RevocationCause,
    pub passive: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct SignBody<'a> {
    pub(crate) csr: &'a str,
    pub(crate) ott: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RevokeBody<'a> {
    pub(crate) serial: &'a str,
    pub(crate) reason_code: u8,
    pub(crate) reason: RevocationCause,
    pub(crate) passive: bool,
}
