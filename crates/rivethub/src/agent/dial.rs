use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::path::PathBuf;

use config::Config;
use gateway::server::BindSource;
use gateway::server::is_bind_loopback;
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
    port: u16,
    ca: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum Class {
    V4,
    V6,
    Localhost,
    WildV4,
    WildV6,
}

pub(super) async fn endpoint(
    config: &Config,
    env: &config::Environment,
    node_name: &str,
) -> Result<GatewayEndpoint, AgentError> {
    let section = config.gateway.clone();
    let env = env.clone();
    let node_name = node_name.to_owned();
    let shared = config::shared_dir(&env);
    match tokio::task::spawn_blocking(move || {
        let source = BindSource::resolve_section(section.as_ref(), &env, &node_name, &shared);
        dial(&source)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(AgentError::Join(error)),
    }
}

fn dial(source: &BindSource) -> Result<GatewayEndpoint, AgentError> {
    let Some(cert) = present(source.tls_cert.clone()) else {
        return Ok(GatewayEndpoint {
            url: http_url(source)?,
            trust: None,
        });
    };
    load(&Files {
        cert,
        bind: source.host.clone(),
        port: source.port,
        ca: present(source.tls_ca.clone()),
    })
}

pub(super) fn choose(pem: &str, bind: &str) -> Result<String, AgentError> {
    dial_name(pem, classify(bind)?)
}

fn load(files: &Files) -> Result<GatewayEndpoint, AgentError> {
    let host = choose(&std::fs::read_to_string(&files.cert)?, &files.bind)?;
    let url = https_url(&host, files.port)?;
    let trust = match &files.ca {
        Some(path) => {
            let ca_pem = std::fs::read_to_string(path)?;
            Some(anchor(&ca_pem)?)
        }
        None => None,
    };
    Ok(GatewayEndpoint { url, trust })
}

fn anchor(ca_pem: &str) -> Result<tasks::MeshIdentity, AgentError> {
    tasks::MeshIdentity::from_ca(ca_pem).map_err(|_| AgentError::GatewayCertificate)
}

fn classify(bind: &str) -> Result<Class, AgentError> {
    let bare = strip_brackets(bind.trim());
    if let Some(ip) = ip_literal(bare)
        && ip.is_unspecified()
    {
        return Ok(if ip.is_ipv4() {
            Class::WildV4
        } else {
            Class::WildV6
        });
    }
    if is_bind_loopback(bare) {
        return Ok(loopback_class(bare));
    }
    Err(AgentError::GatewayBind {
        host: bind.trim().to_owned(),
    })
}

fn loopback_class(bare: &str) -> Class {
    if bare.eq_ignore_ascii_case("localhost") {
        Class::Localhost
    } else if bare.eq_ignore_ascii_case("::1") {
        Class::V6
    } else {
        Class::V4
    }
}

fn dial_name(pem: &str, class: Class) -> Result<String, AgentError> {
    let names = leaf_names(pem)?;
    let host = match class {
        Class::V4 => find_ip(&names, IpAddr::V4(Ipv4Addr::LOCALHOST)),
        Class::V6 => find_ip(&names, IpAddr::V6(Ipv6Addr::LOCALHOST)),
        Class::Localhost => find_dns(&names, "localhost"),
        Class::WildV4 => find_ip(&names, IpAddr::V4(Ipv4Addr::LOCALHOST))
            .or_else(|| find_dns(&names, "localhost")),
        Class::WildV6 => find_ip(&names, IpAddr::V6(Ipv6Addr::LOCALHOST))
            .or_else(|| find_dns(&names, "localhost")),
    };
    host.ok_or(AgentError::GatewayName)
}

fn plain_host(bind: &str) -> Result<String, AgentError> {
    Ok(match classify(bind)? {
        Class::V4 | Class::WildV4 => Ipv4Addr::LOCALHOST.to_string(),
        Class::V6 | Class::WildV6 => dial_form(&IpAddr::V6(Ipv6Addr::LOCALHOST)),
        Class::Localhost => "localhost".to_owned(),
    })
}

fn present(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|item| !item.as_os_str().is_empty() && item.is_file())
}

fn find_ip(names: &Names, ip: IpAddr) -> Option<String> {
    names.ips.contains(&ip).then(|| dial_form(&ip))
}

fn find_dns(names: &Names, host: &str) -> Option<String> {
    names
        .dns
        .iter()
        .find(|item| item.eq_ignore_ascii_case(host))
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
    with_host("http://127.0.0.1/", &plain_host(&source.host)?, source.port)
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
