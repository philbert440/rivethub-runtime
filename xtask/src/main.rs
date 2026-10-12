use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use xtask::check;
use xtask::error::{Error, Result};

enum Mode {
    Check { root: PathBuf, locked: bool },
    PreCommit,
    CommitMsg { file: PathBuf },
    Commits { range: String },
    InstallHooks,
}

fn parse(args: &[String]) -> Result<Mode> {
    let head = args.first().map(String::as_str);
    match head {
        Some("check") => {
            let mut root = PathBuf::from(".");
            let mut locked = false;
            for arg in args.iter().skip(1) {
                if arg == "--locked" {
                    locked = true;
                } else {
                    root = PathBuf::from(arg);
                }
            }
            Ok(Mode::Check {
                root: check::absolute(&root),
                locked,
            })
        }
        Some("pre-commit") => Ok(Mode::PreCommit),
        Some("commit-msg") => {
            let file = args
                .get(1)
                .ok_or_else(|| Error::Usage("commit-msg FILE".to_owned()))?;
            Ok(Mode::CommitMsg {
                file: PathBuf::from(file),
            })
        }
        Some("commits") => {
            let range = args
                .get(1)
                .ok_or_else(|| Error::Usage("commits RANGE".to_owned()))?;
            Ok(Mode::Commits {
                range: range.clone(),
            })
        }
        Some("install-hooks") => Ok(Mode::InstallHooks),
        _ => Err(Error::Usage(
            "usage: cargo xtask check [--locked]|pre-commit|commit-msg FILE|commits RANGE|install-hooks"
                .to_owned(),
        )),
    }
}

fn hook_mode(argv0: Option<&str>, args: &[String]) -> Option<Result<Mode>> {
    let invoked = argv0
        .and_then(|name| Path::new(name).file_name())?
        .to_str()?;
    match invoked {
        "pre-commit" => Some(Ok(Mode::PreCommit)),
        "commit-msg" => Some(match args.first() {
            Some(file) => Ok(Mode::CommitMsg {
                file: PathBuf::from(file),
            }),
            None => Err(Error::Usage("commit-msg FILE".to_owned())),
        }),
        _ => None,
    }
}

fn main() -> ExitCode {
    let mut raw = std::env::args();
    let invoked_as = raw.next();
    let args: Vec<String> = raw.collect();
    let root = check::absolute(&PathBuf::from("."));
    let outcome = match hook_mode(invoked_as.as_deref(), &args) {
        Some(mode) => mode,
        None => parse(&args),
    }
    .and_then(|mode| match mode {
        Mode::Check { root, locked } => check::check(&root, locked),
        Mode::PreCommit => check::pre_commit(&root),
        Mode::CommitMsg { file } => check::commit_msg(&root, &file),
        Mode::Commits { range } => check::commits(&root, &range),
        Mode::InstallHooks => check::install_hooks(&root),
    });
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Finding(_)) => ExitCode::FAILURE,
        Err(error) => {
            let stderr = std::io::stderr();
            let mut out = stderr.lock();
            let _ = writeln!(out, "xtask: {error}");
            let _ = out.flush();
            ExitCode::FAILURE
        }
    }
}
