use cli::{Cli, Dispatch, Report};
use config::Environment;
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub(crate) type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(crate) fn directory() -> Result<tempfile::TempDir> {
    Ok(tempfile::tempdir()?)
}

pub(crate) async fn run(home: &Path, args: &[&str]) -> Result<Report> {
    let env = Environment::from_vars([("HOME".as_ref(), home.as_os_str())]);
    let parsed = Cli::parse_args(
        std::iter::once("rh")
            .chain(args.iter().copied())
            .map(Into::into),
    );
    let command = match parsed {
        Ok(command) => command,
        Err(report) => return Ok(report),
    };
    match cli::dispatch(command, &env, &CancellationToken::new()).await {
        Dispatch::Complete(report) => Ok(report),
        Dispatch::Start
        | Dispatch::Gateway { .. }
        | Dispatch::Worker { .. }
        | Dispatch::Mcp { .. } => Err("unexpected service launch".into()),
    }
}

pub(crate) fn text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| "non-UTF8 test path".into())
}
