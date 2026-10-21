#![cfg(test)]

use cli::{Cli, Dispatch, Report};
use config::Environment;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

pub(crate) fn home() -> TempDir {
    tempfile::tempdir().expect("temporary home")
}

pub(crate) fn environment(home: &TempDir) -> Environment {
    Environment::from_vars([("HOME".as_ref(), home.path().as_os_str())])
}

pub(crate) fn config(home: &TempDir, extra: &str) {
    std::fs::create_dir_all(home.path().join(".rivethub")).expect("state directory");
    std::fs::write(home.path().join(".rivethub/config.yaml"), format!("runtime:\n  workspace: /home/user/work\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:1/v1\n    model: m\n    api_key: secret-that-must-stay-private\n{extra}")).expect("config");
}

pub(crate) async fn run(args: &[&str], env: &Environment) -> Report {
    let parsed = Cli::parse_args(
        std::iter::once("rh")
            .chain(args.iter().copied())
            .map(Into::into),
    );
    let cli = match parsed {
        Ok(cli) => cli,
        Err(report) => return report,
    };
    let output = cli::dispatch(cli, env, &CancellationToken::new()).await;
    let Dispatch::Complete(report) = output else {
        panic!("unexpected service start")
    };
    report
}
