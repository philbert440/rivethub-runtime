use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::path::PathBuf;

use config::Config;
use gateway::server::BindSource;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use x509_parser::certificate::X509Certificate;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::FromDer;

use super::assemble::AgentError;

#[derive(Debug)]
pub(super) struct GatewayEndpoint {
    pub(super) url: url::Url,
    pub(super) trust: Option<tasks::MeshIdentity>,
}

struct Names {
    dns: Vec<String>,
    ips: Vec<IpAddr>,
}

struct Files {
    cert: PathBuf,
    bind: String,
    advertise: Option<String>,
    port: u16,
    ca: Option<PathBuf>,
    client: Option<(PathBuf, PathBuf)>,
}

pub(super) async fn endpoint(config: &Config) -> Result<GatewayEndpoint, AgentError> {
    let source = BindSource::from_config(config);
    let Some(cert) = present(source.tls_cert.clone()) else {
        return Ok(GatewayEndpoint {
            url: http_url(&source)?,
            trust: None,
        });
    };
    let client = client_files(config)?;
    let ca = present(source.tls_ca);
    if client.is_some() && ca.is_none() {
        return Err(AgentError::GatewayCa);
    }
    let files = Files {
        cert,
        bind: source.host,
        advertise: advertise(config),
        port: source.port,
        ca,
        client,
    };
    match tokio::task::spawn_blocking(move || load(&files)).await {
        Ok(result) => result,
        Err(error) => Err(AgentError::Join(error)),
    }
}

pub(super) fn choose(pem: &str, bind: &str, advertise: Option<&str>) -> Result<String, AgentError> {
    let names = leaf_names(pem)?;
    if let Some(host) = advertise {
        return named(&names, host).ok_or(AgentError::AdvertiseHost);
    }
    if dialable_bind(bind)
        && let Some(host) = named(&names, bind)
    {
        return Ok(host);
    }
    sole(&names)
}

fn load(files: &Files) -> Result<GatewayEndpoint, AgentError> {
    let pem = std::fs::read_to_string(&files.cert)?;
    let host = choose(&pem, &files.bind, files.advertise.as_deref())?;
    let url = https_url(&host, files.port)?;
    let trust = match &files.ca {
        Some(path) => {
            let ca_pem = std::fs::read_to_string(path)?;
            Some(trust(&ca_pem, files.client.as_ref())?)
        }
        None => None,
    };
    Ok(GatewayEndpoint { url, trust })
}

fn trust(
    ca_pem: &str,
    client: Option<&(PathBuf, PathBuf)>,
) -> Result<tasks::MeshIdentity, AgentError> {
    let anchor =
        tasks::MeshIdentity::from_ca(ca_pem).map_err(|_| AgentError::GatewayCertificate)?;
    let Some((cert, key)) = client else {
        return Ok(anchor);
    };
    let certificate = std::fs::read_to_string(cert)?;
    let private_key = std::fs::read_to_string(key)?;
    Ok(tasks::MeshIdentity::from_pem(
        &certificate,
        &private_key,
        ca_pem,
    )?)
}

fn advertise(config: &Config) -> Option<String> {
    config
        .mesh
        .as_ref()
        .and_then(|mesh| mesh.advertise_host.clone())
}

fn client_files(config: &Config) -> Result<Option<(PathBuf, PathBuf)>, AgentError> {
    let Some(mesh) = config.mesh.as_ref() else {
        return Ok(None);
    };
    if mesh.enabled != Some(true) {
        return Ok(None);
    }
    match &mesh.tls {
        config::model::MeshTls::Enabled(true) => {
            let source = BindSource::from_config(config);
            Ok(Some((
                required(source.tls_cert)?,
                required(source.tls_key)?,
            )))
        }
        config::model::MeshTls::Enabled(false) => Err(tasks::Error::MeshTlsRequired.into()),
        config::model::MeshTls::Paths(paths) => Ok(Some((
            required(paths.cert_path.clone())?,
            required(paths.key_path.clone())?,
        ))),
    }
}

fn required(path: Option<PathBuf>) -> Result<PathBuf, AgentError> {
    path.filter(|item| !item.as_os_str().is_empty())
        .ok_or_else(|| tasks::Error::MeshTlsRequired.into())
}

fn present(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|item| !item.as_os_str().is_empty())
}

