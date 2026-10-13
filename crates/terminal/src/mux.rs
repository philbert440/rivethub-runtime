use std::path::Path;
use std::path::PathBuf;

use crate::error::Error;
use crate::herdr::ProbeFailure;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrSetup {
    pub binary: PathBuf,
    pub config_home: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnavailableReason {
    Probe(ProbeFailure),
    SocketPath { len: usize, path: PathBuf },
    Init { message: String },
}

impl UnavailableReason {
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Probe(ProbeFailure::NotFound) => {
                "herdr not found on PATH or in ~/.local/bin".to_owned()
            }
            Self::Probe(ProbeFailure::Version { binary, .. }) => format!(
                "herdr at {} is not {} (den pins {} / protocol 20)",
                binary.display(),
                crate::herdr::PINNED_VERSION,
                crate::herdr::PINNED_VERSION
            ),
            Self::SocketPath { len, path } => {
                format!("socket path too long ({len} > 107): {}", path.display())
            }
            Self::Init { message } => format!("failed to init herdr ({message})"),
        }
    }

    pub fn to_error(&self) -> Error {
        match self {
            Self::Probe(failure) => Error::from(failure),
            Self::SocketPath { len, path } => Error::SocketPathTooLong {
                len: *len,
                path: Box::from(path.as_path()),
            },
            Self::Init { message } => Error::HerdrInit {
                message: message.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrSetupUnavailable {
    pub reason: UnavailableReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxPlan {
    Herdr(HerdrSetup),
    Direct,
    Unavailable(HerdrSetupUnavailable),
}

pub async fn resolve(
    choice: config::TermMuxChoice,
    paths: &[PathBuf],
    home: Option<&Path>,
    state_dir: &Path,
    port: u16,
) -> MuxPlan {
    if let Some(value) = &choice.ignored {
        tracing::warn!(
            "config: RIVETHUB_TERM_MUX=\"{value}\" is not 'herdr' or 'none' — using direct PTYs"
        );
    }
    match choice.setting {
        config::TermMuxSetting::Direct => MuxPlan::Direct,
        config::TermMuxSetting::Auto => match crate::herdr::probe(paths, home).await {
            Ok(binary) => herdr_plan(binary, state_dir, port),
            Err(reason) => {
                let reason = UnavailableReason::Probe(reason);
                tracing::warn!(
                    "terminal mux: {} — falling back to direct PTY (sessions will not survive restarts)",
                    reason.text()
                );
                MuxPlan::Direct
            }
        },
        config::TermMuxSetting::Herdr => match crate::herdr::probe(paths, home).await {
            Ok(binary) => herdr_plan(binary, state_dir, port),
            Err(reason) => {
                let reason = UnavailableReason::Probe(reason);
                tracing::warn!(
                    "terminal mux: term.mux is 'herdr' but {} — terminals unavailable",
                    reason.text()
                );
                MuxPlan::Unavailable(HerdrSetupUnavailable { reason })
            }
        },
    }
}

fn herdr_plan(binary: crate::herdr::HerdrBinary, state_dir: &Path, port: u16) -> MuxPlan {
    let uid = nix::unistd::Uid::current().as_raw();
    let config_home = crate::herdr::paths::resolve_config_home(state_dir, port, uid);
    match crate::herdr::paths::client_socket(&config_home) {
        Ok(_) => MuxPlan::Herdr(HerdrSetup {
            binary: binary.path,
            config_home,
        }),
        Err(Error::SocketPathTooLong { len, path }) => {
            let reason = UnavailableReason::SocketPath {
                len,
                path: std::path::PathBuf::from(path),
            };
            tracing::warn!(
                "terminal mux: term.mux is 'herdr' but {} — terminals unavailable",
                reason.text()
            );
            MuxPlan::Unavailable(HerdrSetupUnavailable { reason })
        }
        Err(error) => {
            let reason = UnavailableReason::Init {
                message: error.to_string(),
            };
            tracing::warn!(
                "terminal mux: term.mux is 'herdr' but {} — terminals unavailable",
                reason.text()
            );
            MuxPlan::Unavailable(HerdrSetupUnavailable { reason })
        }
    }
}
