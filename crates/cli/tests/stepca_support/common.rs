use base64::Engine;
use cli::{Cli, Dispatch, Report};
use config::Environment;
use serde::Serialize;
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
        Dispatch::Gateway { .. }
        | Dispatch::Worker { .. }
        | Dispatch::Mcp { .. }
        | Dispatch::Agent { .. } => Err("unexpected service launch".into()),
    }
}

pub(crate) fn text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| "non-UTF8 test path".into())
}

pub(crate) fn token(sans: &[&str]) -> Result<String> {
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"ES256"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&TokenClaims { sans })?);
    Ok(format!("{header}.{payload}.e30"))
}

#[derive(Serialize)]
struct TokenClaims<'a> {
    sans: &'a [&'a str],
}