fn sole(names: &Names) -> Result<String, AgentError> {
    match (names.dns.as_slice(), names.ips.as_slice()) {
        ([host], _) => Ok(host.clone()),
        ([], [ip]) => Ok(dial_form(ip)),
        _ => Err(AgentError::GatewayName),
    }
}

fn named(names: &Names, host: &str) -> Option<String> {
    let bare = strip_brackets(host);
    if let Some(ip) = ip_literal(bare) {
        return names.ips.iter().find(|item| *item == &ip).map(dial_form);
    }
    names
        .dns
        .iter()
        .find(|item| item.eq_ignore_ascii_case(bare))
        .cloned()
}

fn leaf_names(pem: &str) -> Result<Names, AgentError> {
    let der = CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|_| AgentError::GatewayCertificate)?;
    let (rest, certificate) =
        X509Certificate::from_der(der.as_ref()).map_err(|_| AgentError::GatewayCertificate)?;
    if !rest.is_empty() {
        return Err(AgentError::GatewayCertificate);
    }
    let san = certificate
        .subject_alternative_name()
        .map_err(|_| AgentError::GatewayCertificate)?
        .ok_or(AgentError::GatewayName)?;
    collect(&san.value.general_names)
}

fn collect(generals: &[GeneralName<'_>]) -> Result<Names, AgentError> {
    let mut dns = Vec::new();
    let mut ips = Vec::new();
    for name in generals {
        match name {
            GeneralName::DNSName(value) => {
                if dialable_dns(value) {
                    dns.push((*value).to_owned());
                }
            }
            GeneralName::IPAddress(bytes) => {
                if let Some(ip) = san_ip(bytes)? {
                    ips.push(ip);
                }
            }
            GeneralName::URI(_)
            | GeneralName::OtherName(_, _)
            | GeneralName::RFC822Name(_)
            | GeneralName::X400Address(_)
            | GeneralName::DirectoryName(_)
            | GeneralName::EDIPartyName(_)
            | GeneralName::RegisteredID(_)
            | GeneralName::Invalid(_, _) => {}
        }
    }
    Ok(Names { dns, ips })
}

fn san_ip(bytes: &[u8]) -> Result<Option<IpAddr>, AgentError> {
    let ip = if let Ok(octets) = <[u8; 4]>::try_from(bytes) {
        IpAddr::V4(Ipv4Addr::from(octets))
    } else if let Ok(octets) = <[u8; 16]>::try_from(bytes) {
        IpAddr::V6(Ipv6Addr::from(octets))
    } else {
        return Err(AgentError::GatewayCertificate);
    };
    if ip.is_unspecified() {
        Ok(None)
    } else {
        Ok(Some(ip))
    }
}

fn dialable_bind(host: &str) -> bool {
    let bare = strip_brackets(host);
    if let Some(ip) = ip_literal(bare) {
        return !ip.is_unspecified();
    }
    dialable_dns(bare)
}

fn dialable_dns(name: &str) -> bool {
    !name.is_empty() && !name.as_bytes().contains(&b'*')
}

fn ip_literal(text: &str) -> Option<IpAddr> {
    if let Ok(address) = text.parse::<Ipv4Addr>() {
        return Some(IpAddr::V4(address));
    }
    if let Ok(address) = text.parse::<Ipv6Addr>() {
        return Some(IpAddr::V6(address));
    }
    None
}

fn dial_form(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => format!("[{address}]"),
    }
}

fn strip_brackets(host: &str) -> &str {
    match host.strip_prefix('[') {
        Some(rest) => match rest.strip_suffix(']') {
            Some(bare) => bare,
            None => host,
        },
        None => host,
    }
}

fn https_url(host: &str, port: u16) -> Result<url::Url, AgentError> {
    with_host("https://127.0.0.1/", host, port)
}

fn http_url(source: &BindSource) -> Result<url::Url, AgentError> {
    with_host(
        "http://127.0.0.1/",
        strip_brackets(&source.host),
        source.port,
    )
}

fn with_host(base: &str, host: &str, port: u16) -> Result<url::Url, AgentError> {
    let mut url = url::Url::parse(base).map_err(|_| AgentError::GatewayUrl)?;
    url.set_host(Some(host))
        .map_err(|_| AgentError::GatewayUrl)?;
    url.set_port(Some(port))
        .map_err(|()| AgentError::GatewayUrl)?;
    url.set_path("/");
    if url.host_str().is_none() {
        return Err(AgentError::GatewayUrl);
    }
    Ok(url)
}

#[cfg(test)]
#[path = "dial_test.rs"]
mod tests;
