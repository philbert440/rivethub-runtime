pub mod ca;
pub mod cert;
mod configuration;
pub mod doctor;
mod error;
mod exit;
mod process;
pub mod service;
mod status;
pub mod tree;

use config::Environment;
use tokio_util::sync::CancellationToken;

pub use error::Error;
pub use exit::Exit;
pub use tree::{Cli, Command, ConfigCommand};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub stdout: String,
    pub stderr: String,
    pub exit: Exit,
}

impl Report {
    pub fn success(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            exit: Exit::Ok,
        }
    }

    pub fn failure(error: &Error) -> Self {
        Self {
            stdout: String::new(),
            stderr: format!("Error: {error}\n"),
            exit: Exit::Failure,
        }
    }

    pub fn write(self) -> Result<Exit, std::io::Error> {
        use std::io::Write;
        std::io::stdout().lock().write_all(self.stdout.as_bytes())?;
        std::io::stderr().lock().write_all(self.stderr.as_bytes())?;
        Ok(self.exit)
    }
}

#[derive(Debug)]
pub enum Dispatch {
    Gateway {
        check: bool,
    },
    Start,
    Worker {
        role: workers::ProcessRole,
        check: bool,
    },
    Mcp {
        check: bool,
    },
    Agent {
        check: bool,
    },
    Complete(Report),
}

pub async fn dispatch(cli: Cli, env: &Environment, shutdown: &CancellationToken) -> Dispatch {
    let Some(command) = cli.command else {
        return Dispatch::Complete(Report::success(Cli::help()));
    };
    let structured_shutdown = matches!(
        command,
        Command::Ca(_) | Command::Cert { .. } | Command::Service { .. }
    );
    let operation = async {
        match command {
            Command::Start => return Ok(Dispatch::Start),
            Command::Gateway { check } => return Ok(Dispatch::Gateway { check }),
            Command::Worker { role, check } => return Ok(Dispatch::Worker { role, check }),
            Command::Mcp { check } => return Ok(Dispatch::Mcp { check }),
            Command::Agent { check } => return Ok(Dispatch::Agent { check }),
            Command::Version => Ok(Report::success(version())),
            Command::Config { command } => configuration::run(command, env).await,
            Command::Doctor { json } => doctor::run(env, json).await,
            Command::Status => Ok(status::run(env).await),
            Command::Stop => process::stop(env).await,
            Command::Service { command } => {
                service::Scope::for_uid(nix::unistd::geteuid().as_raw())
                    .run(command, env, shutdown)
                    .await
                    .map_err(Error::from)
            }
            Command::Ca(args) => ca::run(args, env, shutdown).await,
            Command::Cert { command } => cert::run(command, env, shutdown).await,
        }
        .map(Dispatch::Complete)
    };
    let result = if structured_shutdown {
        operation.await
    } else {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => Err(Error::Cancelled),
            result = operation => result,
        }
    };
    match result {
        Ok(dispatch) => dispatch,
        Err(error) => Dispatch::Complete(Report::failure(&error)),
    }
}

fn version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match option_env!("RIVETHUB_BUILD_REVISION") {
        Some(revision) => format!("RivetHub v{version} ({revision})\n"),
        None => format!("RivetHub v{version}\n"),
    }
}
