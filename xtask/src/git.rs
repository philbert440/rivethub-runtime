use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(Error::Io)?;
    if !output.status.success() {
        return Err(Error::Command {
            command: format!("git {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn tracked_files(root: &Path) -> Result<Vec<String>> {
    Ok(git(root, &["ls-files"])?
        .lines()
        .filter(|file| !file.is_empty())
        .map(str::to_owned)
        .collect())
}
