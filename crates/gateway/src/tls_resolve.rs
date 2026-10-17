use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::server::BindSource;

const LOOPBACK_HOST: &str = "127.0.0.1";
const DEFAULT_GATEWAY_PORT: u16 = 5174;

impl BindSource {
    #[must_use]
    pub fn resolve(
        config: &config::Config,
        env: &config::Environment,
        node_name: &str,
        shared_dir: &Path,
    ) -> Self {
        Self::resolve_section(config.gateway.as_ref(), env, node_name, shared_dir)
    }

    #[must_use]
    pub fn resolve_section(
        section: Option<&config::model::GatewaySection>,
        env: &config::Environment,
        node_name: &str,
        shared_dir: &Path,
    ) -> Self {
        let mut notices = Vec::new();
        let mut cert = configured_text(section.and_then(|section| section.tls_cert.as_deref()))
            .or_else(|| {
                env_path(
                    env,
                    "RIVETHUB_TLS_CERT",
                    "RIVETOS_DEN_TLS_CERT",
                    &mut notices,
                )
            });
        let mut key = configured_text(section.and_then(|section| section.tls_key.as_deref()))
            .or_else(|| env_path(env, "RIVETHUB_TLS_KEY", "RIVETOS_DEN_TLS_KEY", &mut notices));
        if (cert.is_none() || key.is_none())
            && let Some((issued_cert, issued_key)) = issued_pair(shared_dir, node_name)
        {
            if cert.is_none() {
                cert = Some(issued_cert);
            }
            if key.is_none() {
                key = Some(issued_key);
            }
        }
        let tls_ca = if cert.is_some() || key.is_some() {
            Some(resolve_ca(section, env, shared_dir, &mut notices))
        } else {
            None
        };
        let require_client = require_client(section, env, &mut notices);
        for notice in &notices {
            tracing::warn!("{notice}");
        }
        Self {
            host: host_of(section),
            port: port_of(section),
            tls_cert: cert,
            tls_key: key,
            tls_ca,
            tls_crl: section.and_then(|section| configured_path(section.tls_crl.as_ref())),
            require_client,
            notices,
        }
    }
}

fn host_of(section: Option<&config::model::GatewaySection>) -> String {
    match section.and_then(|section| section.host.clone()) {
        Some(host) => host,
        None => LOOPBACK_HOST.to_owned(),
    }
}

fn port_of(section: Option<&config::model::GatewaySection>) -> u16 {
    match section {
        Some(section) => section.port,
        None => DEFAULT_GATEWAY_PORT,
    }
}

fn require_client(
    section: Option<&config::model::GatewaySection>,
    env: &config::Environment,
    notices: &mut Vec<String>,
) -> bool {
    let configured_off = section.is_some_and(|section| section.tls_require_client == Some(false));
    let variable = env_value(
        env,
        "RIVETHUB_TLS_REQUIRE_CLIENT",
        "RIVETOS_DEN_TLS_REQUIRE_CLIENT",
        notices,
    );
    let environment_off = variable.as_deref().is_some_and(requirement_disabled);
    !configured_off && !environment_off
}

fn requirement_disabled(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "0" | "off" | "false" | "no"
    )
}

pub(crate) fn mesh_ca(
    section: Option<&config::model::GatewaySection>,
    env: &config::Environment,
    shared_dir: &Path,
) -> Option<PathBuf> {
    let path = resolve_ca(section, env, shared_dir, &mut Vec::new());
    path.is_file().then_some(path)
}

fn resolve_ca(
    section: Option<&config::model::GatewaySection>,
    env: &config::Environment,
    shared_dir: &Path,
    notices: &mut Vec<String>,
) -> PathBuf {
    if let Some(path) = section.and_then(|section| configured_path(section.tls_ca.as_ref())) {
        return path;
    }
    if let Some(path) = env_path(env, "RIVETHUB_TLS_CA", "RIVETOS_DEN_TLS_CA", notices) {
        return path;
    }
    if let Some(path) = trimmed(env.text("RIVET_CA_CHAIN")) {
        return PathBuf::from(path);
    }
    shared_dir
        .join("rivet-ca")
        .join("intermediate")
        .join("chain.pem")
}

fn configured_text(value: Option<&str>) -> Option<PathBuf> {
    trimmed(value).map(PathBuf::from)
}

fn configured_path(value: Option<&PathBuf>) -> Option<PathBuf> {
    let path = value?;
    match path.to_str() {
        Some(text) => trimmed(Some(text)).map(PathBuf::from),
        None if path.as_os_str().is_empty() => None,
        None => Some(path.clone()),
    }
}

fn env_path(
    env: &config::Environment,
    current: &str,
    legacy: &str,
    notices: &mut Vec<String>,
) -> Option<PathBuf> {
    env_value(env, current, legacy, notices).map(PathBuf::from)
}

fn env_value(
    env: &config::Environment,
    current: &str,
    legacy: &str,
    notices: &mut Vec<String>,
) -> Option<String> {
    if let Some(value) = trimmed(env.text(current)) {
        return Some(value.to_owned());
    }
    if let Some(notice) = env.legacy_notice(current, legacy) {
        notices.push(notice);
    }
    trimmed(env.text(legacy)).map(str::to_owned)
}

fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

fn issued_pair(shared_dir: &Path, node_name: &str) -> Option<(PathBuf, PathBuf)> {
    let name = single_component(node_name)?;
    let root = shared_dir.join("rivet-ca").join("issued");
    let cert = contained_file(&root, &root.join(format!("{name}.crt")))?;
    let key = contained_file(&root, &root.join(format!("{name}.key")))?;
    Some((cert, key))
}

fn single_component(name: &str) -> Option<&str> {
    if name.is_empty() || name.contains(['/', '\\', '\0']) {
        return None;
    }
    let mut components = Path::new(name).components();
    match components.next() {
        Some(Component::Normal(part))
            if components.next().is_none() && part == OsStr::new(name) =>
        {
            Some(name)
        }
        Some(_) | None => None,
    }
}

fn contained_file(root: &Path, candidate: &Path) -> Option<PathBuf> {
    let root = canonical_directory(root)?;
    let file = canonical_file(candidate)?;
    let normalized_root = config::normalize(&root);
    let normalized_file = config::normalize(&file);
    if normalized_file.starts_with(&normalized_root) {
        Some(file)
    } else {
        None
    }
}

fn canonical_directory(path: &Path) -> Option<PathBuf> {
    let found = canonicalize(path)?;
    if found.is_dir() { Some(found) } else { None }
}

fn canonical_file(path: &Path) -> Option<PathBuf> {
    let found = canonicalize(path)?;
    if found.is_file() { Some(found) } else { None }
}

fn canonicalize(path: &Path) -> Option<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(found) => Some(found),
        Err(error) => unavailable(&error),
    }
}

fn unavailable(error: &std::io::Error) -> Option<PathBuf> {
    tracing::debug!(%error, "shared issued material is not available");
    None
}
