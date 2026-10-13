use std::process::ExitCode;

use crate::service::StartupError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok,
    Config,
    Memory,
    Bind,
    Serve,
    Runtime,
}

impl Exit {
    #[must_use]
    pub fn code(self) -> ExitCode {
        match self {
            Self::Ok => ExitCode::SUCCESS,
            Self::Config => ExitCode::from(1),
            Self::Memory => ExitCode::from(2),
            Self::Bind => ExitCode::from(3),
            Self::Serve => ExitCode::from(4),
            Self::Runtime => ExitCode::from(5),
        }
    }

    #[must_use]
    pub fn from_startup(error: &StartupError) -> Self {
        match error {
            StartupError::Config(_) | StartupError::Url => Self::Config,
            StartupError::Memory(_) => Self::Memory,
            StartupError::Bind(_) => Self::Bind,
            StartupError::Serve(_) | StartupError::Io(_) => Self::Serve,
        }
    }
}
