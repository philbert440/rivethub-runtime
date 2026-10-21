use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use testkit::http::{RequestRecord, ResponseRecord};

use super::Error;

pub(super) const RECORDING: &str =
    include_str!("../../../../wire/tests/golden/gateway/http-tls.json");

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RunKind {
    #[serde(rename = "tls-main")]
    Main,
    #[serde(rename = "tls-optional")]
    Optional,
    #[serde(rename = "tls-tenancy")]
    Tenancy,
}

impl RunKind {
    pub(super) fn parse(name: &str) -> Result<Self, Error> {
        Ok(Self::deserialize(serde::de::value::StrDeserializer::<
            serde_json::Error,
        >::new(name))?)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ClientCertificate {
    DeviceTest,
    DeviceCnOnly,
    OuClient,
    Node,
    DeviceNodb,
    DeviceStranger,
    WrongCaDevice,
    DeviceServerauth,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub(super) enum Source {
    #[serde(rename = "loopback (source 127.0.0.1)")]
    Loopback,
    #[serde(rename = "remote (source 127.0.0.2)")]
    Remote,
}

#[derive(Debug, Deserialize)]
pub(super) struct Recording {
    pub(super) runs: BTreeMap<String, Run>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Run {
    pub(super) cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Case {
    pub(super) id: String,
    pub(super) request: Request,
    pub(super) response: ResponseRecord,
}

#[derive(Debug, Deserialize)]
pub(super) struct Request {
    pub(super) via: Source,
    #[serde(rename = "clientCert")]
    pub(super) certificate: Option<ClientCertificate>,
    #[serde(flatten)]
    pub(super) http: RequestRecord,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Skip {
    pub(super) id: String,
    pub(super) reason: String,
    pub(super) expected: ExpectedFailure,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum ExpectedFailure {
    Http { status: u16 },
    Tls { alert: CertificateAlert },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum CertificateAlert {
    UnknownCa,
    UnsupportedCertificate,
}

impl CertificateAlert {
    pub(super) fn alert(&self) -> rustls::AlertDescription {
        match self {
            Self::UnknownCa => rustls::AlertDescription::UnknownCA,
            Self::UnsupportedCertificate => rustls::AlertDescription::UnsupportedCertificate,
        }
    }
}

pub(super) fn recording() -> Result<Recording, Error> {
    Ok(serde_json::from_str(RECORDING)?)
}

pub(super) fn skips() -> Result<BTreeMap<String, Skip>, Error> {
    let entries: Vec<Skip> =
        serde_json::from_str(include_str!("../../fixtures/tls-replay/skips.json"))?;
    let mut skips = BTreeMap::new();
    for entry in entries {
        let id = entry.id.clone();
        if entry.reason.trim().is_empty() || skips.insert(id.clone(), entry).is_some() {
            return Err(Error::Fixture(format!("invalid or duplicate skip: {id}")));
        }
    }
    Ok(skips)
}

pub(super) fn inventory(
    recording: &Recording,
    skips: &BTreeMap<String, Skip>,
) -> Result<(), Error> {
    let mut ids = BTreeSet::new();
    for (run, entry) in &recording.runs {
        for case in &entry.cases {
            let id = format!("{run}/{}", case.id);
            if !ids.insert(id.clone()) {
                return Err(Error::Fixture(format!("duplicate case: {id}")));
            }
        }
    }
    for id in skips.keys() {
        if !ids.contains(id) {
            return Err(Error::Fixture(format!("skip outside TLS recording: {id}")));
        }
    }
    Ok(())
}
