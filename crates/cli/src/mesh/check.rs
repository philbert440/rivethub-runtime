use std::net::Ipv4Addr;
use std::time::Duration;

use crate::{Error, Exit, Report};

pub(super) struct Target {
    pub(super) user: String,
    pub(super) host: String,
}

pub(super) fn usage(text: &'static str) -> Report {
    Report {
        stdout: String::new(),
        stdout_raw: Vec::new(),
        stderr: text.to_owned(),
        exit: Exit::Failure,
    }
}

pub(super) fn ssh_refusal(value: &str) -> Option<Report> {
    if valid_ssh(value) {
        None
    } else {
        Some(Report {
            stdout: String::new(),
            stdout_raw: Vec::new(),
            stderr: format!("❌ Refusing unsafe --ssh-user \"{value}\"\n"),
            exit: Exit::Failure,
        })
    }
}

pub(super) fn target(spec: &str) -> Result<Target, Error> {
    if shell_unsafe(spec) {
        return Err(Error::UnsafeTarget);
    }
    let Some((user, host)) = split_target(spec) else {
        return Err(Error::Target {
            spec: spec.to_owned(),
        });
    };
    if valid_user(user) && safe_host(host) {
        Ok(Target {
            user: user.to_owned(),
            host: host.to_owned(),
        })
    } else {
        Err(Error::Target {
            spec: spec.to_owned(),
        })
    }
}

pub(super) fn node_name(name: &str) -> Result<(), Error> {
    if valid_node_name(name) {
        Ok(())
    } else {
        Err(Error::NodeName {
            name: name.to_owned(),
        })
    }
}

pub(super) fn hub_cmd(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains('/')
        || shell_unsafe(value)
        || !value.bytes().all(hub_byte)
    {
        Err(Error::UnsafeHub)
    } else {
        Ok(())
    }
}

pub(super) fn advertise(explicit: Option<&str>) -> Result<String, Error> {
    match explicit {
        Some(value) if safe_host(value) => Ok(value.to_owned()),
        Some(_) => Err(Error::UnsafeAdvertise),
        None => {
            let host = host_name()?;
            if safe_host(&host) {
                Ok(host)
            } else {
                Err(Error::AdvertiseRequired)
            }
        }
    }
}

pub(super) fn safe_host(value: &str) -> bool {
    !value.is_empty() && !shell_unsafe(value) && (ipv4(value) || dns_name(value))
}

pub(super) fn host_name() -> Result<String, Error> {
    let raw = nix::unistd::gethostname().map_err(|_| Error::HostName)?;
    let text = raw.into_string().map_err(|_| Error::HostName)?;
    Ok(text.trim().to_owned())
}

pub(super) fn join_port(value: Option<u32>) -> Result<u16, Error> {
    let raw = value.unwrap_or(3000);
    if let Ok(port) = u16::try_from(raw)
        && port > 0
    {
        Ok(port)
    } else {
        Err(Error::MeshPort)
    }
}

pub(super) fn ping_timeout(value: Option<u64>) -> Result<Duration, Error> {
    match value {
        Some(0) => Err(Error::MeshTimeout),
        Some(millis) => Ok(Duration::from_millis(millis)),
        None => Ok(Duration::from_millis(5000)),
    }
}

fn split_target(spec: &str) -> Option<(&str, &str)> {
    let mut parts = spec.split('@');
    let user = parts.next()?;
    let host = parts.next()?;
    if parts.next().is_some() || user.is_empty() || host.is_empty() {
        None
    } else {
        Some((user, host))
    }
}

fn valid_node_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !edge_byte(first) {
        return false;
    }
    let mut last = first;
    let mut len = 1usize;
    for byte in bytes {
        if !body_byte(byte) {
            return false;
        }
        last = byte;
        len += 1;
        if len > 63 {
            return false;
        }
    }
    edge_byte(last)
}

fn valid_user(user: &str) -> bool {
    let mut bytes = user.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let mut len = 1usize;
    for byte in bytes {
        if !user_byte(byte) {
            return false;
        }
        len += 1;
        if len > 32 {
            return false;
        }
    }
    true
}

fn valid_ssh(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    let mut len = 1usize;
    for byte in bytes {
        if !(byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') {
            return false;
        }
        len += 1;
        if len > 32 {
            return false;
        }
    }
    true
}

fn shell_unsafe(value: &str) -> bool {
    value.bytes().any(|byte| {
        byte.is_ascii_whitespace()
            || matches!(
                byte,
                b';' | b'&' | b'|' | b'`' | b'$' | b'<' | b'>' | b'(' | b')' | b'\'' | b'"' | b'\\'
            )
    })
}

fn ipv4(value: &str) -> bool {
    value.parse::<Ipv4Addr>().is_ok() && value.split('.').count() == 4
}

fn dns_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || value.contains(':') {
        return false;
    }
    let mut any = false;
    for label in value.split('.') {
        if !dns_label(label) {
            return false;
        }
        any = true;
    }
    any
}

fn dns_label(label: &str) -> bool {
    let mut bytes = label.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let mut last = first;
    let mut len = 1usize;
    for byte in bytes {
        if !(byte.is_ascii_alphanumeric() || byte == b'-') {
            return false;
        }
        last = byte;
        len += 1;
        if len > 63 {
            return false;
        }
    }
    last.is_ascii_alphanumeric()
}

fn edge_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

fn body_byte(byte: u8) -> bool {
    edge_byte(byte) || byte == b'-'
}

fn user_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

fn hub_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
}
