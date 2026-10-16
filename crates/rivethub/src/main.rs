use std::process::ExitCode;
use std::time::Duration;

use config::Environment;
use rivethub::Exit;

fn main() -> ExitCode {
    let env = Environment::from_process();
    let cli = match cli::Cli::parse_args(std::env::args_os()) {
        Ok(cli) => cli,
        Err(report) => return write(report).code(),
    };
    let exit = match build_runtime() {
        Ok(runtime) => {
            let exit = runtime.block_on(dispatch(cli, &env));
            runtime.shutdown_timeout(Duration::from_secs(10));
            exit
        }
        Err(error) => write(cli::Report {
            stdout: String::new(),
            stderr: format!("Error: the tokio runtime failed to start: {error}\n"),
            exit: Exit::Runtime,
            stdout_raw: Vec::new(),
        }),
    };
    exit.code()
}

fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
}

async fn dispatch(command: cli::Cli, env: &Environment) -> Exit {
    if matches!(
        command.command,
        Some(cli::Command::Start { .. } | cli::Command::Local { .. })
    ) {
        rivethub::logging::init(env);
    }
    let shutdown = tokio_util::sync::CancellationToken::new();
    let mut sigterm = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        Ok(signal) => signal,
        Err(error) => return write(cli::Report::failure(&cli::Error::Io(error))),
    };
    let operation = cli::dispatch(command, env, &shutdown);
    tokio::pin!(operation);
    let dispatch = tokio::select! {
        dispatch = &mut operation => dispatch,
        signal = tokio::signal::ctrl_c() => {
            if let Err(error) = signal { return write(cli::Report::failure(&cli::Error::Io(error))); }
            shutdown.cancel();
            operation.await
        }
        _ = sigterm.recv() => {
            shutdown.cancel();
            operation.await
        }
    };
    match dispatch {
        cli::Dispatch::Gateway { check, config } => {
            rivethub::logging::init(env);
            let env = overlay(env, config);
            rivethub::service::start_with_check(&env, check).await
        }
        cli::Dispatch::Worker {
            role,
            check,
            config,
        } => {
            rivethub::logging::init(env);
            let env = overlay(env, config);
            rivethub::worker::start(&env, role, check).await
        }
        cli::Dispatch::Mcp { check, config } => {
            rivethub::logging::init(env);
            let env = overlay(env, config);
            rivethub::sidecar::start(&env, check).await
        }
        cli::Dispatch::Agent { check, config } => {
            rivethub::logging::init(env);
            let env = overlay(env, config);
            rivethub::agent::start(&env, check).await
        }
        cli::Dispatch::Local(prepared) => rivethub::local::start(env, *prepared).await,
        cli::Dispatch::Complete(report) => write(report),
    }
}

fn overlay(env: &Environment, config: Option<std::path::PathBuf>) -> Environment {
    match config {
        Some(path) => env
            .clone()
            .with_override(config::CONFIG_PATH_VARIABLE, path),
        None => env.clone(),
    }
}

fn write(report: cli::Report) -> Exit {
    match report.write() {
        Ok(exit) => exit,
        Err(error) => {
            tracing::error!("command output failed: {error}");
            Exit::Failure
        }
    }
}
