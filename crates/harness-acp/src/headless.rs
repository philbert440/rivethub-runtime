use std::path::PathBuf;

use config::Config;
use config::provider::CliProvider;
use wire::harness::HarnessId;

use crate::{AcpError, AcpLaunch, LaunchTable};

pub(crate) async fn launch(
    config: &Config,
    harness: HarnessId,
    provider: &CliProvider,
    search: Vec<PathBuf>,
) -> Result<(LaunchTable, crate::discovery::Advertisement), AcpError> {
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
    command.working_dir = provider.cwd.clone().or(command.working_dir).or_else(|| {
        config
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.workspace.clone())
    });
    let program = command.program.clone();
    let resolved = tokio::task::spawn_blocking(move || resolve_executable(&program, &search))
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
    let cwd = command.working_dir.as_ref().ok_or(AcpError::WorkingDir)?;
    let discovery = crate::discovery::discover(&command, cwd);
    let advertisement = if let Some(timeout) = provider.timeout_ms {
        tokio::time::timeout(std::time::Duration::from_millis(timeout), discovery)
            .await
            .map_err(|_| AcpError::DiscoveryTimeout)??
    } else {
        discovery.await?
    };
    Ok((
        LaunchTable::from_entries([(harness, AcpLaunch::Native(command))]),
        advertisement,
    ))
}

fn resolve_executable(
    program: &std::path::Path,
    search: &[PathBuf],
) -> Result<Option<PathBuf>, std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(path) = crate::adapter::resolve_program(program, search)? else {
        return Ok(None);
    };
    let permissions = std::fs::metadata(&path)?.permissions();
    if permissions.mode() & 0o111 == 0 {
        return Ok(None);
    }
    Ok(Some(path))
}
