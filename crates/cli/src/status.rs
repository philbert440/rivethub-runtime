use std::time::Duration;

use config::Environment;
use gateway::server::BindSource;
use serde::Deserialize;
use wire::gateway::HealthzResponse;

use crate::{Error, Report, configuration, process};

#[derive(Debug, Deserialize)]
struct Health {
    #[serde(flatten)]
    response: HealthzResponse,
    version: Option<String>,
    uptime: Option<f64>,
}

pub(crate) async fn run(env: &Environment) -> Report {
    match health(env).await {
        Ok(report) => report,
        Err(error) => {
            let mut report = match process::status(env).await {
                Ok(message) => Report::success(message),
                Err(pid_error) => {
                    let mut report = Report::success("RivetHub status is unavailable.\n");
                    report.stderr = format!("Error: {pid_error}\n");
                    report
                }
            };
            report.stderr = format!("{}Health unavailable: {error}\n", report.stderr);
            report
        }
    }
}

async fn health(env: &Environment) -> Result<Report, Error> {
    let (_, loaded) = configuration::load(env).await?;
    let hostname = host_name()?;
    let node = config::resolve_node_name(&loaded.config, env, &hostname)?;
    let section = loaded.config.gateway.clone();
    let tls_env = env.clone();
    let shared = config::shared_dir(&tls_env);
    let source = tokio::task::spawn_blocking(move || {
        BindSource::resolve_section(section.as_ref(), &tls_env, &node, &shared)
    })
    .await?;
    let mut url = url::Url::parse(if source.tls_cert.is_some() {
        "https://localhost/healthz"
    } else {
        "http://localhost/healthz"
    })?;
    let host = if source.host.contains(':') && !source.host.starts_with('[') {
        format!("[{}]", source.host)
    } else {
        source.host
    };
    url.set_host(Some(&host))?;
    match url.host() {
        Some(url::Host::Ipv4(address)) if address.is_unspecified() => {
            url.set_host(Some("127.0.0.1"))?;
        }
        Some(url::Host::Ipv6(address)) if address.is_unspecified() => {
            url.set_host(Some("[::1]"))?;
        }
        Some(url::Host::Domain(_) | url::Host::Ipv4(_) | url::Host::Ipv6(_)) => {}
        None => return Err(Error::Address),
    }
    url.set_port(Some(source.port))
        .map_err(|()| Error::Address)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()?;
    let health: Health = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let text = if health.response.ok {
        "RivetHub is running".to_owned()
    } else {
        "RivetHub is not running".to_owned()
    };
    let mut parts = vec![text];
    if let Some(version) = health.version {
        parts.push(format!(" (v{version})"));
    }
    if let Some(uptime) = health.uptime {
        parts.push(format!("; uptime {uptime}s"));
    }
    let mut report = Report::success(format!("{}\n", parts.join("")));
    report.stderr = configuration::notices(&loaded);
    Ok(report)
}

fn host_name() -> Result<String, Error> {
    let raw = nix::unistd::gethostname().map_err(|_| Error::HostName)?;
    let text = raw.into_string().map_err(|_| Error::HostName)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Ok(String::new())
    } else {
        Ok(trimmed.to_owned())
    }
}
