use std::process::ExitCode;
use std::time::Duration;

use config::Environment;
use rivethub::Exit;

fn main() -> ExitCode {
    let env = Environment::from_process();
    rivethub::logging::init(&env);
    let exit = match build_runtime() {
        Ok(runtime) => {
            let exit = runtime.block_on(rivethub::service::start(&env));
            runtime.shutdown_timeout(Duration::from_secs(10));
            exit
        }
        Err(error) => {
            tracing::error!("the tokio runtime failed to start: {error}");
            Exit::Runtime
        }
    };
    exit.code()
}

fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
}
