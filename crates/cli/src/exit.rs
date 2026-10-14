use std::process::ExitCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok,
    Failure,
    Memory,
    Bind,
    Serve,
    Runtime,
}

impl Exit {
    pub fn code(self) -> ExitCode {
        match self {
            Self::Ok => ExitCode::SUCCESS,
            Self::Failure => ExitCode::from(1),
            Self::Memory => ExitCode::from(2),
            Self::Bind => ExitCode::from(3),
            Self::Serve => ExitCode::from(4),
            Self::Runtime => ExitCode::from(5),
        }
    }
}
