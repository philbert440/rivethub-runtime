use std::path::{Component, PathBuf};

use super::TerminalOptions;
use crate::error::ApiError;

pub(super) fn validate_options(mut options: TerminalOptions) -> Result<TerminalOptions, ApiError> {
    options.root = options.root.canonicalize().map_err(ApiError::Io)?;
    if options.max_ptys == 0
        || !options
            .commands
            .iter()
            .any(|command| command.id == options.default)
    {
        return Err(ApiError::BadRequest(
            "invalid terminal configuration".into(),
        ));
    }
    for command in &mut options.commands {
        if command.argv.is_empty() {
            return Err(ApiError::BadRequest(
                "invalid terminal command or directory".into(),
            ));
        }
        if !command.cwd.is_absolute() {
            return Err(ApiError::BadRequest(
                "invalid terminal command or directory".into(),
            ));
        }
    }
    Ok(options)
}

pub(super) fn contained_directory(
    root: &std::path::Path,
    directory: &std::path::Path,
) -> Result<PathBuf, ApiError> {
    if !directory.is_absolute()
        || directory
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return Err(ApiError::BadRequest(
            "invalid terminal command or directory".into(),
        ));
    }
    let directory = match directory.canonicalize() {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::BadRequest(
                "working directory does not exist".into(),
            ));
        }
        Err(error) => return Err(ApiError::Io(error)),
    };
    if !directory.starts_with(root) {
        return Err(ApiError::Forbidden(
            "working directory is outside the terminal root".into(),
        ));
    }
    if !directory.is_dir() {
        return Err(ApiError::BadRequest(
            "working directory is not a directory".into(),
        ));
    }
    Ok(directory)
}
