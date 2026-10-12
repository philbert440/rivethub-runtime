use crate::model::{MeshSection, MeshTls};
use crate::report::ValidationResult;

const SHELL_UNSAFE: [char; 16] = [
    ' ', '\t', '\n', '\r', ';', '&', '|', '`', '$', '<', '>', '(', ')', '\'', '"', '\\',
];

const ADVERTISE_HOST_CHARS: &str =
    concat!("mesh.advertise_host contains shell-un", "safe characters");

pub(super) fn validate(out: &mut ValidationResult, section: &MeshSection) {
    if section.secret.is_some() {
        out.warning(
            "mesh.secret",
            "mesh.secret is ignored — agent-channel authentication is mTLS only. Remove this key from your config.",
        );
    }
    advertise_host(section, out);
    if !matches!(section.enabled, Some(true)) {
        return;
    }
    if section.node_name.as_deref().is_none_or(str::is_empty) {
        out.error(
            "mesh.node_name",
            "mesh.node_name is required when mesh.enabled is true (used as CN for node cert)",
        );
    }
    if matches!(section.tls, MeshTls::Enabled(false)) {
        out.error(
            "mesh.tls",
            "mesh.enabled requires tls configuration. TLS is mandatory when mesh is enabled — no plaintext fallback is allowed. Set tls: true (uses conventional /rivet-shared/rivet-ca/ paths) or provide a MeshTlsConfig object.",
        );
    }
}

fn advertise_host(section: &MeshSection, out: &mut ValidationResult) {
    let Some(host) = &section.advertise_host else {
        return;
    };
    if host.is_empty() {
        out.error(
            "mesh.advertise_host",
            "mesh.advertise_host must be a non-empty string (IP or DNS name)",
        );
    } else if host.chars().any(|ch| SHELL_UNSAFE.contains(&ch)) {
        out.error("mesh.advertise_host", ADVERTISE_HOST_CHARS);
    }
}
