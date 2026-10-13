use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return ExitCode::FAILURE;
    };
    match runtime.block_on(serve(&args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            best_effort_report(&error);
            ExitCode::FAILURE
        }
    }
}

async fn serve(args: &[String]) -> xtask::error::Result<()> {
    let invocation = xtask::recmcp::parse(args)?;
    xtask::recmcp::run(&invocation).await
}

#[expect(
    clippy::let_underscore_must_use,
    reason = "the failure report is the process's last write: a broken stderr cannot be reported anywhere else, and the failure exit code stands"
)]
#[expect(
    clippy::let_underscore_untyped,
    reason = "the failure report is the process's last write: a broken stderr cannot be reported anywhere else, and the failure exit code stands"
)]
fn best_effort_report(error: &xtask::error::Error) {
    let stderr = std::io::stderr();
    let mut out = stderr.lock();
    let _ = writeln!(out, "record-mcp: {error}");
    let _ = out.flush();
}
