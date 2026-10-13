use crate::error::{Error, Result};
use crate::git::git;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn finish(label: &str, findings: &[String]) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for finding in findings {
        let _ = writeln!(out, "{finding}");
    }
    let state = if findings.is_empty() { "OK" } else { "FAIL" };
    let _ = writeln!(out, "GATE {state}: {} finding(s), {label}", findings.len());
    let _ = out.flush();
    if findings.is_empty() {
        Ok(())
    } else {
        Err(Error::Finding(format!("{} finding(s)", findings.len())))
    }
}

pub fn run_tool(root: &Path, command: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(command)
        .current_dir(root)
        .args(args)
        .status()
        .map_err(Error::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Command {
            command: format!("{command} {}", args.join(" ")),
            stderr: String::new(),
        })
    }
}

pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
}

pub fn machete(root: &Path) -> Result<()> {
    machete_with(root, "cargo-machete")
}

fn machete_with(root: &Path, program: &str) -> Result<()> {
    match Command::new(program).current_dir(root).status() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Error::MissingTool {
            tool: program.to_owned(),
            install: "cargo install cargo-machete --locked".to_owned(),
        }),
        Err(error) => Err(Error::Io(error)),
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(Error::Command {
            command: program.to_owned(),
            stderr: String::new(),
        }),
    }
}

pub fn check(root: &Path, locked: bool) -> Result<()> {
    run_tool(root, "cargo", &["fmt", "--all", "--check"])?;
    let mut clippy = vec!["clippy"];
    if locked {
        clippy.push("--locked");
    }
    clippy.extend([
        "--workspace",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ]);
    run_tool(root, "cargo", &clippy)?;
    let dylint = Command::new("cargo")
        .current_dir(root)
        .args(["dylint", "--all", "--workspace", "--", "--all-targets"])
        .env("DYLINT_RUSTFLAGS", "-Dwarnings")
        .status()
        .map_err(Error::Io)?;
    if !dylint.success() {
        return Err(Error::Command {
            command: "cargo dylint --all --workspace".to_owned(),
            stderr: String::new(),
        });
    }
    let mut deny = vec!["deny", "--all-features"];
    if locked {
        deny.push("--locked");
    }
    deny.push("check");
    run_tool(root, "cargo", &deny)?;
    machete(root)?;

    let mut findings = Vec::new();
    crate::lang::check(root, &mut findings)?;
    crate::code::check_code(root, &mut findings)?;
    crate::manifests::manifests(root, &mut findings)?;
    crate::manifests::lint_policy(root, &mut findings)?;
    crate::toolconfig::check(root, &mut findings)?;
    crate::patterns::check(root, &mut findings)?;
    crate::dupfixture::check(root, &mut findings)?;
    crate::labpath::check(root, &mut findings)?;
    crate::lang::privacy_and_secrets(root, &mut findings)?;
    finish("cargo xtask check", &findings)
}

pub fn pre_commit(root: &Path) -> Result<()> {
    let mut findings = Vec::new();
    crate::lang::staged_privacy(root, &mut findings)?;
    let status = Command::new("cargo")
        .current_dir(root)
        .args(["fmt", "--all", "--check"])
        .status()
        .map_err(Error::Io)?;
    if !status.success() {
        findings.push("FMT staged changes must be formatted (cargo fmt --all)".to_owned());
    }
    finish("pre-commit", &findings)
}

pub fn commit_msg(root: &Path, file: &Path) -> Result<()> {
    let name = git(root, &["config", "user.name"])?.trim().to_owned();
    let email = git(root, &["config", "user.email"])?.trim().to_owned();
    let text = std::fs::read_to_string(file)?;
    let mut findings = Vec::new();
    crate::manifests::commit_message(root, &mut findings, &text, Some((&name, &email)))?;
    finish("commit-msg", &findings)
}

pub fn commits(root: &Path, range: &str) -> Result<()> {
    let mut findings = Vec::new();
    crate::manifests::commits(root, range, &mut findings)?;
    finish(&format!("commits {range}"), &findings)
}

pub fn install_hooks(root: &Path) -> Result<()> {
    let binary = std::env::current_exe()?;
    install_hooks_from(root, &binary)
}

pub fn install_hooks_from(root: &Path, binary: &Path) -> Result<()> {
    let hooks = root.join("target").join("git-hooks");
    std::fs::create_dir_all(&hooks)?;
    for name in ["pre-commit", "commit-msg"] {
        let hook = hooks.join(name);
        std::fs::copy(binary, &hook)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&hook)?.permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&hook, permissions)?;
        }
    }
    git(root, &["config", "core.hooksPath", "target/git-hooks"])?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(
        out,
        "hooks installed: target/git-hooks/pre-commit, target/git-hooks/commit-msg (core.hooksPath = target/git-hooks)"
    );
    let _ = out.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::machete_with;
    use std::path::Path;

    #[test]
    fn missing_machete_names_the_install_command() {
        let error = machete_with(Path::new("."), "xtask-machete-missing").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cargo install cargo-machete --locked"),
            "the error must carry the install command, got: {error}"
        );
    }

    #[test]
    fn machete_failure_is_a_command_error() {
        let error = machete_with(Path::new("."), "false").unwrap_err();
        assert!(
            matches!(error, crate::error::Error::Command { ref command, .. } if command == "false"),
            "a non-zero exit must fail the gate as a command error, got: {error}"
        );
    }
}
