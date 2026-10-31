use std::path::PathBuf;
use std::time::Duration;

use config::Config;
use config::provider::CliProvider;
use wire::harness::HarnessId;

use crate::discovery::{Advertisement, DEFAULT_DISCOVERY_TIMEOUT};
use crate::{AcpCommand, AcpError, AcpHarnessAdapter, AcpLaunch, LaunchTable};

#[derive(Debug)]
pub(crate) struct Prepared {
    adapter: AcpHarnessAdapter,
    harness: HarnessId,
    command: AcpCommand,
    cwd: PathBuf,
    timeout: Duration,
}

impl Prepared {
    pub(crate) async fn discover(&self) -> Result<Advertisement, AcpError> {
        crate::discovery::discover(self.harness, &self.command, &self.cwd, self.timeout).await
    }

    pub(crate) fn discover_in(&mut self, directory: PathBuf) {
        self.cwd = directory;
    }

    pub(crate) fn ready(self, advertisement: Advertisement) -> AcpHarnessAdapter {
        self.adapter.advertise(self.harness, advertisement)
    }
}

pub(crate) async fn prepare(
    config: &Config,
    harness: HarnessId,
    provider: &CliProvider,
    search: Vec<PathBuf>,
) -> Result<Prepared, AcpError> {
    let launch = LaunchTable::from_config(config).launch(harness)?;
    let mut command = launch.command().clone();
    if let Some(binary) = &provider.binary {
        match (&launch, harness) {
            (AcpLaunch::Adapter(_), HarnessId::ClaudeCode) => command
                .env
                .push(("CLAUDE_CODE_EXECUTABLE".to_owned(), binary.clone())),
            (AcpLaunch::Adapter(_), _) => {}
            _ => command.program = PathBuf::from(binary),
        }
    }
    command
        .env
        .push((config::MODEL_RUN_VARIABLE.to_owned(), "1".to_owned()));
    command.working_dir = provider.cwd.clone().or(command.working_dir).or_else(|| {
        config
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.workspace.clone())
    });
    let program = command.program.clone();
    let paths = search.clone();
    let resolved = tokio::task::spawn_blocking(move || resolve_executable(&program, &paths))
        .await
        .map_err(AcpError::ProbeTask)?
        .map_err(|source| AcpError::Launch {
            program: command.program.display().to_string(),
            source,
        })?;
    let Some(program) = resolved else {
        return Err(AcpError::UnverifiedLaunch {
            harness,
            program: command.program.display().to_string(),
            reason: crate::UnverifiedReason::NotInstalled,
        });
    };
    command.program = program;
    let cwd = command.working_dir.clone().ok_or(AcpError::WorkingDir)?;
    let timeout = provider
        .discovery_timeout_ms
        .map_or(DEFAULT_DISCOVERY_TIMEOUT, Duration::from_millis);
    Ok(Prepared {
        adapter: AcpHarnessAdapter::headless(config, harness, command.clone(), search),
        harness,
        command,
        cwd,
        timeout,
    })
}

fn resolve_executable(
    program: &std::path::Path,
    search: &[PathBuf],
) -> Result<Option<PathBuf>, std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(path) = crate::launch::resolve_program(program, search)? else {
        return Ok(None);
    };
    let permissions = std::fs::metadata(&path)?.permissions();
    if permissions.mode() & 0o111 == 0 {
        return Ok(None);
    }
    Ok(Some(path))
}
