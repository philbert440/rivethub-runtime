use crate::enums::{HarnessId, listed};
use crate::model::{GatewaySection, HarnessSlot};
use crate::report::ValidationResult;

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}

pub(super) fn validate(out: &mut ValidationResult, section: &GatewaySection) {
    if section
        .host
        .as_deref()
        .is_some_and(|host| host.trim().is_empty())
    {
        out.error(
            "den.host",
            "\"den.host\" must be a non-empty string (bind address, e.g. 127.0.0.1 or 0.0.0.0)",
        );
    }
    if section.port == 0 {
        out.error(
            "den.port",
            "\"den.port\" must be an integer between 1 and 65535",
        );
    }
    if let Some(static_dir) = &section.static_dir
        && static_dir.as_os_str().is_empty()
    {
        out.error(
            "den.static_dir",
            "\"den.static_dir\" must be a non-empty string path",
        );
    }
    for (path, entries) in [
        ("den.allowed_origins", &section.allowed_origins),
        ("den.allowed_hosts", &section.allowed_hosts),
    ] {
        if entries.iter().any(|entry| entry.trim().is_empty()) {
            out.error(
                path,
                format!("\"{path}\" must be a list of non-empty strings"),
            );
        }
    }
    allowed_harnesses(out, section);
    exposed_terminal_gate(section, out);
}

fn allowed_harnesses(out: &mut ValidationResult, section: &GatewaySection) {
    let Some(entries) = &section.allowed_harnesses else {
        return;
    };
    let mut blank_entry = false;
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            HarnessSlot::Known(_) => {}
            HarnessSlot::Unknown(id) if id.trim().is_empty() => blank_entry = true,
            HarnessSlot::Unknown(id) => out.warning(
                format!("den.allowed_harnesses[{index}]"),
                format!(
                    "Unknown harness id \"{id}\" — expected one of: {}",
                    listed::<HarnessId>()
                ),
            ),
        }
    }
    if blank_entry {
        out.error(
            "den.allowed_harnesses",
            "\"den.allowed_harnesses\" must be a list of non-empty harness ids (omit the key to allow all)",
        );
    }
}

fn exposed_terminal_gate(section: &GatewaySection, out: &mut ValidationResult) {
    if !matches!(section.enabled, Some(true)) {
        return;
    }
    let Some(terminal) = &section.terminal else {
        return;
    };
    if !matches!(terminal.enabled, Some(true)) {
        return;
    }
    if matches!(terminal.open, Some(true)) {
        return;
    }
    let certified = section
        .tls_cert
        .as_deref()
        .is_some_and(|cert| !cert.is_empty());
    if certified {
        return;
    }
    let loopback = section.host.as_deref().is_none_or(is_loopback);
    if loopback {
        return;
    }
    out.error(
        "den.tls_cert",
        "\"den.tls_cert\" (and den.tls_key) is required when den.terminal.enabled is true and den.host is not loopback (127.0.0.1/::1/localhost) — an exposed terminal without TLS would hang an unauthenticated shell on the network. Set den.tls_cert/tls_key, bind den.host to loopback, disable terminals, or set den.terminal.open: true on a trusted private network.",
    );
}
