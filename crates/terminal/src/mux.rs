use std::path::Path;
use std::path::PathBuf;

use crate::error::Error;
use crate::herdr::ProbeFailure;

#[derive(Debug, Clone)]
pub struct HerdrSetup {
    pub binary: PathBuf,
    pub config_home: PathBuf,
}

#[derive(Debug, Clone)]
pub enum UnavailableReason {
    Probe(std::sync::Arc<ProbeFailure>),
    SocketPath { len: usize, path: PathBuf },
    Init(std::sync::Arc<Error>),
}

impl UnavailableReason {
    #[must_use]
    pub fn text(&self) -> String {
        self.to_error().to_string()
    }

    pub fn to_error(&self) -> Error {
        match self {
            Self::Probe(failure) => Error::Probe(std::sync::Arc::clone(failure)),
            Self::SocketPath { len, path } => Error::SocketPathTooLong {
                len: *len,
                path: Box::from(path.as_path()),
            },
            Self::Init(error) => Error::PtyFailure(std::sync::Arc::clone(error)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HerdrSetupUnavailable {
    pub reason: UnavailableReason,
}

#[derive(Debug, Clone)]
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
                let reason = UnavailableReason::Probe(std::sync::Arc::new(reason));
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
                let reason = UnavailableReason::Probe(std::sync::Arc::new(reason));
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
    let config_home = crate::herdr::paths::resolve_config_home(state_dir, port);
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
            let reason = UnavailableReason::Init(std::sync::Arc::new(error));
            tracing::warn!(
                "terminal mux: term.mux is 'herdr' but {} — terminals unavailable",
                reason.text()
            );
            MuxPlan::Unavailable(HerdrSetupUnavailable { reason })
        }
    }
}
