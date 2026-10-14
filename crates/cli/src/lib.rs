mod configuration;
pub mod doctor;
mod error;
mod exit;
mod process;
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
    Start,
    Complete(Report),
}

pub async fn dispatch(cli: Cli, env: &Environment, shutdown: &CancellationToken) -> Dispatch {
    let Some(command) = cli.command else {
        return Dispatch::Complete(Report::success(Cli::help()));
    };
    let operation = async {
        match command {
            Command::Start => return Ok(Dispatch::Start),
            Command::Version => Ok(Report::success(version())),
            Command::Config { command } => configuration::run(command, env).await,
            Command::Doctor { json } => doctor::run(env, json).await,
            Command::Status => Ok(status::run(env).await),
            Command::Stop => process::stop(env).await,
        }
        .map(Dispatch::Complete)
    };
    let result = tokio::select! {
        biased;
        () = shutdown.cancelled() => Err(Error::Cancelled),
        result = operation => result,
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
